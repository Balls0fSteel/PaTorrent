//! The RSS Downloader's network side, shared by the frontends: feeds and `.torrent` files are
//! fetched on worker threads (one per feed refresh, one per batch of downloads), and
//! [`Worker::collect`] hands the results to the [`RssDownloader`] on the frontend's own thread
//! and returns the torrents to add. The worker starts threads and calls the frontend's
//! `notify` when one finishes; it never touches the model from another thread, so the
//! frontend decides when to look (the Win32 window on a posted message, nox on its status
//! tick).
//!
//! A download that fails is forgotten ([`RssDownloader::forget`]), so the item is tried again
//! at the next refresh; one the engine refuses (a duplicate, a broken file) stays added.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use patorrent_core::fetch::{FetchOptions, FetchProxy, http_get};
use patorrent_core::metainfo::{TorrentInfo, parse_magnet_uri};
use patorrent_core::{AddTorrentParams, InfoHashes, TorrentFlags, TorrentSource};

use super::{AddRequest, Feed, FeedId, RssDownloader, fetch_feed};
use crate::tr;

/// What an item's URL gave: a `.torrent` file, or a magnet link (named by the item, or where
/// the download redirected, as indexers do).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Download {
    Torrent(Vec<u8>),
    Magnet(String),
}

impl Download {
    /// The torrent's info-hashes, when the file parses or the link names them.
    pub fn info_hashes(&self) -> Option<InfoHashes> {
        match self {
            Download::Torrent(b) => TorrentInfo::from_bytes(b).ok().map(|ti| ti.info_hashes()),
            Download::Magnet(m) => parse_magnet_uri(m).ok().map(|m| m.info_hashes),
        }
    }

    pub fn into_source(self) -> TorrentSource {
        match self {
            Download::Torrent(b) => TorrentSource::TorrentFile(b),
            Download::Magnet(m) => TorrentSource::Magnet(m),
        }
    }
}

impl AddRequest {
    /// Fills in what the request decides, after the frontend's defaults: the filter's Save in
    /// and "Add in paused state" (µTorrent's Stop: paused and outside the queue). The label and
    /// "highest priority" (top of the queue once added) are the frontend's to apply.
    pub fn apply_to(&self, p: &mut AddTorrentParams) {
        if !self.save_path.trim().is_empty() {
            p.save_path = self.save_path.trim().to_owned();
        }
        if self.paused {
            p.flags = p.flags & !TorrentFlags::AUTO_MANAGED | TorrentFlags::PAUSED;
        }
    }
}

/// Fetches what an item names (blocking: call it from a worker thread): a magnet link as it
/// is, else the `.torrent` behind the URL, sent with the feed's cookies.
pub fn fetch_download(url: &str, cookies: &str, proxy: Option<FetchProxy>) -> Result<Download, String> {
    let url = url.trim();
    if url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("magnet:")) {
        return Ok(Download::Magnet(url.to_owned()));
    }
    let opts = FetchOptions { cookies: cookies.to_owned(), proxy, ..FetchOptions::default() };
    let resp = http_get(url, &opts).map_err(|e| e.to_string())?;
    match resp.redirect() {
        Some(m) if m.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("magnet:")) => {
            Ok(Download::Magnet(m.to_owned()))
        }
        Some(other) => Err(tr!(RssErrRedirect, url = other)),
        None if resp.body.first() == Some(&b'd') => Ok(Download::Torrent(resp.body)),
        // an HTML page (a login form, a "download will start" page) is no torrent
        None => Err(tr!(RssErrNotTorrent, url)),
    }
}

/// How the worker fetches; the network by default, a fake in tests.
pub trait Fetcher: Send + Sync {
    fn feed(&self, url: &str, proxy: Option<FetchProxy>) -> Result<Feed, String>;
    fn download(&self, url: &str, cookies: &str, proxy: Option<FetchProxy>) -> Result<Download, String>;
}

/// [`fetch_feed`] and [`fetch_download`].
pub struct Net;

impl Fetcher for Net {
    fn feed(&self, url: &str, proxy: Option<FetchProxy>) -> Result<Feed, String> {
        fetch_feed(url, proxy)
    }

