//! A merged search over the enabled sources (D41), shared by the frontends: one worker thread
//! per source sends the query, and [`Search::collect`] merges the answers on the frontend's own
//! thread as they come ([`merge`]: de-duplicated by info-hash, else by title and size, sorted
//! by seeders). As with the RSS worker ([`crate::rss::worker`]) the threads never touch the
//! model; they call `notify` after each answer and the frontend decides when to look.
//!
//! Each search has an id: [`Search::start`] and [`Search::stop`] change it, and an answer for
//! an older id is dropped, so a slow source of a search the user replaced or stopped never
//! shows. (A request under way is not interrupted; it ends within its 30 s timeout.)
//!
//! Network rules (SEC-26 and the owner's proxy rule, 2026-10-30): the query goes to the
//! source's URL wherever it is (`Reach::User`: it is the user's); the engine's proxy is used
//! only for a source on a public address ([`proxy_for`]), so a Prowlarr on this machine or
//! network still answers with a proxy set. A result's link is followed with `Reach::User`
//! only when it is on the source's own host, else public addresses only ([`download_reach`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use patorrent_core::fetch::{FetchOptions, FetchProxy, Reach, http_get};

use super::source::{Source, url_origin};
use super::torznab::{Category, SearchError, SearchResult, caps_url, parse_caps, parse_results, search_url};
use crate::rss::worker::Download;
use crate::tr;

/// What to search for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    /// Newznab category numbers; empty for all.
    pub categories: Vec<u32>,
}

/// The engine's proxy for a request to `url`: none when the host is on this machine or network
/// (loopback, private, link-local, `localhost`), the proxy otherwise. Resolves the host
/// (blocking); a name that does not resolve counts as public.
pub fn proxy_for(url: &str, proxy: Option<&FetchProxy>) -> Option<FetchProxy> {
    let proxy = proxy?;
    (Reach::for_links_from(url) == Reach::Public).then(|| proxy.clone())
}

/// Which addresses a result's link may reach: any (`Reach::User`) when it is on the source's
/// own host (Prowlarr and Jackett hand out downloads through themselves), public ones only
/// otherwise.
pub fn download_reach(link: &str, source_url: &str) -> Reach {
    // the same scheme, host and port: another port on the source's machine (a local Web UI,
    // a router page) gets public addresses only, like any other link
    match (url_origin(link), url_origin(source_url)) {
        (Some(a), Some(b)) if a == b => Reach::User,
        _ => Reach::Public,
    }
}

/// Fetches a source's answer to `query` (blocking: call it from a worker thread).
pub fn fetch_results(
    source: &Source,
    query: &Query,
    proxy: Option<&FetchProxy>,
) -> Result<Vec<SearchResult>, SearchError> {
    let url = search_url(&source.url, &source.api_key, &query.text, &query.categories);
    fetch_torznab(source, &url, proxy, parse_results)
}

/// Fetches a source's categories (`t=caps`, its Test; blocking).
pub fn fetch_caps(source: &Source, proxy: Option<&FetchProxy>) -> Result<Vec<Category>, SearchError> {
    let url = caps_url(&source.url, &source.api_key);
    fetch_torznab(source, &url, proxy, parse_caps)
}

fn fetch_torznab<T>(
    source: &Source,
    url: &str,
    proxy: Option<&FetchProxy>,
    parse: fn(&[u8]) -> Result<T, SearchError>,
) -> Result<T, SearchError> {
    let opts =
        FetchOptions { proxy: proxy_for(&source.url, proxy), reach: Reach::User, ..FetchOptions::default() };
    let resp = http_get(url, &opts).map_err(|e| SearchError::Fetch(source.redact(&e.to_string())))?;
    if resp.redirect().is_some() {
        return Err(SearchError::Fetch(tr!(RssErrFeedRedirect, url = source.redact(url))));
    }
    parse(&resp.body).map_err(|e| match e {
        SearchError::NotTorznab(_) => SearchError::NotTorznab(source.redact(url)),
        e => e,
    })
}

