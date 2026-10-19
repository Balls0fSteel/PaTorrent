//! The RSS Downloader model: scheduling, subscriptions, filters, the smart episode filter,
//! persistence, and deterministic mutation runs over the feed parser and `rss.dat` loader.

use super::*;

const NOW: i64 = 1_800_000_000;

fn item(title: &str, published: Option<i64>) -> FeedItem {
    FeedItem {
        title: title.into(),
        guid: format!("guid:{title}"),
        link: format!("https://feeds.example.org/get/{}.torrent", title.replace(' ', "_")),
        published,
        ..FeedItem::default()
    }
}

fn doc(items: Vec<FeedItem>) -> Feed {
    Feed { title: "Example".into(), items, ..Feed::default() }
}

fn titles(reqs: &[AddRequest]) -> Vec<&str> {
    reqs.iter().map(|r| r.title.as_str()).collect()
}

#[test]
fn cookies_in_feed_urls() {
    assert_eq!(split_cookie("https://a.b/rss.php"), ("https://a.b/rss.php", ""));
    assert_eq!(
        split_cookie(" https://a.b/rss.php?x=1:COOKIE:uid=1;pass=abc "),
        ("https://a.b/rss.php?x=1", "uid=1;pass=abc")
    );
    assert_eq!(split_cookie("http://a.b/r:cookie:k=v"), ("http://a.b/r", "k=v"));
    let mut d = RssDownloader::default();
    let id = d.add_feed("https://a.b/rss:COOKIE:uid=7", "");
    assert_eq!(d.feed(id).unwrap().fetch_url(), ("https://a.b/rss", "uid=7"));
    assert_eq!(d.feed(id).unwrap().name(), "https://a.b/rss:COOKIE:uid=7");
}

#[test]
fn refresh_schedule() {
    let mut d = RssDownloader::default();
    assert_eq!(d.settings.interval, 15);
    assert_eq!(d.next_due(), None);
    let a = d.add_feed("https://a/rss", "A");
    let b = d.add_feed("https://b/rss", "B");
    let c = d.add_feed("https://c/rss", "C");
    d.feed_mut(b).unwrap().interval = Some(60);
    d.feed_mut(c).unwrap().enabled = false;
    assert_eq!(d.due_feeds(NOW), [a, b], "never fetched: due at once");
    assert_eq!(d.next_due(), Some(i64::MIN));
    d.feed_fetched(a, NOW, Ok(doc(vec![])));
    d.feed_fetched(b, NOW, Err("HTTP 503".into()));
    assert_eq!(d.feed(b).unwrap().last_error, "HTTP 503");
    assert!(d.due_feeds(NOW + 14 * 60).is_empty());
    assert_eq!(d.next_due(), Some(NOW + 15 * 60));
    assert_eq!(d.due_feeds(NOW + 15 * 60), [a]);
    assert_eq!(d.due_feeds(NOW + 60 * 60), [a, b], "a failed fetch waits its interval too");
    d.feed_fetched(b, NOW + 3600, Ok(doc(vec![])));
    assert!(d.feed(b).unwrap().last_error.is_empty());
    d.remove_feed(a);
    assert_eq!(d.next_due(), Some(NOW + 2 * 3600));
    assert!(d.feed_fetched(a, NOW, Ok(doc(vec![]))).is_empty(), "a removed feed is ignored");
}

#[test]
fn subscription_adds_everything_once() {
    let mut d = RssDownloader::default();
    let f = d.add_feed("https://a/rss:COOKIE:uid=1", "Distros");
    d.feed_mut(f).unwrap().subscribe = true;
    // newest first, as feeds list them; no dates: document order reversed
    let items = vec![item("Distro 2.0", None), item("Distro 1.0", None)];
    let reqs = d.feed_fetched(f, NOW, Ok(doc(items.clone())));
    assert_eq!(titles(&reqs), ["Distro 1.0", "Distro 2.0"]);
    let r = &reqs[0];
    assert_eq!((r.feed, r.filter, r.cookies.as_str(), r.label.as_str()), (f, None, "uid=1", "Distros"));
    assert_eq!(r.url, "https://feeds.example.org/get/Distro_1.0.torrent");
    assert!(!r.is_magnet() && !r.paused && !r.top_priority && r.save_path.is_empty());
    assert!(d.feed_fetched(f, NOW + 900, Ok(doc(items.clone()))).is_empty(), "nothing twice");
    let mut more = vec![item("Distro 3.0", None)];
    more.extend(items);
    assert_eq!(titles(&d.feed_fetched(f, NOW + 1800, Ok(doc(more)))), ["Distro 3.0"]);
    assert_eq!(d.history.len(), 3);
    assert_eq!(d.history.back().unwrap().title, "Distro 3.0");
}