    fn download(&self, url: &str, cookies: &str, proxy: Option<FetchProxy>) -> Result<Download, String> {
        fetch_download(url, cookies, proxy)
    }
}

enum Done {
    Feed(FeedId, Result<Feed, String>),
    Download(AddRequest, Result<Download, String>),
    /// A download batch is through.
    Batch,
}

/// What [`Worker::collect`] found.
#[derive(Debug, Default)]
pub struct Collected {
    /// The torrents to add now, with the requests that asked for them.
    pub adds: Vec<(AddRequest, Download)>,
    /// Lines for the Logger.
    pub log: Vec<String>,
    /// The downloader changed (a feed fetched, an item forgotten): save `rss.dat`, refresh
    /// what shows it.
    pub changed: bool,
}

/// Starts the fetches and collects their results; see the module documentation.
pub struct Worker {
    tx: Sender<Done>,
    rx: Receiver<Done>,
    notify: Arc<dyn Fn() + Send + Sync>,
    fetcher: Arc<dyn Fetcher>,
    /// Feeds being fetched.
    busy: HashSet<FeedId>,
    /// Download batches under way.
    batches: usize,
}

impl Worker {
    /// A worker on the network; `notify` runs on the worker thread after each result.
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Worker {
        Worker::with_fetcher(Arc::new(Net), notify)
    }

    pub fn with_fetcher(fetcher: Arc<dyn Fetcher>, notify: impl Fn() + Send + Sync + 'static) -> Worker {
        let (tx, rx) = channel();
        Worker { tx, rx, notify: Arc::new(notify), fetcher, busy: HashSet::new(), batches: 0 }
    }

    /// Whether feed `id` is being fetched.
    pub fn is_busy(&self, id: FeedId) -> bool {
        self.busy.contains(&id)
    }

    /// Whether anything is under way.
    pub fn is_idle(&self) -> bool {
        self.busy.is_empty() && self.batches == 0
    }

    /// Starts fetching the feeds due at `now` that are not being fetched already; returns how
    /// many it started.
    pub fn refresh_due(&mut self, rss: &RssDownloader, now: i64, proxy: Option<&FetchProxy>) -> usize {
        rss.due_feeds(now).into_iter().filter(|&id| self.refresh(rss, id, proxy)).count()
    }

    /// Starts fetching feed `id` now (the dialog's Update); false when it is being fetched
    /// already or does not exist.
    pub fn refresh(&mut self, rss: &RssDownloader, id: FeedId, proxy: Option<&FetchProxy>) -> bool {
        let Some(feed) = rss.feed(id) else { return false };
        if !self.busy.insert(id) {
            return false;
        }
        let url = feed.url.clone();
        let (tx, notify, fetcher, proxy) =
            (self.tx.clone(), self.notify.clone(), self.fetcher.clone(), proxy.cloned());
        std::thread::spawn(move || {
            let r = fetcher.feed(&url, proxy);
            if tx.send(Done::Feed(id, r)).is_ok() {
                notify();
            }
        });
        true
    }

    /// Starts the downloads `reqs` ask for. Magnet links need no fetching: they come back at
    /// once, to add now.
    pub fn download(
        &mut self,
        reqs: Vec<AddRequest>,
        proxy: Option<&FetchProxy>,
    ) -> Vec<(AddRequest, Download)> {
        let (magnets, files): (Vec<AddRequest>, Vec<AddRequest>) =
            reqs.into_iter().partition(AddRequest::is_magnet);
        if !files.is_empty() {
            self.batches += 1;
            let (tx, notify, fetcher, proxy) =
                (self.tx.clone(), self.notify.clone(), self.fetcher.clone(), proxy.cloned());
            std::thread::spawn(move || {
                for req in files {
                    let r = fetcher.download(&req.url, &req.cookies, proxy.clone());
                    if tx.send(Done::Download(req, r)).is_err() {
                        return;
                    }
                    notify();
                }
                if tx.send(Done::Batch).is_ok() {
                    notify();
                }
            });
        }
        magnets
            .into_iter()
            .map(|r| {
                let m = Download::Magnet(r.url.trim().to_owned());
                (r, m)
            })
            .collect()
    }

