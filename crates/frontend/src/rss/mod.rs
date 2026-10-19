//! RSS feeds and µTorrent 2.2.1's RSS Downloader, without the GUI: feed parsing ([`feed`]),
//! episode and quality detection ([`episode`]), filters ([`filter`]), and the [`RssDownloader`]
//! model that decides what to add and remembers what it added, persisted to `rss.dat`
//! ([`store`]).
//!
//! The model is pure: a frontend asks [`RssDownloader::due_feeds`] which feeds to refresh,
//! fetches them on a worker thread ([`fetch_feed`]), hands the parsed document to
//! [`RssDownloader::feed_fetched`] and adds the [`AddRequest`]s that come back. Nothing in the
//! model starts threads or timers; [`worker`] is the shared helper that runs those fetches for
//! a frontend, and [`view`] formats what the dialogs and the feed item list show.
//!
//! What follows µTorrent 2.2.1 is from memory of its dialogs and Advanced settings (no source
//! or documentation was at hand): the filter fields, the feed's "Automatically download all
//! items published in feed" subscription with its own smart episode filter, the
//! `url:COOKIE:name=value;...` feed URL suffix, `rss.update_interval` (15 minutes),
//! `rss.smart_repack_filter` and `rss.feed_as_default_label`. Where a detail was uncertain the
//! documentation of the item says so.

pub mod episode;
pub mod feed;
pub mod filter;
pub mod store;
pub mod view;
pub mod worker;

use std::collections::{HashSet, VecDeque};

use patorrent_core::fetch::{FetchOptions, FetchProxy, http_get};

pub use episode::{Episode, EpisodeSpec, Quality, is_repack, parse_episode};
pub use feed::{Enclosure, Feed, FeedItem, FeedKind, parse_date, parse_feed};
pub use filter::{FilterId, MIN_INTERVALS, RssFilter, wildcard_match};
pub use worker::{Collected, Download, Worker, fetch_download};

pub type FeedId = u32;

/// Item ids remembered per feed (added ones, and seen ones), beyond those still in the feed.
pub const MAX_FEED_HISTORY: usize = 2000;
/// Episodes remembered per smart filter.
pub const MAX_EPISODES: usize = 2000;
/// Entries of the History list.
pub const MAX_HISTORY: usize = 1000;

/// Strings in insertion order with fast lookup; the oldest go first when it is pruned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentSet {
    order: VecDeque<String>,
    set: HashSet<String>,
}

impl RecentSet {
    pub fn contains(&self, s: &str) -> bool {
        self.set.contains(s)
    }

    pub fn insert(&mut self, s: &str) {
        if self.set.insert(s.to_owned()) {
            self.order.push_back(s.to_owned());
        }
    }

    pub fn remove(&mut self, s: &str) {
        if self.set.remove(s) {
            self.order.retain(|x| x != s);
        }
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(String::as_str)
    }

    /// Drops the oldest entries until at most `cap` are left, never one in `keep`.
    pub fn prune(&mut self, cap: usize, keep: &HashSet<&str>) {
        let mut excess = self.order.len().saturating_sub(cap);
        if excess == 0 {
            return;
        }
        let set = &mut self.set;
        self.order.retain(|s| {
            if excess > 0 && !keep.contains(s.as_str()) {
                excess -= 1;
                set.remove(s);
                false
            } else {
                true
            }
        });
    }
}

impl<'a> FromIterator<&'a str> for RecentSet {
    fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> RecentSet {
        let mut r = RecentSet::default();
        for s in iter {
            r.insert(s);
        }
        r
    }
}