#[test]
fn subscription_with_smart_filter() {
    let mut d = RssDownloader::default();
    d.settings.feed_as_default_label = false;
    let f = d.add_feed("https://a/rss", "");
    let feed = d.feed_mut(f).unwrap();
    feed.subscribe = true;
    feed.subscribe_smart = true;
    let items = vec![
        item("Show.S01E02.1080p", Some(NOW - 10)),
        item("Show.S01E02.720p", Some(NOW - 20)),
        item("Show.S01E01.720p", Some(NOW - 30)),
        item("Show.S01E01.720p.REPACK", Some(NOW - 5)),
        item("Unrelated special without episode", Some(NOW - 1)),
    ];
    let reqs = d.feed_fetched(f, NOW, Ok(doc(items)));
    assert_eq!(
        titles(&reqs),
        [
            "Show.S01E01.720p",
            "Show.S01E02.720p",
            "Show.S01E01.720p.REPACK",
            "Unrelated special without episode"
        ]
    );
    assert!(reqs[0].label.is_empty());
    assert_eq!(d.feed(f).unwrap().title, "Example");
}

#[test]
fn filters_pick_and_configure() {
    let mut d = RssDownloader::default();
    let tv = d.add_feed("https://tv/rss", "TV");
    let other = d.add_feed("https://other/rss", "");
    let first = d.add_filter(RssFilter {
        name: "Show 720p".into(),
        pattern: "*show*".into(),
        not: "*german*".into(),
        quality: Quality::P720,
        save_in: "D:\\TV\\Show".into(),
        label: "TV Shows".into(),
        top_priority: true,
        paused: true,
        feed: Some(tv),
        ..RssFilter::default()
    });
    let second =
        d.add_filter(RssFilter { name: "Anything else".into(), pattern: "*".into(), ..RssFilter::default() });
    let disabled = d.add_filter(RssFilter { pattern: "*".into(), enabled: false, ..RssFilter::default() });
    assert!(first != second && second != disabled && first != tv && first != other);
    let items = vec![
        item("Show.S01E01.720p.GERMAN", Some(NOW - 30)),
        item("Show.S01E01.720p", Some(NOW - 20)),
        item("Movie.2026.1080p", Some(NOW - 10)),
    ];
    let reqs = d.feed_fetched(tv, NOW, Ok(doc(items)));
    assert_eq!(titles(&reqs), ["Show.S01E01.720p.GERMAN", "Show.S01E01.720p", "Movie.2026.1080p"]);
    // the German release is not the first filter's, so the catch-all takes it
    assert_eq!(reqs[0].filter, Some(second));
    assert_eq!(reqs[0].label, "TV", "feed name as the default label");
    let r = &reqs[1];
    assert_eq!(r.filter, Some(first));
    assert_eq!(
        (r.save_path.as_str(), r.label.as_str(), r.top_priority, r.paused),
        ("D:\\TV\\Show", "TV Shows", true, true)
    );
    // the first filter is bound to the TV feed
    d.filter_mut(second).unwrap().enabled = false;
    assert!(d.feed_fetched(other, NOW, Ok(doc(vec![item("Show.S01E02.720p", None)]))).is_empty());
    assert_eq!(d.feed(other).unwrap().name(), "Example", "the channel title without an alias");
    // the dialog's preview
    let f = d.filter(first).unwrap().clone();
    let preview: Vec<_> = d.matching_items(&f).map(|(feed, i)| (feed, i.title.as_str())).collect();
    assert_eq!(preview, [(tv, "Show.S01E01.720p")]);
    // removing a filter
    d.remove_filter(first);
    assert!(d.filter(first).is_none());
}