    /// Takes the finished fetches into `rss` at `now`: a fetched feed runs its subscription or
    /// the filters and starts the downloads they want; a finished download comes back to add,
    /// a failed one is forgotten so the next refresh tries it again.
    pub fn collect(&mut self, rss: &mut RssDownloader, now: i64, proxy: Option<&FetchProxy>) -> Collected {
        let mut out = Collected::default();
        while let Ok(done) = self.rx.try_recv() {
            match done {
                Done::Batch => self.batches = self.batches.saturating_sub(1),
                Done::Feed(id, result) => {
                    self.busy.remove(&id);
                    if rss.feed(id).is_none() {
                        continue; // removed while it was being fetched
                    }
                    if let Err(e) = &result {
                        let name = rss.feed(id).map(|f| f.name().to_owned()).unwrap_or_default();
                        out.log.push(tr!(RssLogError, name, error = e));
                    }
                    let reqs = rss.feed_fetched(id, now, result);
                    out.changed = true;
                    let name = rss.feed(id).map(|f| f.name().to_owned()).unwrap_or_default();
                    for r in &reqs {
                        out.log.push(tr!(RssLogDownloading, name, title = r.title));
                    }
                    out.adds.extend(self.download(reqs, proxy));
                }
                Done::Download(req, Ok(d)) => out.adds.push((req, d)),
                Done::Download(req, Err(e)) => {
                    out.log.push(tr!(RssLogDownloadFailed, title = req.title, error = e));
                    rss.forget(req.feed, &req.item_id);
                    out.changed = true;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::super::{FeedItem, RssFilter};
    use super::*;

    const NOW: i64 = 1_800_000_000;

    /// Feeds and downloads from tables; a URL missing from them fails.
    #[derive(Default)]
    struct Fake {
        feeds: Mutex<Vec<(String, Feed)>>,
        files: Mutex<Vec<(String, Vec<u8>)>>,
        cookies: Mutex<Vec<String>>,
    }

    impl Fetcher for Fake {
        fn feed(&self, url: &str, _: Option<FetchProxy>) -> Result<Feed, String> {
            let url = super::super::split_cookie(url).0;
            let feeds = self.feeds.lock().unwrap();
            feeds.iter().find(|(u, _)| u == url).map(|(_, f)| f.clone()).ok_or_else(|| "HTTP 404".into())
        }

        fn download(&self, url: &str, cookies: &str, _: Option<FetchProxy>) -> Result<Download, String> {
            self.cookies.lock().unwrap().push(cookies.to_owned());
            let files = self.files.lock().unwrap();
            files
                .iter()
                .find(|(u, _)| u == url)
                .map(|(_, b)| Download::Torrent(b.clone()))
                .ok_or_else(|| "HTTP 404".into())
        }
    }

    fn item(title: &str, url: &str) -> FeedItem {
        FeedItem { title: title.into(), guid: title.into(), link: url.into(), ..FeedItem::default() }
    }

    /// Collects until `want` adds came back (or a second passed).
    fn collect_until(w: &mut Worker, d: &mut RssDownloader, want: usize) -> Collected {
        let mut all = Collected::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let c = w.collect(d, NOW, None);
            all.adds.extend(c.adds);
            all.log.extend(c.log);
            all.changed |= c.changed;
            if all.adds.len() >= want && w.is_idle() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        all
    }

    #[test]
    fn a_refresh_adds_what_the_filters_take_and_retries_failed_downloads() {
        let fake = Arc::new(Fake::default());
        fake.feeds.lock().unwrap().push((
            "https://f/rss".into(),
            Feed {
                title: "Shows".into(),
                items: vec![
                    item("Show.S01E01.720p", "https://f/1.torrent"),
                    item("Show.S01E02.720p", "https://f/2.torrent"),
                    item("Show.S01E03.720p", "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"),
                    item("Other.S01E01", "https://f/other.torrent"),
                ],
                ..Feed::default()
            },
        ));
        fake.files.lock().unwrap().push(("https://f/1.torrent".into(), b"d4:infod4:name1:aee".to_vec()));
        let notified = Arc::new(AtomicUsize::new(0));
        let n = notified.clone();
        let mut w = Worker::with_fetcher(fake.clone(), move || {
            n.fetch_add(1, Ordering::SeqCst);
        });
        let mut d = RssDownloader::default();
        let id = d.add_feed("https://f/rss:COOKIE:uid=5", "");
        d.add_filter(RssFilter { pattern: "show".into(), ..RssFilter::default() });
        assert_eq!(w.refresh_due(&d, NOW, None), 1);
        assert!(w.is_busy(id));
        assert_eq!(w.refresh_due(&d, NOW, None), 0, "a feed being fetched is not started twice");
        assert!(!w.refresh(&d, id, None));
        let c = collect_until(&mut w, &mut d, 2);
        assert!(c.changed);
        assert!(!w.is_busy(id) && w.is_idle());
        assert!(notified.load(Ordering::SeqCst) >= 2);
        let mut titles: Vec<&str> = c.adds.iter().map(|(r, _)| r.title.as_str()).collect();
        titles.sort_unstable();
        assert_eq!(titles, ["Show.S01E01.720p", "Show.S01E03.720p"], "the magnet at once, file 1 fetched");
        assert!(c.adds.iter().any(|(_, d)| matches!(d, Download::Magnet(_))));
        assert!(
            c.log.iter().any(|l| l.contains("could not download Show.S01E02.720p: HTTP 404")),
            "{:?}",
            c.log
        );
        assert!(fake.cookies.lock().unwrap().iter().all(|c| c == "uid=5"), "the feed's cookies go along");
        assert_eq!(d.feed(id).unwrap().title, "Shows");
        // the failed one is forgotten: the next refresh takes it again
        assert!(!d.feed(id).unwrap().downloaded.contains("Show.S01E02.720p"));
        assert!(d.feed(id).unwrap().downloaded.contains("Show.S01E01.720p"));
        fake.files.lock().unwrap().push(("https://f/2.torrent".into(), b"d4:infod4:name1:bee".to_vec()));
        assert!(w.refresh(&d, id, None));
        let c = collect_until(&mut w, &mut d, 1);
        let titles: Vec<&str> = c.adds.iter().map(|(r, _)| r.title.as_str()).collect();
        assert_eq!(titles, ["Show.S01E02.720p"]);
    }

    #[test]
    fn a_failed_feed_is_logged_and_kept() {
        let fake = Arc::new(Fake::default());
        let mut w = Worker::with_fetcher(fake, || {});
        let mut d = RssDownloader::default();
        let id = d.add_feed("https://nowhere/rss", "Nowhere");
        w.refresh(&d, id, None);
        let c = collect_until(&mut w, &mut d, 0);
        assert!(c.changed && c.adds.is_empty());
        assert_eq!(c.log, ["RSS: Nowhere: HTTP 404"]);
        assert_eq!(d.feed(id).unwrap().last_error, "HTTP 404");
        assert_eq!(d.feed(id).unwrap().last_refresh, Some(NOW));
    }

    #[test]
    fn requests_apply_save_path_and_paused() {
        let req = AddRequest {
            feed: 1,
            filter: Some(2),
            item_id: "x".into(),
            title: "x".into(),
            url: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".into(),
            cookies: String::new(),
            save_path: " D:\\tv ".into(),
            label: "tv".into(),
            top_priority: true,
            paused: true,
        };
        let mut p = AddTorrentParams::new(TorrentSource::Magnet(req.url.clone()), "C:\\dl");
        p.flags = TorrentFlags::DEFAULT;
        req.apply_to(&mut p);
        assert_eq!(p.save_path, "D:\\tv");
        assert!(p.flags.contains(TorrentFlags::PAUSED) && !p.flags.contains(TorrentFlags::AUTO_MANAGED));
        let plain = AddRequest { save_path: String::new(), paused: false, ..req.clone() };
        let mut q = AddTorrentParams::new(TorrentSource::Magnet(req.url.clone()), "C:\\dl");
        q.flags = TorrentFlags::DEFAULT;
        plain.apply_to(&mut q);
        assert_eq!((q.save_path.as_str(), q.flags), ("C:\\dl", TorrentFlags::DEFAULT));
        let m = Download::Magnet(req.url.clone());
        assert_eq!(m.info_hashes().unwrap().key_hex(), "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(Download::Torrent(b"junk".to_vec()).info_hashes(), None);
        assert_eq!(fetch_download(" MAGNET:?xt=x ", "", None), Ok(Download::Magnet("MAGNET:?xt=x".into())));
    }
}
