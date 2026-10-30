//! `rss.dat`: the RSS Downloader's feeds, filters, history and per-feed memory as one bencoded
//! dictionary next to `patorrent.ini` (µTorrent kept its own `rss.dat` there, also bencoded;
//! this layout is PaTorrent's). Unknown keys are ignored and missing ones take their defaults,
//! so a damaged or foreign file loads as far as it makes sense.
//!
//! ```text
//! version   1
//! settings  { interval, smart_repack, feed_label }
//! next_id
//! feeds     [ { id, url, alias, enabled, interval (0 = default), subscribe, subscribe_smart,
//!               title, last_refresh, last_error, etag, last_modified, downloaded [ids],
//!               seen [ids], episodes [keys] } ]
//! filters   [ { id, name, enabled, pattern, not, save_in, feed (0 = all), quality, episodes,
//!               label, min_interval, smart, top_priority, paused, last_match, seen_episodes } ]
//! history   [ { title, url, feed, time } ]
//! ```

use std::collections::VecDeque;
use std::path::Path;

use patorrent_core::bencode::{Entry, bdecode_with_limits};

use super::{
    HistoryEntry, MAX_EPISODES, MAX_FEED_HISTORY, MAX_HISTORY, Quality, RecentSet, RssDownloader, RssFeed,
    RssFilter, RssSettings,
};

/// The file name, in the settings folder.
pub const FILE_NAME: &str = "rss.dat";
const VERSION: i64 = 1;

fn dict(pairs: Vec<(&str, Entry)>) -> Entry {
    let mut e = Entry::new_dict();
    for (k, v) in pairs {
        e.dict_mut().insert(k.as_bytes().to_vec(), v);
    }
    e
}

fn strings<'a>(it: impl Iterator<Item = &'a str>) -> Entry {
    Entry::List(it.map(Entry::from).collect())
}

fn flag(b: bool) -> Entry {
    Entry::Int(b as i64)
}

impl RssDownloader {
    /// The bencoded `rss.dat`.
    pub fn to_bencode(&self) -> Vec<u8> {
        let s = &self.settings;
        let feeds = self
            .feeds
            .iter()
            .map(|f| {
                dict(vec![
                    ("id", Entry::Int(f.id.into())),
                    ("url", f.url.as_str().into()),
                    ("alias", f.alias.as_str().into()),
                    ("enabled", flag(f.enabled)),
                    ("interval", Entry::Int(f.interval.unwrap_or(0).into())),
                    ("subscribe", flag(f.subscribe)),
                    ("subscribe_smart", flag(f.subscribe_smart)),
                    ("title", f.title.as_str().into()),
                    ("last_refresh", Entry::Int(f.last_refresh.unwrap_or(0))),
                    ("last_error", f.last_error.as_str().into()),
                    ("etag", f.etag.as_str().into()),
                    ("last_modified", f.last_modified.as_str().into()),
                    ("downloaded", strings(f.downloaded.iter())),
                    ("seen", strings(f.seen.iter())),
                    ("episodes", strings(f.episodes.iter())),
                ])
            })
            .collect();
        let filters = self
            .filters
            .iter()
            .map(|f| {
                dict(vec![
                    ("id", Entry::Int(f.id.into())),
                    ("name", f.name.as_str().into()),
                    ("enabled", flag(f.enabled)),
                    ("pattern", f.pattern.as_str().into()),
                    ("not", f.not.as_str().into()),
                    ("save_in", f.save_in.as_str().into()),
                    ("feed", Entry::Int(f.feed.unwrap_or(0).into())),
                    ("quality", Entry::Int(f.quality.0.into())),
                    ("episodes", f.episodes.as_str().into()),
                    ("label", f.label.as_str().into()),
                    ("min_interval", Entry::Int(f.min_interval.into())),
                    ("smart", flag(f.smart)),
                    ("top_priority", flag(f.top_priority)),
                    ("paused", flag(f.paused)),
                    ("last_match", Entry::Int(f.last_match.unwrap_or(0))),
                    ("seen_episodes", strings(f.seen_episodes.iter().map(String::as_str))),
                ])
            })
            .collect();
        let history = self
            .history
            .iter()
            .map(|h| {
                dict(vec![
                    ("title", h.title.as_str().into()),
                    ("url", h.url.as_str().into()),
                    ("feed", Entry::Int(h.feed.into())),
                    ("time", Entry::Int(h.time)),
                ])
            })
            .collect();
        dict(vec![
            ("version", Entry::Int(VERSION)),
            (
                "settings",
                dict(vec![
                    ("interval", Entry::Int(s.interval.into())),
                    ("smart_repack", flag(s.smart_repack)),
                    ("feed_label", flag(s.feed_as_default_label)),
                ]),
            ),
            ("next_id", Entry::Int(self.next_id.into())),
            ("feeds", Entry::List(feeds)),
            ("filters", Entry::List(filters)),
            ("history", Entry::List(history)),
        ])
        .bencode()
    }