/// What adding `result` (from `source`) adds (blocking): its magnet as it is, else the
/// `.torrent` behind its link, or the magnet that link redirects to, else a magnet made from
/// its info-hash. Errors have the key removed.
pub fn fetch_download(
    result: &SearchResult,
    source: &Source,
    proxy: Option<&FetchProxy>,
) -> Result<Download, String> {
    if !result.magnet.is_empty() {
        return Ok(Download::Magnet(result.magnet.clone()));
    }
    let link = result.link.trim();
    if link.is_empty() {
        return result.magnet_link().map(Download::Magnet).ok_or_else(|| tr!(RssErrNotTorrent, url = ""));
    }
    let opts = FetchOptions {
        proxy: proxy_for(link, proxy),
        reach: download_reach(link, &source.url),
        ..FetchOptions::default()
    };
    let resp = http_get(link, &opts).map_err(|e| source.redact(&e.to_string()))?;
    match resp.redirect() {
        Some(m) if m.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("magnet:")) => {
            Ok(Download::Magnet(m.to_owned()))
        }
        Some(other) => Err(tr!(RssErrRedirect, url = source.redact(other))),
        None if resp.body.first() == Some(&b'd') => Ok(Download::Torrent(resp.body)),
        None => Err(tr!(RssErrNotTorrent, url = source.redact(link))),
    }
}

/// How a search fetches; the network by default, a fake in tests.
pub trait Fetcher: Send + Sync {
    fn search(
        &self,
        source: &Source,
        query: &Query,
        proxy: Option<&FetchProxy>,
    ) -> Result<Vec<SearchResult>, SearchError>;
}

/// [`fetch_results`].
pub struct Net;

impl Fetcher for Net {
    fn search(
        &self,
        source: &Source,
        query: &Query,
        proxy: Option<&FetchProxy>,
    ) -> Result<Vec<SearchResult>, SearchError> {
        fetch_results(source, query, proxy)
    }
}

/// Where a source of the current search is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceStatus {
    Searching,
    /// Answered with this many results (before merging).
    Done(usize),
    /// Failed; the message has the key removed.
    Failed(String),
    /// The search was stopped before it answered.
    Stopped,
}

/// A source of the current search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchSource {
    /// A copy of the source (its key included: needed to add a result; never show or send it).
    pub source: Source,
    pub status: SourceStatus,
}

struct Answer {
    search: u64,
    source: usize,
    result: Result<Vec<SearchResult>, SearchError>,
}

/// One search at a time over the enabled sources; see the module documentation.
pub struct Search {
    tx: Sender<Answer>,
    rx: Receiver<Answer>,
    notify: Arc<dyn Fn() + Send + Sync>,
    fetcher: Arc<dyn Fetcher>,
    id: u64,
    query: Query,
    sources: Vec<SearchSource>,
    results: Vec<SearchResult>,
}

impl Search {
    /// A search on the network; `notify` runs on a worker thread after each answer.
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Search {
        Search::with_fetcher(Arc::new(Net), notify)
    }

    pub fn with_fetcher(fetcher: Arc<dyn Fetcher>, notify: impl Fn() + Send + Sync + 'static) -> Search {
        let (tx, rx) = channel();
        Search {
            tx,
            rx,
            notify: Arc::new(notify),
            fetcher,
            id: 0,
            query: Query::default(),
            sources: Vec::new(),
            results: Vec::new(),
        }
    }