/// A feed and what the downloader remembers about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RssFeed {
    pub id: FeedId,
    /// As entered, with µTorrent's optional `:COOKIE:` suffix (see [`split_cookie`]).
    pub url: String,
    /// The name in the Feeds list; empty shows the feed's own title.
    pub alias: String,
    pub enabled: bool,
    /// Refresh interval in minutes; `None` uses `RssSettings::interval`.
    pub interval: Option<u32>,
    /// "Automatically download all items published in feed".
    pub subscribe: bool,
    /// "Use smart episode filter" for the subscription.
    pub subscribe_smart: bool,
    /// The channel title of the last fetch.
    pub title: String,
    /// When the feed was last fetched (successfully or not), unix time.
    pub last_refresh: Option<i64>,
    /// Why the last fetch failed; empty after a success.
    pub last_error: String,
    /// Ids ([`FeedItem::id`]) of the items added from this feed.
    pub downloaded: RecentSet,
    /// Ids of every item seen in this feed (the UI marks the others as new).
    pub seen: RecentSet,
    /// Episodes the subscription's smart filter has added.
    pub episodes: RecentSet,
    /// The items of the last successful fetch (not persisted).
    pub items: Vec<FeedItem>,
}

impl RssFeed {
    /// The alias, else the feed's title, else its URL.
    pub fn name(&self) -> &str {
        [&self.alias, &self.title].into_iter().find(|s| !s.trim().is_empty()).unwrap_or(&self.url)
    }

    /// The URL to fetch and the cookies to send with it.
    pub fn fetch_url(&self) -> (&str, &str) {
        split_cookie(&self.url)
    }
}

/// µTorrent's way of giving a feed cookies: `https://site/rss.php:COOKIE:uid=1;pass=abc`.
/// Returns the URL and the cookie string (empty without the suffix).
pub fn split_cookie(url: &str) -> (&str, &str) {
    const MARK: &str = ":COOKIE:";
    match url.as_bytes().windows(MARK.len()).position(|w| w.eq_ignore_ascii_case(MARK.as_bytes())) {
        Some(i) => (url[..i].trim(), url[i + MARK.len()..].trim()),
        None => (url.trim(), ""),
    }
}

/// The downloader-wide options (µTorrent's Advanced `rss.*` settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RssSettings {
    /// Minutes between refreshes of a feed without its own interval (`rss.update_interval`,
    /// 15 in µTorrent 2.2.1 as far as remembered).
    pub interval: u32,
    /// The smart episode filter lets a REPACK / PROPER of an added episode through once
    /// (`rss.smart_repack_filter`).
    pub smart_repack: bool,
    /// A filter without a label labels its downloads with the feed's name
    /// (`rss.feed_as_default_label`).
    pub feed_as_default_label: bool,
}

impl Default for RssSettings {
    fn default() -> RssSettings {
        RssSettings { interval: 15, smart_repack: true, feed_as_default_label: true }
    }
}

/// A torrent the downloader wants added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddRequest {
    pub feed: FeedId,
    /// The filter that matched; `None` for a subscription.
    pub filter: Option<FilterId>,
    /// The item's [`FeedItem::id`], for [`RssDownloader::forget`] if the add fails.
    pub item_id: String,
    pub title: String,
    /// A `.torrent` URL or a `magnet:` link.
    pub url: String,
    /// The feed's cookies, to send when fetching `url` (private trackers need them).
    pub cookies: String,
    /// Empty for the default download folder.
    pub save_path: String,
    /// Empty for none.
    pub label: String,
    pub top_priority: bool,
    pub paused: bool,
}

impl AddRequest {
    pub fn is_magnet(&self) -> bool {
        self.url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("magnet:"))
    }
}

/// One line of the History list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub title: String,
    pub url: String,
    pub feed: FeedId,
    /// Unix time.
    pub time: i64,
}

/// The RSS Downloader: feeds, filters and history.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RssDownloader {
    pub settings: RssSettings,
    pub feeds: Vec<RssFeed>,
    /// In priority order: an item goes to the first filter that takes it.
    pub filters: Vec<RssFilter>,
    /// Newest last.
    pub history: VecDeque<HistoryEntry>,
    next_id: u32,
}