#[test]
fn new_filters_apply_to_items_already_in_the_feed() {
    let mut d = RssDownloader::default();
    let f = d.add_feed("https://a/rss", "");
    assert!(d.feed_fetched(f, NOW, Ok(doc(vec![item("Show.S01E01", None)]))).is_empty());
    d.add_filter(RssFilter { pattern: "show".into(), ..RssFilter::default() });
    assert_eq!(titles(&d.process(f, NOW + 1)), ["Show.S01E01"]);
    assert!(d.process(f, NOW + 2).is_empty());
    // the add failed: forget it, and it comes again
    d.forget(f, "guid:Show.S01E01");
    assert_eq!(titles(&d.process(f, NOW + 3)), ["Show.S01E01"]);
    assert!(d.feed(f).unwrap().seen.contains("guid:Show.S01E01"));
}

#[test]
fn minimum_interval() {
    let mut d = RssDownloader::default();
    let f = d.add_feed("https://a/rss", "");
    let id = d.add_filter(RssFilter { pattern: "*".into(), min_interval: 2 * 3600, ..RssFilter::default() });
    let items = vec![item("b", Some(NOW - 10)), item("a", Some(NOW - 20))];
    assert_eq!(titles(&d.feed_fetched(f, NOW, Ok(doc(items.clone())))), ["a"]);
    assert_eq!(d.filter(id).unwrap().last_match, Some(NOW));
    assert!(d.process(f, NOW + 3600).is_empty());
    assert_eq!(titles(&d.process(f, NOW + 2 * 3600)), ["b"]);
}

#[test]
fn smart_episode_filter_and_repacks() {
    let mut d = RssDownloader::default();
    let f = d.add_feed("https://a/rss", "");
    let id = d.add_filter(RssFilter {
        pattern: "*show*".into(),
        smart: true,
        episodes: "S01E01-S01E03".into(),
        ..RssFilter::default()
    });
    let items = vec![
        item("Show.S01E04.720p", Some(NOW - 1)),
        item("Show.S01E02E03.1080p", Some(NOW - 2)),
        item("Show.S01E03.720p", Some(NOW - 3)),
        item("Show.S01E01.1080p", Some(NOW - 4)),
        item("Show.S01E01.720p", Some(NOW - 5)),
    ];
    let reqs = d.feed_fetched(f, NOW, Ok(doc(items)));
    // E04 is out of range; E02E03 still brings E02
    assert_eq!(titles(&reqs), ["Show.S01E01.720p", "Show.S01E03.720p", "Show.S01E02E03.1080p"]);
    assert_eq!(d.filter(id).unwrap().seen_episodes, ["s1e1", "s1e3", "s1e2"]);
    let later = vec![item("Show.S01E01.PROPER.720p", None), item("Show.S01E01.REPACK.720p", None)];
    assert_eq!(titles(&d.feed_fetched(f, NOW + 900, Ok(doc(later.clone())))), ["Show.S01E01.REPACK.720p"]);
    // without the repack rule, neither would come
    let mut d2 = d.clone();
    d2.settings.smart_repack = false;
    d2.filter_mut(id).unwrap().seen_episodes.retain(|k| !k.starts_with('r'));
    d2.feed_mut(f).unwrap().downloaded = RecentSet::default();
    assert!(d2.process(f, NOW + 1000).is_empty());
}

#[test]
fn items_without_a_download_are_skipped() {
    let mut d = RssDownloader::default();
    let f = d.add_feed("https://a/rss", "");
    d.feed_mut(f).unwrap().subscribe = true;
    let bare = FeedItem { title: "no link".into(), guid: "g".into(), ..FeedItem::default() };
    let magnet = FeedItem {
        title: "magnet".into(),
        magnet: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".into(),
        ..FeedItem::default()
    };
    let reqs = d.feed_fetched(f, NOW, Ok(doc(vec![bare, magnet, FeedItem::default()])));
    assert_eq!(titles(&reqs), ["magnet"]);
    assert!(reqs[0].is_magnet());
    assert_eq!(reqs[0].item_id, reqs[0].url, "no guid or link: the magnet identifies it");
}