    /// Starts searching the enabled `sources` for `query`, dropping the previous search and its
    /// results; returns the new search's id. With no enabled source nothing starts.
    pub fn start(&mut self, sources: &[Source], query: Query, proxy: Option<&FetchProxy>) -> u64 {
        self.id += 1;
        self.results.clear();
        self.sources = sources
            .iter()
            .filter(|s| s.enabled && !s.url.trim().is_empty())
            .map(|s| SearchSource { source: s.clone(), status: SourceStatus::Searching })
            .collect();
        self.query = query;
        for (i, s) in self.sources.iter().enumerate() {
            let (tx, notify, fetcher, proxy) =
                (self.tx.clone(), self.notify.clone(), self.fetcher.clone(), proxy.cloned());
            let (source, query, search) = (s.source.clone(), self.query.clone(), self.id);
            std::thread::spawn(move || {
                let result = fetcher.search(&source, &query, proxy.as_ref());
                let result = result.map_err(|e| match e {
                    SearchError::Fetch(m) => SearchError::Fetch(source.redact(&m)),
                    SearchError::NotTorznab(u) => SearchError::NotTorznab(source.redact(&u)),
                    SearchError::Torznab { code, description } => {
                        SearchError::Torznab { code, description: source.redact(&description) }
                    }
                });
                if tx.send(Answer { search, source: i, result }).is_ok() {
                    notify();
                }
            });
        }
        self.id
    }

    /// Stops the search: answers still to come are dropped, the results so far stay.
    pub fn stop(&mut self) {
        self.id += 1;
        for s in &mut self.sources {
            if s.status == SourceStatus::Searching {
                s.status = SourceStatus::Stopped;
            }
        }
    }

    /// Takes the answers that came in; true when the results or a source's status changed.
    pub fn collect(&mut self) -> bool {
        let mut changed = false;
        while let Ok(a) = self.rx.try_recv() {
            if a.search != self.id {
                continue; // a replaced or stopped search
            }
            let Some(s) = self.sources.get_mut(a.source) else { continue };
            changed = true;
            match a.result {
                Ok(mut found) => {
                    s.status = SourceStatus::Done(found.len());
                    for r in &mut found {
                        r.source = a.source;
                        r.sources = vec![a.source];
                    }
                    merge(&mut self.results, found);
                }
                Err(e) => s.status = SourceStatus::Failed(e.to_string()),
            }
        }
        changed
    }

    /// The current search's id (0 before the first).
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn query(&self) -> &Query {
        &self.query
    }

    /// Whether a source has yet to answer.
    pub fn is_running(&self) -> bool {
        self.sources.iter().any(|s| s.status == SourceStatus::Searching)
    }

    /// The sources searched, in the order given to [`Search::start`] (enabled ones only);
    /// [`SearchResult::source`] indexes this.
    pub fn sources(&self) -> &[SearchSource] {
        &self.sources
    }

    /// The merged results so far, most seeders first.
    pub fn results(&self) -> &[SearchResult] {
        &self.results
    }

    /// The source a result came from.
    pub fn source_of(&self, r: &SearchResult) -> Option<&Source> {
        self.sources.get(r.source).map(|s| &s.source)
    }
}

/// What makes two results the same torrent: the info-hash, else the title (any case) and size.
fn dedup_key(r: &SearchResult) -> (String, Option<u64>) {
    if r.info_hash.is_empty() {
        (r.name.to_lowercase(), r.size)
    } else {
        (r.info_hash.clone(), Some(u64::MAX))
    }
}

/// Adds `found` to `results`: a torrent already there keeps the entry with the most seeders and
/// remembers every source; then everything is sorted by seeders, most first (unknown last).
pub fn merge(results: &mut Vec<SearchResult>, found: Vec<SearchResult>) {
    let mut index: HashMap<(String, Option<u64>), usize> =
        results.iter().enumerate().map(|(i, r)| (dedup_key(r), i)).collect();
    for r in found {
        let key = dedup_key(&r);
        match index.get(&key) {
            Some(&i) => {
                let old = &mut results[i];
                let prev = std::mem::take(&mut old.sources);
                if r.seeders.map_or(-1, i64::from) > old.seeders.map_or(-1, i64::from) {
                    let prev_magnet = std::mem::take(&mut old.magnet);
                    *old = r;
                    old.sources = union(&old.sources, prev);
                    if old.magnet.is_empty() {
                        old.magnet = prev_magnet;
                    }
                } else {
                    old.sources = union(&prev, r.sources);
                    // a magnet the kept entry lacks is still useful
                    if old.magnet.is_empty() {
                        old.magnet = r.magnet;
                    }
                }
            }
            None => {
                index.insert(key, results.len());
                results.push(r);
            }
        }
    }
    results.sort_by(|a, b| b.seeders.map_or(-1, i64::from).cmp(&a.seeders.map_or(-1, i64::from)));
}