impl RssDownloader {
    fn new_id(&mut self) -> u32 {
        let used = |id: u32| self.feeds.iter().any(|f| f.id == id) || self.filters.iter().any(|f| f.id == id);
        let mut id = self.next_id.max(1);
        while used(id) {
            id = id.wrapping_add(1).max(1);
        }
        self.next_id = id.wrapping_add(1);
        id
    }

    /// Adds an enabled feed refreshed at the default interval.
    pub fn add_feed(&mut self, url: &str, alias: &str) -> FeedId {
        let id = self.new_id();
        self.feeds.push(RssFeed {
            id,
            url: url.trim().to_owned(),
            alias: alias.trim().to_owned(),
            enabled: true,
            ..RssFeed::default()
        });
        id
    }

    /// Changes a feed's URL and alias (the Edit Feed dialog). A new URL is a new feed to
    /// fetch: the old items, title, error and refresh time go. False when there is no such
    /// feed.
    pub fn edit_feed(&mut self, id: FeedId, url: &str, alias: &str) -> bool {
        let Some(f) = self.feed_mut(id) else { return false };
        let url = url.trim();
        if f.url != url {
            f.url = url.to_owned();
            f.items.clear();
            f.title.clear();
            f.last_error.clear();
            f.last_refresh = None;
        }
        f.alias = alias.trim().to_owned();
        true
    }

    /// Removes a feed. Filters bound to it stay, and match nothing until pointed elsewhere.
    pub fn remove_feed(&mut self, id: FeedId) {
        self.feeds.retain(|f| f.id != id);
    }

    pub fn feed(&self, id: FeedId) -> Option<&RssFeed> {
        self.feeds.iter().find(|f| f.id == id)
    }

    pub fn feed_mut(&mut self, id: FeedId) -> Option<&mut RssFeed> {
        self.feeds.iter_mut().find(|f| f.id == id)
    }

    /// Adds a filter at the end of the list, with a fresh id.
    pub fn add_filter(&mut self, mut filter: RssFilter) -> FilterId {
        filter.id = self.new_id();
        let id = filter.id;
        self.filters.push(filter);
        id
    }

    pub fn remove_filter(&mut self, id: FilterId) {
        self.filters.retain(|f| f.id != id);
    }

    pub fn filter(&self, id: FilterId) -> Option<&RssFilter> {
        self.filters.iter().find(|f| f.id == id)
    }

    pub fn filter_mut(&mut self, id: FilterId) -> Option<&mut RssFilter> {
        self.filters.iter_mut().find(|f| f.id == id)
    }

    fn interval_secs(&self, feed: &RssFeed) -> i64 {
        i64::from(feed.interval.unwrap_or(self.settings.interval).max(1)) * 60
    }

    /// The enabled feeds whose refresh is due at `now` (never fetched, or their interval has
    /// passed since the last fetch).
    pub fn due_feeds(&self, now: i64) -> Vec<FeedId> {
        self.feeds
            .iter()
            .filter(|f| {
                f.enabled && f.last_refresh.is_none_or(|t| now >= t.saturating_add(self.interval_secs(f)))
            })
            .map(|f| f.id)
            .collect()
    }

    /// When the next refresh is due (for a frontend's timer); `None` without enabled feeds.
    pub fn next_due(&self) -> Option<i64> {
        self.feeds
            .iter()
            .filter(|f| f.enabled)
            .map(|f| f.last_refresh.map_or(i64::MIN, |t| t.saturating_add(self.interval_secs(f))))
            .min()
    }

    /// Takes the result of fetching feed `id` at `now`: remembers its items (or the error) and
    /// returns what to add.
    pub fn feed_fetched(&mut self, id: FeedId, now: i64, result: Result<Feed, String>) -> Vec<AddRequest> {
        let Some(feed) = self.feed_mut(id) else { return Vec::new() };
        feed.last_refresh = Some(now);
        match result {
            Ok(doc) => {
                feed.last_error.clear();
                feed.title = doc.title;
                feed.items = doc.items;
                self.process(id, now)
            }
            Err(e) => {
                feed.last_error = e;
                Vec::new()
            }
        }
    }