#[test]
fn history_is_bounded_but_keeps_current_items() {
    let mut s: RecentSet = ["a", "b", "c", "d", "e"].into_iter().collect();
    s.insert("a");
    assert_eq!(s.len(), 5);
    s.prune(2, &["a", "b"].into_iter().collect());
    assert_eq!(s.iter().collect::<Vec<_>>(), ["a", "b"]);
    s.prune(1, &["a", "b"].into_iter().collect());
    assert_eq!(s.len(), 2, "never drops what the feed still lists");
    s.remove("a");
    assert!(!s.contains("a") && s.contains("b") && !s.is_empty());

    let mut d = RssDownloader::default();
    let f = d.add_feed("https://a/rss", "");
    d.feed_mut(f).unwrap().subscribe = true;
    let many: Vec<FeedItem> = (0..MAX_HISTORY + 10).map(|i| item(&format!("t{i}"), None)).collect();
    assert_eq!(d.feed_fetched(f, NOW, Ok(doc(many))).len(), MAX_HISTORY + 10);
    assert_eq!(d.history.len(), MAX_HISTORY);
    let feed = d.feed(f).unwrap();
    assert_eq!(feed.downloaded.len(), MAX_HISTORY + 10);
    // once those items leave the feed, the memory shrinks to the cap
    let fresh: Vec<FeedItem> = (0..MAX_FEED_HISTORY + 500).map(|i| item(&format!("n{i}"), None)).collect();
    d.feed_fetched(f, NOW + 1, Ok(doc(fresh)));
    let feed = d.feed(f).unwrap();
    assert_eq!(feed.downloaded.len(), MAX_FEED_HISTORY + 500);
    d.feed_fetched(f, NOW + 2, Ok(doc(vec![])));
    assert_eq!(d.feed(f).unwrap().downloaded.len(), MAX_FEED_HISTORY);
}

fn sample() -> RssDownloader {
    let mut d = RssDownloader {
        settings: RssSettings { interval: 30, smart_repack: false, feed_as_default_label: false },
        ..RssDownloader::default()
    };
    let a = d.add_feed("https://a/rss:COOKIE:uid=1", "Alias A");
    let b = d.add_feed("https://b/atom", "");
    let fb = d.feed_mut(b).unwrap();
    fb.enabled = false;
    fb.interval = Some(120);
    fb.subscribe = true;
    fb.subscribe_smart = true;
    d.add_filter(RssFilter {
        name: "Show".into(),
        pattern: "*show*|*other*".into(),
        not: "*sample*".into(),
        save_in: "C:\\Downloads\\TV".into(),
        feed: Some(a),
        quality: Quality::P720.union(Quality::HDTV),
        episodes: "S01E01-S01E10".into(),
        label: "TV".into(),
        min_interval: 7200,
        smart: true,
        top_priority: true,
        paused: true,
        ..RssFilter::default()
    });
    d.add_filter(RssFilter { name: "Off".into(), enabled: false, ..RssFilter::default() });
    d.feed_fetched(a, NOW, Ok(doc(vec![item("Show.S01E02.720p.HDTV", None), item("other thing", None)])));
    d.feed_fetched(b, NOW, Err("timed out".into()));
    d
}

#[test]
fn persistence_round_trip() {
    let d = sample();
    assert_eq!(d.history.len(), 1);
    let bytes = d.to_bencode();
    let back = RssDownloader::from_bencode(&bytes).unwrap();
    // fetched items are not persisted; everything else is
    let mut want = d.clone();
    for f in &mut want.feeds {
        f.items.clear();
    }
    assert_eq!(back, want);
    assert_eq!(back.to_bencode(), bytes, "canonical");
    // ids keep counting from where they were
    let mut back = back;
    let id = back.add_feed("https://c/rss", "");
    assert!(d.feeds.iter().all(|f| f.id != id) && d.filters.iter().all(|f| f.id != id));

    let dir = std::env::temp_dir().join(format!("patorrent-rss-test-{}", std::process::id()));
    let path = dir.join(store::FILE_NAME);
    d.save(&path).unwrap();
    assert_eq!(RssDownloader::load(&path), want);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(RssDownloader::load(&path), RssDownloader::default(), "missing file");
}