/// Both lists' sources once each, in source order: the column reads the same whichever source
/// answered first (the searches run in parallel).
fn union(a: &[usize], b: Vec<usize>) -> Vec<usize> {
    let mut all: Vec<usize> = a.iter().copied().chain(b).collect();
    all.sort_unstable();
    all.dedup();
    all
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use patorrent_core::{ProxyType, Settings};

    use super::*;

    fn res(name: &str, hash: &str, size: u64, seeders: Option<u32>) -> SearchResult {
        SearchResult {
            name: name.into(),
            info_hash: hash.into(),
            size: Some(size),
            seeders,
            link: format!("https://idx.example.org/{name}"),
            ..SearchResult::default()
        }
    }

    fn from(mut r: SearchResult, source: usize) -> SearchResult {
        r.source = source;
        r.sources = vec![source];
        r
    }

    #[test]
    fn merging_dedups_and_sorts_by_seeders() {
        let h = "114aac2345b3a0a9dffdc4a5d0922c69200b45f6";
        let mut all = Vec::new();
        merge(
            &mut all,
            vec![
                from(res("Linux ISO", h, 10, Some(5)), 0),
                from(res("Other", "", 7, None), 0),
                from(res("Show S01", "", 99, Some(8)), 0),
            ],
        );
        let mut jackett = from(res("linux iso (renamed)", h, 11, Some(50)), 1);
        jackett.magnet = "magnet:?xt=urn:btih:114aac2345b3a0a9dffdc4a5d0922c69200b45f6".into();
        merge(
            &mut all,
            vec![
                jackett,
                from(res("SHOW s01", "", 99, Some(3)), 1),
                from(res("Show S01", "", 100, Some(1)), 1),
                from(res("Other", "", 7, Some(0)), 1),
            ],
        );
        let names: Vec<(&str, Option<u32>, &[usize])> =
            all.iter().map(|r| (r.name.as_str(), r.seeders, r.sources.as_slice())).collect();
        assert_eq!(
            names,
            [
                ("linux iso (renamed)", Some(50), &[0, 1][..]),
                ("Show S01", Some(8), &[0, 1][..]),
                ("Show S01", Some(1), &[1][..]),
                ("Other", Some(0), &[0, 1][..]),
            ]
        );
        assert_eq!(all[0].source, 1, "the entry with the most seeders is kept");
        assert!(!all[0].magnet.is_empty());
        assert_eq!(all[1].source, 0);
    }

    #[test]
    fn the_proxy_rule() {
        let s = Settings {
            proxy_type: ProxyType::Socks5,
            proxy_hostname: "proxy.example.org".into(),
            proxy_port: 1080,
            ..Settings::default()
        };
        let p = FetchProxy::from_settings(&s).unwrap();
        for local in [
            "http://localhost:9696/1/api",
            "http://prowlarr.localhost/1/api",
            "http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab/",
            "http://192.168.1.20:9696/1/api",
            "http://10.0.0.5/api",
            "http://[::1]:9696/1/api",
            "http://[fe80::1]/api",
        ] {
            assert_eq!(proxy_for(local, Some(&p)), None, "{local}");
        }
        assert_eq!(proxy_for("https://1.1.1.1/api", Some(&p)), Some(p.clone()));
        assert_eq!(proxy_for("https://[2606:4700:4700::1111]/api", Some(&p)), Some(p.clone()));
        assert_eq!(proxy_for("https://1.1.1.1/api", None), None);
        // download links: any address only on the source's own host
        assert_eq!(
            download_reach("http://LOCALHOST:9696/5/download?x", "http://localhost:9696/5/api"),
            Reach::User
        );
        assert_eq!(
            download_reach("http://192.168.1.9/x.torrent", "http://localhost:9696/5/api"),
            Reach::Public
        );
        assert_eq!(
            download_reach("https://tracker.example.org/t", "https://idx.example.org/api"),
            Reach::Public
        );
        assert_eq!(download_reach("not a url", "http://localhost/"), Reach::Public);
        // another port or scheme on the source's own machine is not the source
        assert_eq!(
            download_reach("http://localhost:8080/api/v2/app/shutdown", "http://localhost:9696/5/api"),
            Reach::Public
        );
        assert_eq!(download_reach("https://localhost:9696/x", "http://localhost:9696/5/api"), Reach::Public);
        assert_eq!(download_reach("http://[::1]:9117/dl/x", "http://[::1]:9117/api/v2.0/x"), Reach::User);
        assert_eq!(download_reach("http://jackett/dl/x", "http://jackett:80/api"), Reach::User);
    }

    #[test]
    fn a_magnet_is_added_without_fetching() {
        let src = Source { url: "http://localhost:9696/1/api".into(), ..Source::default() };
        let mut r = res("x", "114aac2345b3a0a9dffdc4a5d0922c69200b45f6", 1, None);
        r.magnet = "magnet:?xt=urn:btih:114aac2345b3a0a9dffdc4a5d0922c69200b45f6".into();
        assert_eq!(fetch_download(&r, &src, None), Ok(Download::Magnet(r.magnet.clone())));
        r.magnet.clear();
        r.link.clear();
        assert_eq!(
            fetch_download(&r, &src, None),
            Ok(Download::Magnet("magnet:?xt=urn:btih:114aac2345b3a0a9dffdc4a5d0922c69200b45f6&dn=x".into()))
        );
        r.info_hash.clear();
        assert!(fetch_download(&r, &src, None).is_err());
        // an unsupported URL fails without the key in the message
        r.link = "ftp://localhost/?apikey=SECRETKEY".into();
        let e = fetch_download(&r, &Source { api_key: "SECRETKEY".into(), ..src }, None).unwrap_err();
        assert!(!e.contains("SECRETKEY"), "{e}");
    }

    type Answered = Result<Vec<SearchResult>, SearchError>;

    /// Answers from a table by source URL, after a delay per URL; a missing URL fails.
    #[derive(Default)]
    struct Fake {
        answers: Mutex<Vec<(String, Duration, Answered)>>,
        queries: Mutex<Vec<String>>,
    }

    impl Fetcher for Fake {
        fn search(
            &self,
            source: &Source,
            query: &Query,
            _: Option<&FetchProxy>,
        ) -> Result<Vec<SearchResult>, SearchError> {
            self.queries.lock().unwrap().push(format!("{}|{}", source.url, query.text));
            let found = self.answers.lock().unwrap().iter().find(|(u, ..)| *u == source.url).cloned();
            match found {
                Some((_, delay, r)) => {
                    std::thread::sleep(delay);
                    r
                }
                None => {
                    Err(SearchError::Fetch(format!("HTTP 401 for {}?apikey={}", source.url, source.api_key)))
                }
            }
        }
    }

    fn wait(s: &mut Search, until: impl Fn(&Search) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            s.collect();
            if until(s) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out: {:?}", s.sources());
    }

    fn source(url: &str, key: &str, enabled: bool) -> Source {
        Source { name: url.into(), url: url.into(), api_key: key.into(), enabled }
    }

    #[test]
    fn sources_are_searched_in_parallel_and_a_failure_is_one_source_s() {
        let h = "114aac2345b3a0a9dffdc4a5d0922c69200b45f6";
        let fake = Arc::new(Fake::default());
        {
            let mut a = fake.answers.lock().unwrap();
            a.push((
                "http://a/api".into(),
                Duration::from_millis(50),
                Ok(vec![res("One", h, 1, Some(3)), res("Two", "", 2, Some(9))]),
            ));
            a.push(("http://b/api".into(), Duration::from_millis(50), Ok(vec![res("one", h, 1, Some(4))])));
            a.push((
                "http://e/api".into(),
                Duration::ZERO,
                Err(SearchError::Torznab { code: 100, description: "bad key KEY-E".into() }),
            ));
        }
        let notified = Arc::new(AtomicUsize::new(0));
        let n = notified.clone();
        let mut s = Search::with_fetcher(fake.clone(), move || {
            n.fetch_add(1, Ordering::SeqCst);
        });
        let sources = [
            source("http://a/api", "", true),
            source("http://off/api", "", false),
            source("http://b/api", "", true),
            source("http://missing/api", "KEY-M", true),
            source("http://e/api", "KEY-E", true),
        ];
        let started = Instant::now();
        let id = s.start(&sources, Query { text: "linux".into(), categories: vec![] }, None);
        assert_eq!(id, 1);
        assert!(s.is_running());
        assert_eq!(s.sources().len(), 4, "the disabled source is not searched");
        wait(&mut s, |s| !s.is_running());
        assert!(started.elapsed() < Duration::from_millis(1000), "in parallel");
        assert!(notified.load(Ordering::SeqCst) >= 4);
        let st: Vec<&SourceStatus> = s.sources().iter().map(|x| &x.status).collect();
        assert_eq!(st[0], &SourceStatus::Done(2));
        assert_eq!(st[1], &SourceStatus::Done(1));
        assert_eq!(st[2], &SourceStatus::Failed("HTTP 401 for http://missing/api?apikey=***".into()));
        assert_eq!(st[3], &SourceStatus::Failed("100: bad key ***".into()));
        let r: Vec<(&str, Option<u32>, usize)> =
            s.results().iter().map(|r| (r.name.as_str(), r.seeders, r.source)).collect();
        assert_eq!(r, [("Two", Some(9), 0), ("one", Some(4), 1)]);
        assert_eq!(s.results()[1].sources, [0, 1], "in source order, whichever answered first");
        assert_eq!(s.source_of(&s.results()[1]).unwrap().url, "http://b/api");
        assert_eq!(s.query().text, "linux");
        assert!(!s.collect(), "nothing new");
    }

    #[test]
    fn a_replaced_or_stopped_search_s_answers_are_dropped() {
        let fake = Arc::new(Fake::default());
        {
            let mut a = fake.answers.lock().unwrap();
            a.push((
                "http://slow/api".into(),
                Duration::from_millis(150),
                Ok(vec![res("Old", "", 1, Some(1))]),
            ));
            a.push(("http://fast/api".into(), Duration::ZERO, Ok(vec![res("New", "", 1, Some(1))])));
        }
        let mut s = Search::with_fetcher(fake.clone(), || {});
        s.start(
            &[source("http://slow/api", "", true)],
            Query { text: "old".into(), ..Query::default() },
            None,
        );
        let id = s.start(
            &[source("http://fast/api", "", true)],
            Query { text: "new".into(), ..Query::default() },
            None,
        );
        assert_eq!(id, 2);
        wait(&mut s, |s| !s.is_running());
        std::thread::sleep(Duration::from_millis(250));
        s.collect();
        let names: Vec<&str> = s.results().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["New"], "the first search's late answer is dropped");

        // stopped: the slow answer never shows, the status says stopped
        s.start(&[source("http://slow/api", "", true)], Query::default(), None);
        assert!(s.results().is_empty());
        s.stop();
        assert!(!s.is_running());
        assert_eq!(s.sources()[0].status, SourceStatus::Stopped);
        std::thread::sleep(Duration::from_millis(250));
        assert!(!s.collect());
        assert!(s.results().is_empty());
        // nothing enabled: nothing starts
        s.start(&[source("http://fast/api", "", false)], Query::default(), None);
        assert!(!s.is_running() && s.sources().is_empty());
        assert_eq!(fake.queries.lock().unwrap().len(), 3);
    }
}
