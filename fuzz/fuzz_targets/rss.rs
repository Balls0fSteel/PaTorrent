//! RSS and Atom feeds as a feed server would send them, through the parser, the episode,
//! quality and date readers and the RSS Downloader's matching; and the same bytes as an
//! `rss.dat` state file; and as a Torznab search answer, error document and caps (D41).
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_frontend::rss::{
    EpisodeSpec, FeedItem, Quality, RssDownloader, RssFilter, is_repack, parse_date, parse_episode,
    parse_feed, wildcard_match,
};
use patorrent_frontend::search;

fuzz_target!(|data: &[u8]| {
    if let Ok(results) = search::parse_results(data) {
        let mut merged = Vec::new();
        search::merge(&mut merged, results.clone());
        search::merge(&mut merged, results);
        for r in &merged {
            let _ = r.magnet_link();
        }
    }
    let _ = search::parse_caps(data);
    let text = String::from_utf8_lossy(data);
    let _ = search::torznab::search_url(&text, &text, &text, &[2000]);
    let _ = search::redact(&text, "key");
    if let Some(d) = RssDownloader::from_bencode(data) {
        let _ = RssDownloader::from_bencode(&d.to_bencode()).expect("a saved state loads");
        let _ = d.due_feeds(0);
    }
    let Some(feed) = parse_feed(data) else {
        let text = String::from_utf8_lossy(data);
        let _ = parse_date(&text);
        let _ = parse_episode(&text);
        let _ = EpisodeSpec::parse(&text);
        return;
    };
    for item in &feed.items {
        let _ = (item.id(), item.torrent_url(), parse_date(&item.title), is_repack(&item.title));
        let _ = Quality::of_title(&item.title).names();
        let ep = parse_episode(&item.title);
        if let Some(spec) = EpisodeSpec::parse(&item.description) {
            let _ = spec.matches(ep);
        }
        // the item's own text as a filter pattern: wildcards in titles are common junk
        let _ = wildcard_match(&item.title, &item.description);
    }
    let mut d = RssDownloader::default();
    let id = d.add_feed("https://feed.invalid/rss", "");
    d.add_filter(RssFilter {
        pattern: "*".into(),
        smart: true,
        episodes: "S01-S05, 1-500, 2000.01.01-".into(),
        ..RssFilter::default()
    });
    let n = feed.items.len();
    let reqs = d.feed_fetched(id, 1_800_000_000, Ok(feed));
    assert!(reqs.len() <= n);
    for r in &reqs {
        assert!(!r.url.is_empty() && !r.item_id.is_empty());
    }
    let _ = FeedItem::default().torrent_url();
});