    /// Runs the subscription or the filters over feed `id`'s current items (after a fetch, or
    /// after the filters changed). Items already added are skipped; the oldest are considered
    /// first, so the smart episode filter and the minimum interval keep the first release.
    pub fn process(&mut self, id: FeedId, now: i64) -> Vec<AddRequest> {
        let Some(fi) = self.feeds.iter().position(|f| f.id == id) else { return Vec::new() };
        let settings = self.settings;
        let items = std::mem::take(&mut self.feeds[fi].items);
        let mut out = Vec::new();
        for i in oldest_first(&items) {
            let item = &items[i];
            let item_id = item.id();
            let feed = &self.feeds[fi];
            if item_id.is_empty() || feed.downloaded.contains(item_id) {
                continue;
            }
            let Some(url) = item.torrent_url() else { continue };
            let episode = parse_episode(&item.title);
            let repack = settings.smart_repack && is_repack(&item.title);
            let feed_label =
                if settings.feed_as_default_label { feed.name().to_owned() } else { String::new() };
            let mut req = AddRequest {
                feed: id,
                filter: None,
                item_id: item_id.to_owned(),
                title: item.title.clone(),
                url,
                cookies: feed.fetch_url().1.to_owned(),
                save_path: String::new(),
                label: feed_label,
                top_priority: false,
                paused: false,
            };
            if feed.subscribe {
                let feed = &mut self.feeds[fi];
                if feed.subscribe_smart && !smart_take(&mut feed.episodes, episode, repack) {
                    continue;
                }
            } else {
                let Some(filter) = self.filters.iter_mut().find(|f| {
                    f.enabled
                        && f.matches(id, item)
                        && f.last_match.is_none_or(|t| now.saturating_sub(t) >= i64::from(f.min_interval))
                        && (!f.smart || smart_allows(&f.seen_episodes, episode, repack))
                }) else {
                    continue;
                };
                if filter.smart {
                    let mut seen: RecentSet = filter.seen_episodes.iter().map(String::as_str).collect();
                    smart_take(&mut seen, episode, repack);
                    seen.prune(MAX_EPISODES, &HashSet::new());
                    filter.seen_episodes = seen.iter().map(str::to_owned).collect();
                }
                filter.last_match = Some(now);
                req.filter = Some(filter.id);
                req.save_path = filter.save_in.clone();
                if !filter.label.is_empty() {
                    req.label = filter.label.clone();
                }
                req.top_priority = filter.top_priority;
                req.paused = filter.paused;
            }
            self.feeds[fi].downloaded.insert(item_id);
            self.history.push_back(HistoryEntry {
                title: req.title.clone(),
                url: req.url.clone(),
                feed: id,
                time: now,
            });
            out.push(req);
        }
        while self.history.len() > MAX_HISTORY {
            self.history.pop_front();
        }
        let feed = &mut self.feeds[fi];
        let current: HashSet<&str> = items.iter().map(FeedItem::id).collect();
        for &s in &current {
            feed.seen.insert(s);
        }
        feed.seen.prune(MAX_FEED_HISTORY, &current);
        feed.downloaded.prune(MAX_FEED_HISTORY, &current);
        feed.episodes.prune(MAX_EPISODES, &HashSet::new());
        feed.items = items;
        out
    }

    /// The user's own download of an item (a double-click on it): the request to add it, with
    /// the feed's name as its label when `feed_as_default_label` is on. The item is marked
    /// added and goes into the History, as a filter's would. `None` when the feed or the item
    /// is gone, or the item names nothing to download.
    pub fn request_item(&mut self, feed: FeedId, item_id: &str, now: i64) -> Option<AddRequest> {
        let label_from_feed = self.settings.feed_as_default_label;
        let f = self.feed_mut(feed)?;
        let item = f.items.iter().find(|i| i.id() == item_id)?;
        let req = AddRequest {
            feed,
            filter: None,
            item_id: item_id.to_owned(),
            title: item.title.clone(),
            url: item.torrent_url()?,
            cookies: f.fetch_url().1.to_owned(),
            save_path: String::new(),
            label: if label_from_feed { f.name().to_owned() } else { String::new() },
            top_priority: false,
            paused: false,
        };
        f.downloaded.insert(item_id);
        self.history.push_back(HistoryEntry {
            title: req.title.clone(),
            url: req.url.clone(),
            feed,
            time: now,
        });
        while self.history.len() > MAX_HISTORY {
            self.history.pop_front();
        }
        Some(req)
    }