    /// Reads `rss.dat`; `None` when it is not bencoded at all. Whatever else is wrong is
    /// skipped or defaulted.
    pub fn from_bencode(data: &[u8]) -> Option<RssDownloader> {
        let tree = bdecode_with_limits(data, 100, 10_000_000).ok()?;
        let root = Entry::from_node(tree.root()?);
        root.as_dict()?;
        let int = |e: &Entry, k: &str| e.get(k).and_then(Entry::as_int);
        let u32_of = |e: &Entry, k: &str| int(e, k).and_then(|v| u32::try_from(v).ok());
        let text = |e: &Entry, k: &str| {
            e.get(k)
                .and_then(Entry::as_bytes)
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default()
        };
        let boolean = |e: &Entry, k: &str, d: bool| int(e, k).map_or(d, |v| v != 0);
        let list = |e: &Entry, k: &str| e.get(k).and_then(Entry::as_list).unwrap_or_default().to_vec();
        let set = |e: &Entry, k: &str, cap: usize| -> RecentSet {
            let l = list(e, k);
            let mut owned: Vec<String> = l
                .iter()
                .filter_map(Entry::as_bytes)
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .collect();
            if owned.len() > cap {
                owned.drain(..owned.len() - cap);
            }
            owned.iter().map(String::as_str).collect()
        };
        let defaults = RssSettings::default();
        let settings = match root.get("settings") {
            Some(s) => RssSettings {
                interval: u32_of(s, "interval").filter(|&v| v > 0).unwrap_or(defaults.interval),
                smart_repack: boolean(s, "smart_repack", defaults.smart_repack),
                feed_as_default_label: boolean(s, "feed_label", defaults.feed_as_default_label),
            },
            None => defaults,
        };
        let mut d = RssDownloader {
            settings,
            next_id: u32_of(&root, "next_id").unwrap_or(1),
            ..RssDownloader::default()
        };
        for f in list(&root, "feeds") {
            let Some(id) = u32_of(&f, "id").filter(|&id| id != 0) else { continue };
            if d.feeds.iter().any(|x| x.id == id) {
                continue;
            }
            d.feeds.push(RssFeed {
                id,
                url: text(&f, "url"),
                alias: text(&f, "alias"),
                enabled: boolean(&f, "enabled", true),
                interval: u32_of(&f, "interval").filter(|&v| v > 0),
                subscribe: boolean(&f, "subscribe", false),
                subscribe_smart: boolean(&f, "subscribe_smart", false),
                title: text(&f, "title"),
                last_refresh: int(&f, "last_refresh").filter(|&t| t > 0),
                last_error: text(&f, "last_error"),
                etag: text(&f, "etag"),
                last_modified: text(&f, "last_modified"),
                downloaded: set(&f, "downloaded", MAX_FEED_HISTORY * 4),
                seen: set(&f, "seen", MAX_FEED_HISTORY * 4),
                episodes: set(&f, "episodes", MAX_EPISODES),
                items: Vec::new(),
            });
        }
        for f in list(&root, "filters") {
            let Some(id) = u32_of(&f, "id").filter(|&id| id != 0) else { continue };
            if d.filters.iter().any(|x| x.id == id) || d.feeds.iter().any(|x| x.id == id) {
                continue;
            }
            d.filters.push(RssFilter {
                id,
                name: text(&f, "name"),
                enabled: boolean(&f, "enabled", true),
                pattern: text(&f, "pattern"),
                not: text(&f, "not"),
                save_in: text(&f, "save_in"),
                feed: u32_of(&f, "feed").filter(|&v| v != 0),
                quality: Quality(u32_of(&f, "quality").unwrap_or(0)),
                episodes: text(&f, "episodes"),
                label: text(&f, "label"),
                min_interval: u32_of(&f, "min_interval").unwrap_or(0),
                smart: boolean(&f, "smart", false),
                top_priority: boolean(&f, "top_priority", false),
                paused: boolean(&f, "paused", false),
                last_match: int(&f, "last_match").filter(|&t| t > 0),
                seen_episodes: set(&f, "seen_episodes", MAX_EPISODES).iter().map(str::to_owned).collect(),
            });
        }
        let mut history = VecDeque::new();
        for h in list(&root, "history") {
            history.push_back(HistoryEntry {
                title: text(&h, "title"),
                url: text(&h, "url"),
                feed: u32_of(&h, "feed").unwrap_or(0),
                time: int(&h, "time").unwrap_or(0),
            });
        }
        while history.len() > MAX_HISTORY {
            history.pop_front();
        }
        d.history = history;
        Some(d)
    }

    /// Loads `path`; a missing or unreadable file gives an empty downloader.
    pub fn load(path: &Path) -> RssDownloader {
        std::fs::read(path).ok().and_then(|b| RssDownloader::from_bencode(&b)).unwrap_or_default()
    }

    /// Writes `path` through a temporary file, so a crash never leaves half of it; owner-only
    /// on Unix, as a feed URL can carry a login or a key.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use patorrent_core::util::private_file;
        if let Some(dir) = path.parent() {
            private_file::create_dir_all(dir)?;
        }
        private_file::write(path, &self.to_bencode())
    }
}
