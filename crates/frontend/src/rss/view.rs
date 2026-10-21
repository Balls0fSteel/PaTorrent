//! What the RSS Downloader's dialogs and the feed item list show, as text, so the Win32
//! window and the Web UI say the same: the item list's columns (µTorrent 2.2.1 listed a
//! feed's items in the main list when the feed was picked in the category tree), the
//! History, a feed's status line, the Minimum interval names and the Episode column.

use super::episode::{Episode, Quality, parse_episode};
use super::{FeedId, MIN_INTERVALS, RssDownloader, RssFeed};
use crate::detail::ColumnDef;
use crate::lang::{S, text};
use crate::{fmt, tr};

/// The feed item list's columns: title, width at 96 DPI, right-aligned (as remembered:
/// µTorrent also had a Codec column, which the model does not detect).
pub const ITEM_COLUMNS: [ColumnDef; 6] = [
    (S::ColName, 320, false),
    (S::ColEpisode, 70, false),
    (S::ColFormat, 80, false),
    (S::ColDate, 130, false),
    (S::ColFeed, 130, false),
    (S::ColUrl, 300, false),
];

/// The History list's columns.
pub const HISTORY_COLUMNS: [ColumnDef; 3] =
    [(S::ColName, 300, false), (S::ColDate, 130, false), (S::ColFeed, 130, false)];

/// One row of the feed item list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRow {
    pub feed: FeedId,
    /// The item's [`super::FeedItem::id`].
    pub id: String,
    /// What a download fetches (`None`: the item names nothing).
    pub url: Option<String>,
    /// The item was added already (by a filter, the subscription or the user).
    pub downloaded: bool,
    /// In [`ITEM_COLUMNS`]' order.
    pub cells: [String; 6],
}

/// The items of feed `feed` (`None`: every feed, one after the other), each feed's in the
/// order of its document (newest first, usually).
pub fn item_rows(rss: &RssDownloader, feed: Option<FeedId>) -> Vec<ItemRow> {
    let mut out = Vec::new();
    for f in rss.feeds.iter().filter(|f| feed.is_none_or(|id| id == f.id)) {
        for item in &f.items {
            let url = item.torrent_url();
            out.push(ItemRow {
                feed: f.id,
                id: item.id().to_owned(),
                downloaded: f.downloaded.contains(item.id()),
                cells: [
                    item.title.clone(),
                    episode_text(parse_episode(&item.title)),
                    Quality::of_title(&item.title).names(),
                    item.published.map(fmt::date_time).unwrap_or_default(),
                    f.name().to_owned(),
                    url.clone().unwrap_or_default(),
                ],
                url,
            });
        }
    }
    out
}

/// The History, newest first: Name, Date, Feed (a feed removed since shows empty).
pub fn history_rows(rss: &RssDownloader) -> Vec<[String; 3]> {
    rss.history
        .iter()
        .rev()
        .map(|h| {
            let feed = rss.feed(h.feed).map(|f| f.name().to_owned()).unwrap_or_default();
            [h.title.clone(), fmt::date_time(h.time), feed]
        })
        .collect()
}

/// The Episode column: `1x02`, `1x02-03`, an absolute `12`, an air date `2026-10-19`.
pub fn episode_text(ep: Option<Episode>) -> String {
    match ep {
        None => String::new(),
        Some(Episode::Numbered { season: 0, first, last }) if first == last => first.to_string(),
        Some(Episode::Numbered { season: 0, first, last }) => format!("{first}-{last}"),
        Some(Episode::Numbered { season, first, last }) if first == last => format!("{season}x{first:02}"),
        Some(Episode::Numbered { season, first, last }) => format!("{season}x{first:02}-{last:02}"),
        Some(Episode::Dated(d)) => format!("{:04}-{:02}-{:02}", d / 10_000, d / 100 % 100, d % 100),
    }
}

/// A feed's state for the Feeds list's tooltip and the Web UI: updating, the last error, or
/// when it was last updated.
pub fn feed_status(f: &RssFeed, updating: bool) -> String {
    match (updating, f.last_refresh) {
        (true, _) => tr!(Updating).to_owned(),
        _ if !f.enabled => tr!(RssDisabled).to_owned(),
        _ if !f.last_error.is_empty() => tr!(RssError, error = f.last_error),
        (false, Some(t)) => tr!(RssUpdated, time = fmt::date_time(t), count = f.items.len()),
        (false, None) => tr!(RssNotUpdated).to_owned(),
    }
}

/// The name of a Minimum interval, or `N seconds` for one outside the list.
pub fn interval_text(secs: u32) -> String {
    MIN_INTERVALS
        .iter()
        .find(|(s, _)| *s == secs)
        .map_or_else(|| tr!(RssSeconds, secs), |(_, n)| text(*n).to_owned())
}

/// The Add RSS Feed (and Edit Feed) dialog's fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedForm {
    /// With µTorrent's optional `:COOKIE:` suffix.
    pub url: String,
    pub alias: String,
    /// "Automatically download all items published in feed".
    pub subscribe: bool,
    /// "Use smart episode filter" (with `subscribe`).
    pub smart: bool,
}

impl FeedForm {
    pub fn of(f: &RssFeed) -> FeedForm {
        FeedForm {
            url: f.url.clone(),
            alias: f.alias.clone(),
            subscribe: f.subscribe,
            smart: f.subscribe_smart,
        }
    }

    /// Adds the feed; its id.
    pub fn add_to(&self, d: &mut RssDownloader) -> FeedId {
        let id = d.add_feed(&self.url, &self.alias);
        self.store(d, id);
        id
    }