#[test]
fn loading_damaged_files() {
    assert_eq!(RssDownloader::from_bencode(b""), None);
    assert_eq!(RssDownloader::from_bencode(b"not bencode"), None);
    assert_eq!(RssDownloader::from_bencode(b"li1ee"), None, "not a dictionary");
    let d = RssDownloader::from_bencode(b"de").unwrap();
    assert_eq!(d, RssDownloader { next_id: 1, ..RssDownloader::default() });
    // wrong types, id 0, duplicate ids and a filter id that clashes with a feed are skipped
    let raw = b"d5:feedsl\
        d2:idi0e3:url1:xe\
        d2:idi5e3:url5:a/rsse\
        d2:idi5e3:url5:dupe!e\
        i3e\
        d2:id3:abce\
        e7:filtersl\
        d2:idi5e4:name5:clashe\
        d2:idi6e7:enabledi0e7:qualityi-1e8:episodes3:S02e\
        e7:historyl1:xd5:title1:te\
        e8:settingsd8:intervali0e12:smart_repacki0eee";
    let d = RssDownloader::from_bencode(raw).unwrap();
    assert_eq!(d.feeds.len(), 1);
    assert_eq!((d.feeds[0].id, d.feeds[0].url.as_str(), d.feeds[0].enabled), (5, "a/rss", true));
    assert_eq!(d.filters.len(), 1);
    let f = &d.filters[0];
    assert_eq!((f.id, f.enabled, f.quality, f.episodes.as_str()), (6, false, Quality::ALL, "S02"));
    assert_eq!(d.history.len(), 2);
    assert_eq!((d.settings.interval, d.settings.smart_repack), (15, false));
}

// --- Deterministic mutation runs (cargo-fuzz's `rss` target does this coverage-guided) ------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

fn mutate(rng: &mut Rng, input: &[u8], alphabet: &[u8]) -> Vec<u8> {
    let mut b = input.to_vec();
    for _ in 0..1 + rng.below(6) {
        if b.is_empty() {
            b.push(b'<');
        }
        let i = rng.below(b.len());
        match rng.below(7) {
            0 => b[i] ^= 1 << rng.below(8),
            1 => b[i] = alphabet[rng.below(alphabet.len())],
            2 => b.truncate(i),
            3 => {
                let j = rng.below(b.len());
                let chunk: Vec<u8> = b[i.min(j)..i.max(j)].to_vec();
                let at = rng.below(b.len());
                b.splice(at..at, chunk);
            }
            4 => {
                let j = (i + rng.below(64)).min(b.len());
                b.drain(i..j);
            }
            5 => b.insert(i, alphabet[rng.below(alphabet.len())]),
            _ => {
                b.remove(i);
            }
        }
    }
    b
}

const RSS_SAMPLE: &str = r#"<?xml version="1.0"?><rss version="2.0" xmlns:torrent="http://xmlns.ezrss.it/0.1/"><channel><title>T &amp; U</title>
<item><title>Show.S01E02.720p.HDTV</title><link>https://x/1</link><guid>g1</guid><pubDate>Mon, 19 Oct 2026 06:35:37 +0000</pubDate>
<description><![CDATA[<b>x</b> &amp;]]></description><enclosure url="https://x/1.torrent?a=1&amp;b=2" type="application/x-bittorrent" length="12"/>
<torrent:magnetURI><![CDATA[magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567]]></torrent:magnetURI>
<torrent:infoHash>0123456789abcdef0123456789abcdef01234567</torrent:infoHash><torrent:contentLength>99</torrent:contentLength></item>
<item><title>[G] Anime - 12 [1080p]</title><link>magnet:?xt=urn:btih:abc</link><pubDate>2026-10-19T06:35:37Z</pubDate></item>
</channel></rss>"#;

const ATOM_SAMPLE: &str = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>A</title><entry><title type="html">E &amp;amp; F 2026.10.19</title>
<link rel="enclosure" type="application/x-bittorrent" href="https://y/1.torrent"/><link href="https://y/1"/><id>urn:1</id>
<updated>2026-10-19T08:00:00Z</updated><published>2026-10-18T22:30:15+02:00</published><summary>s &lt;b&gt;</summary></entry></feed>"#;

