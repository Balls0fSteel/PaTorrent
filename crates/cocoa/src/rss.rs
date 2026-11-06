//! The RSS Downloader and the sidebar's feeds.
//!
//! Slice H fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

/// Not yet built (slice H).
pub fn open() {
    crate::app::not_yet("RSS Downloader");
}

/// The sidebar's Feeds rows: each feed's id and name (slice C shows them under the Feeds
/// row, which lists every feed's items). Empty until slice H fills it (`Core::feeds`).
/// Not yet built (slice H).
pub fn sidebar_rows() -> Vec<(rookparcel_frontend::rss::FeedId, String)> {
    Vec::new()
}

/// The sidebar picked the Feeds row (`None`: every feed's items) or a feed: the list shows its
/// items (`Core::show_feed`, the item columns).
/// Not yet built (slice H).
pub fn show_feed(feed: Option<rookparcel_frontend::rss::FeedId>) {
    let _ = feed;
    crate::app::not_yet("feed items in the list");
}