    /// Stores the form in feed `id`; whether its URL changed (it is fetched again).
    pub fn store(&self, d: &mut RssDownloader, id: FeedId) -> bool {
        let old = d.feed(id).map(|f| f.url.clone());
        d.edit_feed(id, &self.url, &self.alias);
        if let Some(f) = d.feed_mut(id) {
            f.subscribe = self.subscribe;
            f.subscribe_smart = self.subscribe && self.smart;
        }
        old.is_some_and(|u| u != self.url.trim())
    }
}

/// `text` trimmed when it looks like a feed URL (µTorrent filled the Add RSS Feed dialog from
/// the clipboard with one).
pub fn feed_url(text: &str) -> Option<String> {
    let s = text.trim();
    ((s.starts_with("http://") || s.starts_with("https://")) && !s.contains(char::is_whitespace))
        .then(|| s.to_owned())
}

/// The Quality field's text: "All" or the chosen names.
pub fn quality_text(q: Quality) -> String {
    if q.is_all() { tr!(RssQualityAll).to_owned() } else { q.names() }
}

#[cfg(test)]
mod tests {
    use super::super::{Feed, FeedItem};
    use super::*;

    #[test]
    fn rows_and_texts() {
        let mut d = RssDownloader::default();
        let a = d.add_feed("https://a/rss", "A");
        let b = d.add_feed("https://b/rss", "");
        let item = |t: &str| FeedItem {
            title: t.into(),
            guid: t.into(),
            link: format!("https://x/{t}.torrent"),
            published: Some(1_800_000_000),
            ..FeedItem::default()
        };
        d.feed_fetched(a, 1, Ok(Feed { items: vec![item("Show.S01E02.720p.HDTV")], ..Feed::default() }));
        d.feed_fetched(
            b,
            1,
            Ok(Feed {
                title: "Bee".into(),
                items: vec![item("Other.2026.10.19"), FeedItem::default()],
                ..Feed::default()
            }),
        );
        let rows = item_rows(&d, None);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0].cells[..3],
            ["Show.S01E02.720p.HDTV".to_owned(), "1x02".into(), "HDTV|720p".into()]
        );
        assert_eq!(rows[0].cells[4], "A");
        assert_eq!(rows[1].cells[1], "2026-10-19");
        assert_eq!(rows[1].cells[4], "Bee", "the feed's own title without an alias");
        assert_eq!(rows[2].url, None);
        assert!(!rows[0].downloaded);
        assert_eq!(item_rows(&d, Some(b)).len(), 2);
        let req = d.request_item(a, "Show.S01E02.720p.HDTV", 5).unwrap();
        assert_eq!((req.label.as_str(), req.url.as_str()), ("A", "https://x/Show.S01E02.720p.HDTV.torrent"));
        assert!(item_rows(&d, Some(a))[0].downloaded);
        assert_eq!(history_rows(&d)[0][0], "Show.S01E02.720p.HDTV");
        assert_eq!(history_rows(&d)[0][2], "A");
        assert!(d.request_item(a, "nope", 5).is_none());
        assert!(d.request_item(b, "", 5).is_none(), "an item without anything to download");

        assert_eq!(episode_text(Some(Episode::Numbered { season: 0, first: 12, last: 13 })), "12-13");
        assert_eq!(episode_text(Some(Episode::Numbered { season: 2, first: 3, last: 4 })), "2x03-04");
        assert_eq!(interval_text(86_400), "1 day");
        assert_eq!(interval_text(61), "61 s");
        assert_eq!(quality_text(Quality::ALL), "All");
        assert!(feed_status(d.feed(a).unwrap(), false).starts_with("Updated "));
        assert_eq!(feed_status(d.feed(a).unwrap(), true), "Updating...");
    }

    #[test]
    fn the_feed_form_adds_and_edits() {
        let mut d = RssDownloader::default();
        let form =
            FeedForm { url: " https://a/rss ".into(), alias: "A".into(), subscribe: true, smart: true };
        let id = form.add_to(&mut d);
        let f = d.feed(id).unwrap();
        assert_eq!(
            (f.url.as_str(), f.alias.as_str(), f.subscribe, f.subscribe_smart),
            ("https://a/rss", "A", true, true)
        );
        assert_eq!(FeedForm::of(f).url, "https://a/rss");
        let off = FeedForm { subscribe: false, ..FeedForm::of(f) };
        assert!(!off.store(&mut d, id), "the same URL");
        assert!(!d.feed(id).unwrap().subscribe_smart, "smart only with the subscription");
        let moved = FeedForm { url: "https://b/rss".into(), ..off };
        assert!(moved.store(&mut d, id));
        assert_eq!(feed_url(" https://x/feed?a=1 \n"), Some("https://x/feed?a=1".into()));
        assert_eq!(feed_url("ftp://x"), None);
        assert_eq!(feed_url("https://x/a b"), None);
    }

    #[test]
    fn filters_move() {
        let mut d = RssDownloader::default();
        let x = d.add_filter(super::super::RssFilter { name: "x".into(), ..Default::default() });
        let y = d.add_filter(super::super::RssFilter { name: "y".into(), ..Default::default() });
        d.move_filter(y, true);
        assert_eq!(d.filters.iter().map(|f| f.id).collect::<Vec<_>>(), [y, x]);
        d.move_filter(y, true);
        d.move_filter(x, false);
        assert_eq!(d.filters.iter().map(|f| f.id).collect::<Vec<_>>(), [y, x]);
    }
}