#[test]
fn mutated_feeds_never_panic() {
    let rounds: usize = std::env::var("LT_FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(1500);
    let seed: u64 =
        std::env::var("LT_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x5eed_0ff3_3dc0_de01);
    let mut rng = Rng(seed | 1);
    let alphabet = b"<>/=\"'&;#![]CDATA-: xitemrsslinkentry0123456789SsEex.";
    let mut d = RssDownloader::default();
    let feed = d.add_feed("https://x/rss", "");
    d.add_filter(RssFilter {
        pattern: "*".into(),
        smart: true,
        episodes: "S01-S03, 1-100".into(),
        ..RssFilter::default()
    });
    let mut parsed = 0usize;
    for round in 0..rounds {
        for sample in [RSS_SAMPLE, ATOM_SAMPLE] {
            let m = mutate(&mut rng, sample.as_bytes(), alphabet);
            let Some(f) = parse_feed(&m) else { continue };
            parsed += 1;
            assert!(f.items.len() <= feed::MAX_ITEMS);
            for i in &f.items {
                let _ = (
                    i.id(),
                    i.torrent_url(),
                    parse_episode(&i.title),
                    Quality::of_title(&i.title),
                    is_repack(&i.title),
                );
                let _ = EpisodeSpec::parse(&i.title);
                let _ = wildcard_match(&i.title, &i.description);
            }
            let _ = d.feed_fetched(feed, NOW + round as i64, Ok(f));
        }
    }
    assert!(parsed > rounds / 2, "only {parsed} mutated feeds parsed");
    // and the state those runs built survives its file format
    let bytes = d.to_bencode();
    let mut want = d.clone();
    for f in &mut want.feeds {
        f.items.clear();
    }
    assert_eq!(RssDownloader::from_bencode(&bytes).unwrap(), want);
}

#[test]
fn mutated_state_files_never_panic() {
    let original = sample().to_bencode();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut loaded = 0;
    for _ in 0..3000 {
        let m = mutate(&mut rng, &original, b"deil0123456789:-");
        if let Some(d) = RssDownloader::from_bencode(&m) {
            loaded += 1;
            let _ = RssDownloader::from_bencode(&d.to_bencode()).unwrap();
            let _ = d.due_feeds(NOW);
        }
    }
    assert!(loaded > 100, "only {loaded} mutated files loaded");
}

#[test]
fn dates_and_titles_never_panic() {
    let mut rng = Rng(77);
    let seeds = [
        "Mon, 19 Oct 2026 06:35:37 +0000",
        "2026-10-19T06:35:37.5-02:30",
        "S01E02-E03 1x02 2026.10.19 - 12v2",
    ];
    for _ in 0..5000 {
        let s = seeds[rng.below(seeds.len())];
        let m = mutate(&mut rng, s.as_bytes(), b"0123456789 :+-TZSEex.,v");
        let t = String::from_utf8_lossy(&m);
        let _ = parse_date(&t);
        let _ = parse_episode(&t);
        let _ = EpisodeSpec::parse(&t).map(|s| s.matches(parse_episode(&t)));
    }
}

/// A loopback feed server: `fetch_feed` sends the `:COOKIE:` part as a Cookie header and parses
/// what comes back.
#[test]
fn fetch_feed_over_http() {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut heads = Vec::new();
        for body in [RSS_SAMPLE, "<html>no feed</html>"] {
            let (mut s, _) = l.accept().unwrap();
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && s.read(&mut b).unwrap() == 1 {
                head.push(b[0]);
            }
            heads.push(String::from_utf8(head).unwrap());
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            s.write_all(resp.as_bytes()).unwrap();
        }
        heads
    });
    let f = fetch_feed(&format!("http://{addr}/rss.xml:COOKIE:uid=1;pass=2"), None).unwrap();
    assert_eq!((f.title.as_str(), f.items.len()), ("T & U", 2));
    let e = fetch_feed(&format!("http://{addr}/page"), None).unwrap_err();
    assert!(e.ends_with("not an RSS or Atom feed"), "{e}");
    let heads = server.join().unwrap();
    assert!(heads[0].starts_with("GET /rss.xml HTTP/1.1\r\n"), "{}", heads[0]);
    assert!(heads[0].contains("\r\nCookie: uid=1;pass=2\r\n"), "{}", heads[0]);
}

/// Manual check of a real feed: `LT_RSS_FILE=feed.xml cargo test -p patorrent-frontend
/// rss::tests::local_feed_file -- --ignored --nocapture` prints what the parser and the
/// episode / quality detection make of it.
#[test]
#[ignore = "manual"]
fn local_feed_file() {
    let Some(path) = std::env::var_os("LT_RSS_FILE") else {
        eprintln!("skipped: set LT_RSS_FILE to a saved feed");
        return;
    };
    let f = parse_feed(&std::fs::read(path).unwrap()).expect("a feed");
    eprintln!("{:?} {:?}: {} items", f.kind, f.title, f.items.len());
    for i in &f.items {
        let ep = parse_episode(&i.title);
        let q = Quality::of_title(&i.title).names();
        eprintln!("{:?} {:?} {:?} [{q}] {:?} {:?}", i.title, i.published, ep, i.torrent_url(), i.info_hash);
    }
}