    /// Moves filter `id` one place up (`up`) or down the list, where order is priority.
    pub fn move_filter(&mut self, id: FilterId, up: bool) {
        let Some(i) = self.filters.iter().position(|f| f.id == id) else { return };
        let j = if up { i.checked_sub(1) } else { Some(i + 1).filter(|&j| j < self.filters.len()) };
        if let Some(j) = j {
            self.filters.swap(i, j);
        }
    }

    /// Forgets that an item was added (its download could not be fetched), so the next
    /// refresh tries it again. Episodes the smart filters recorded stay recorded.
    pub fn forget(&mut self, feed: FeedId, item_id: &str) {
        if let Some(f) = self.feed_mut(feed) {
            f.downloaded.remove(item_id);
        }
    }

    /// The items of the feeds' last fetches that `filter` matches on its fields (the dialog's
    /// preview), as `(feed, item)`.
    pub fn matching_items<'a>(
        &'a self,
        filter: &'a RssFilter,
    ) -> impl Iterator<Item = (FeedId, &'a FeedItem)> {
        self.feeds.iter().flat_map(move |f| {
            f.items.iter().filter(move |i| filter.matches(f.id, i)).map(move |i| (f.id, i))
        })
    }
}

/// Item indices, oldest first: by date when every item has one, else the reverse of document
/// order (feeds list the newest first).
fn oldest_first(items: &[FeedItem]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len()).rev().collect();
    if items.iter().all(|i| i.published.is_some()) {
        order.sort_by_key(|&i| items[i].published);
    }
    order
}

/// Whether the smart episode filter lets an item through: a title without an episode always
/// passes; otherwise one of its episodes must be new, or (`repack`) not yet re-added.
fn smart_allows(seen: &[String], episode: Option<Episode>, repack: bool) -> bool {
    let Some(ep) = episode else { return true };
    let keys = ep.keys();
    keys.iter().any(|k| !seen.contains(k))
        || (repack && keys.iter().any(|k| !seen.contains(&format!("r{k}"))))
}

/// [`smart_allows`] on a set, recording the item's episodes when it passes.
fn smart_take(seen: &mut RecentSet, episode: Option<Episode>, repack: bool) -> bool {
    let Some(ep) = episode else { return true };
    let keys = ep.keys();
    let new = keys.iter().any(|k| !seen.contains(k));
    let new_repack = repack && keys.iter().any(|k| !seen.contains(&format!("r{k}")));
    if !new && !new_repack {
        return false;
    }
    for k in &keys {
        seen.insert(k);
        if repack {
            seen.insert(&format!("r{k}"));
        }
    }
    true
}

/// Fetches and parses a feed (blocking: call it from a worker thread). The URL may carry
/// µTorrent's `:COOKIE:` suffix.
pub fn fetch_feed(url: &str, proxy: Option<FetchProxy>) -> Result<Feed, String> {
    let (url, cookies) = split_cookie(url);
    let opts = FetchOptions { cookies: cookies.to_owned(), proxy, ..FetchOptions::default() };
    let resp = http_get(url, &opts).map_err(|e| e.to_string())?;
    if resp.redirect().is_some() {
        return Err(format!("{url}: redirected away from the feed"));
    }
    parse_feed(&resp.body).ok_or_else(|| format!("{url}: not an RSS or Atom feed"))
}

#[cfg(test)]
mod tests;
