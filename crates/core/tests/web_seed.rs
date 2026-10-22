//! Loopback tests for web seeds (BEP 19 `url-list`, BEP 17 `httpseeds`), after libtorrent's
//! test/web_seed_suite.cpp and test_web_seed*.cpp: a tiny HTTP/1.1 server in the test serves
//! the torrent's files with byte ranges, keep-alive and pipelining; the session downloads from
//! it alone. Cases: a multi-file torrent from its `url-list`, a single file with the file name
//! left out of the URL, chunked bodies, per-file redirects to another path, a single-file
//! redirect, a server that answers 503 before it serves, an HTTP/1.0 server closing after each
//! response, a BEP 17 seed, the peer list showing the seeds, a seed with corrupt data banned
//! while the download completes from a good one, a seed the IP filter blocks, and web seeds
//! added and removed through the handle and kept across a restart.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, Session, SessionHandle, SessionParams, Settings, TorrentFlags, TorrentId,
    TorrentSource, TorrentState, TorrentStatus,
};

const PIECE: usize = 32 * 1024;

struct Fixture {
    torrent: Vec<u8>,
    /// Paths relative to the save directory (the torrent name first for multi-file ones).
    files: Vec<(String, Vec<u8>)>,
    info_hash: [u8; 20],
}

fn data(seed: u32, size: usize) -> Vec<u8> {
    let mut x = 0x9e3779b9u32.wrapping_mul(seed + 1);
    (0..size)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// A torrent of `files` (`name` is the torrent name; one file without a '/' makes a
/// single-file torrent), with `url-list` and `httpseeds` entries.
fn fixture(name: &str, files: &[(&str, usize)], url_list: &[String], http_seeds: &[String]) -> Fixture {
    let single = files.len() == 1 && !files[0].0.contains('/');
    let mut all = Vec::new();
    let mut out = Vec::new();
    for (i, (path, size)) in files.iter().enumerate() {
        let d = data(i as u32, *size);
        all.extend_from_slice(&d);
        let rel = if single { name.to_owned() } else { format!("{name}/{path}") };
        out.push((rel, d));
    }
    let mut pieces = Vec::new();
    for c in all.chunks(PIECE) {
        pieces.extend_from_slice(&Sha1::digest(c).0);
    }
    let mut info = Entry::new_dict();
    if single {
        *info.key("length") = Entry::Int(files[0].1 as i64);
    } else {
        let mut list = Entry::new_list();
        for (path, size) in files {
            let mut f = Entry::new_dict();
            *f.key("length") = Entry::Int(*size as i64);
            *f.key("path") =
                Entry::List(path.split('/').map(|p| Entry::Bytes(p.as_bytes().to_vec())).collect());
            list.list_mut().push(f);
        }
        *info.key("files") = list;
    }
    *info.key("name") = Entry::Bytes(name.as_bytes().to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let info_hash = Sha1::digest(&info.bencode()).0;
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    let list =
        |urls: &[String]| Entry::List(urls.iter().map(|u| Entry::Bytes(u.as_bytes().to_vec())).collect());
    if !url_list.is_empty() {
        *t.key("url-list") = list(url_list);
    }
    if !http_seeds.is_empty() {
        *t.key("httpseeds") = list(http_seeds);
    }
    Fixture { torrent: t.bencode(), files: out, info_hash }
}

fn multi_files() -> Vec<(&'static str, usize)> {
    vec![("alpha.bin", 300_000), ("sub/beta file.bin", 400_000), ("sub/gamma.txt", 12_345), ("empty", 0)]
}

// --- The HTTP server ---------------------------------------------------------------------------

struct Request {
    /// Percent-decoded (lossy); `raw` as sent.
    path: String,
    raw: String,
    range: Option<(u64, u64)>,
    /// Requests the server has seen before this one.
    seq: usize,
}

enum Reply {
    /// 206 with the range of the file at this path (200 and the whole file without a range).
    File(Vec<u8>),
    /// A canned response; `close` ends the connection after it.
    Raw(Vec<u8>, bool),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Plain,
    Chunked,
    /// HTTP/1.0: `Connection: close` after every response.
    Close,
}

struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    hits: Arc<AtomicUsize>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

fn raw(s: &str, close: bool) -> Reply {
    Reply::Raw(s.as_bytes().to_vec(), close)
}

/// Serves requests with `handler` on 127.0.0.1, one thread per connection, keep-alive and
/// pipelined requests answered in order.
fn server(mode: Mode, handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Server {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let hits = Arc::new(AtomicUsize::new(0));
    let log = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(handler);
    let (stop2, hits2, log2) = (stop.clone(), hits.clone(), log.clone());
    std::thread::spawn(move || {
        for conn in l.incoming() {
            if stop2.load(Ordering::SeqCst) {
                break;
            }
            let Ok(c) = conn else { continue };
            let (handler, hits, log) = (handler.clone(), hits2.clone(), log2.clone());
            std::thread::spawn(move || serve_conn(c, mode, &*handler, &hits, &log));
        }
    });
    Server { addr, stop, hits, log }
}

fn serve_conn(
    mut c: TcpStream,
    mode: Mode,
    handler: &dyn Fn(&Request) -> Reply,
    hits: &AtomicUsize,
    log: &Mutex<Vec<String>>,
) {
    c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            match c.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
            continue;
        };
        let head = String::from_utf8_lossy(&buf[..end]).into_owned();
        buf.drain(..end + 4);
        let mut lines = head.split("\r\n");
        let path = lines.next().unwrap_or_default().split(' ').nth(1).unwrap_or_default().to_owned();
        let mut range = None;
        for l in lines {
            if let Some(r) = l.strip_prefix("Range: bytes=") {
                let (a, b) = r.split_once('-').unwrap();
                range = Some((a.parse().unwrap(), b.parse().unwrap()));
            }
        }
        let seq = hits.fetch_add(1, Ordering::SeqCst);
        log.lock().unwrap().push(format!("{path} {range:?}"));
        let req = Request {
            path: String::from_utf8_lossy(&percent_decode(&path)).into_owned(),
            raw: path,
            range,
            seq,
        };
        let (resp, close) = match handler(&req) {
            Reply::Raw(s, close) => (s, close),
            Reply::File(file) => (file_response(&file, range, mode), mode == Mode::Close),
        };
        if c.write_all(&resp).is_err() || close {
            let _ = c.flush();
            return;
        }
    }
}

fn file_response(file: &[u8], range: Option<(u64, u64)>, mode: Mode) -> Vec<u8> {
    let (status, body, extra) = match range {
        Some((a, b)) => {
            let b = b.min(file.len() as u64 - 1);
            (206, &file[a as usize..=b as usize], format!("Content-Range: bytes {a}-{b}/{}\r\n", file.len()))
        }
        None => (200, file, String::new()),
    };
    let proto = if mode == Mode::Close { "HTTP/1.0" } else { "HTTP/1.1" };
    let mut out = format!("{proto} {status} OK\r\nServer: test-seed/1.0\r\n{extra}").into_bytes();
    if mode == Mode::Chunked {
        out.extend_from_slice(b"Transfer-Encoding: chunked\r\n\r\n");
        for c in body.chunks(7777) {
            out.extend_from_slice(format!("{:x}\r\n", c.len()).as_bytes());
            out.extend_from_slice(c);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"0\r\n\r\n");
    } else {
        out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        out.extend_from_slice(body);
    }
    out
}

/// The fixture's files by URL path under `prefix` (percent-decoded, as `Request::path`).
fn files_at(f: &Fixture, prefix: &str) -> HashMap<String, Vec<u8>> {
    f.files.iter().map(|(p, d)| (format!("{prefix}{p}"), d.clone())).collect()
}

fn not_found() -> Reply {
    raw("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n", false)
}

// --- The session -------------------------------------------------------------------------------

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d =
        std::env::temp_dir().join(format!("lt-webseed-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn settings() -> Settings {
    Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        tick_interval: 100,
        aio_threads: 2,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        urlseed_wait_retry: 1,
        ..Settings::default()
    }
}

struct Watch {
    s: Session,
    h: SessionHandle,
    backlog: std::collections::VecDeque<Alert>,
    seen: Vec<String>,
}

impl Watch {
    fn start(settings: Settings) -> Watch {
        Watch::start_in(settings, None)
    }

    fn start_in(settings: Settings, state_dir: Option<PathBuf>) -> Watch {
        let s = Session::start(SessionParams { settings, state_dir }).expect("session starts");
        let h = s.handle();
        Watch { s, h, backlog: Default::default(), seen: Vec::new() }
    }

    fn wait<T>(&mut self, secs: u64, what: &str, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut alerts = Vec::new();
        loop {
            while let Some(a) = self.backlog.pop_front() {
                if !matches!(a, Alert::StateUpdate(_)) {
                    self.seen.push(a.message());
                }
                if let Some(v) = f(&a) {
                    return v;
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            self.h.post_torrent_updates();
            std::thread::sleep(Duration::from_millis(30));
            self.h.pop_alerts(&mut alerts);
            self.backlog.extend(alerts.drain(..));
        }
        panic!("timed out waiting for {what}; alerts: {:#?}", self.seen);
    }

    fn add(&mut self, f: &Fixture, dir: &Path, url_seeds: &[String]) -> TorrentId {
        let mut p =
            AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
        p.flags = TorrentFlags::APPLY_IP_FILTER;
        p.url_seeds = url_seeds.to_vec();
        self.h.add_torrent(p);
        self.wait(5, "add", |a| match a {
            Alert::TorrentAdded { id, .. } => Some(*id),
            Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
            _ => None,
        })
    }

    fn finished(&mut self, id: TorrentId, secs: u64) -> TorrentStatus {
        self.wait(secs, "finished", |a| {
            matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(())
        });
        self.wait(10, "seeding", |a| match a {
            Alert::StateUpdate(v) => {
                v.iter().find(|s| s.id == id && s.state == TorrentState::Seeding).cloned()
            }
            _ => None,
        })
    }

    fn web_seeds(&mut self, id: TorrentId) -> (Vec<String>, Vec<String>) {
        self.h.torrent(id).post_web_seeds();
        self.wait(5, "web seeds", |a| match a {
            Alert::WebSeeds { id: i, url_seeds, http_seeds } if *i == id => {
                Some((url_seeds.clone(), http_seeds.clone()))
            }
            _ => None,
        })
    }
}

fn assert_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let got = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(got == *data, "{name} differs");
    }
}

/// Downloads `f` from its web seeds; checks the files and that nothing failed a hash check.
fn download(f: &Fixture, url_seeds: &[String], secs: u64) -> (Watch, TorrentId, PathBuf, TorrentStatus) {
    let dir = temp_dir("leech");
    let mut w = Watch::start(settings());
    let id = w.add(f, &dir, url_seeds);
    let st = w.finished(id, secs);
    assert_eq!(st.total_failed_bytes, 0);
    assert_eq!(st.total_wanted_done, st.total_wanted);
    (w, id, dir, st)
}

fn finish(w: Watch, f: &Fixture, dir: PathBuf) {
    w.s.shutdown();
    assert_files(f, &dir);
    let _ = std::fs::remove_dir_all(dir);
}

// --- Tests -------------------------------------------------------------------------------------

#[test]
fn multi_file_torrent_from_its_url_list() {
    // the files are under /ws/<torrent name>/...; the url-list entry lacks the trailing '/'
    let base = Arc::new(Mutex::new(HashMap::new()));
    let b2 = base.clone();
    let srv = server(Mode::Plain, move |r| match b2.lock().unwrap().get(&r.path) {
        Some(d) => Reply::File(Vec::clone(d)),
        None => not_found(),
    });
    let f = fixture("web-multi", &multi_files(), &[srv.url("/ws")], &[]);
    *base.lock().unwrap() = files_at(&f, "/ws/");
    let (mut w, id, dir, st) = download(&f, &[], 60);
    assert!(st.total_payload_download >= st.total_wanted);
    // pipelined keep-alive requests on few connections; ranges never cross a file
    let log = srv.log.lock().unwrap().clone();
    assert!(log.iter().all(|l| l.contains("Some(")), "every request has a range: {log:?}");
    assert!(log.iter().any(|l| l.starts_with("/ws/web-multi/sub/beta%20file.bin")), "{log:?}");
    let (urls, http) = w.web_seeds(id);
    // a multi-file torrent's url-list entries get the trailing slash (libtorrent too)
    assert_eq!((urls, http.len()), (vec![srv.url("/ws/")], 0));
    finish(w, &f, dir);
}

#[test]
fn single_file_with_the_name_left_out_of_the_url() {
    let f = fixture("single.iso", &[("single.iso", 500_000)], &[], &[]);
    let file = f.files[0].1.clone();
    let srv = server(Mode::Plain, move |r| {
        if r.path == "/files/single.iso" { Reply::File(file.clone()) } else { not_found() }
    });
    let (w, _, dir, _) = download(&f, &[srv.url("/files/")], 60);
    assert!(srv.hits.load(Ordering::SeqCst) > 0);
    finish(w, &f, dir);
}

#[test]
fn chunked_transfer_encoding() {
    let base = Arc::new(Mutex::new(HashMap::new()));
    let b2 = base.clone();
    let srv = server(Mode::Chunked, move |r| match b2.lock().unwrap().get(&r.path) {
        Some(d) => Reply::File(Vec::clone(d)),
        None => not_found(),
    });
    let f = fixture("web-chunked", &multi_files(), &[], &[]);
    *base.lock().unwrap() = files_at(&f, "/");
    let (w, _, dir, _) = download(&f, &[srv.url("/")], 60);
    finish(w, &f, dir);
}

#[test]
fn http_1_0_server_closing_after_every_response() {
    let f = fixture("closing.bin", &[("closing.bin", 300_000)], &[], &[]);
    let file = f.files[0].1.clone();
    let srv = server(Mode::Close, move |_| Reply::File(file.clone()));
    let (w, _, dir, _) = download(&f, &[srv.url("/closing.bin")], 60);
    assert!(srv.hits.load(Ordering::SeqCst) >= 2, "one request per connection");
    finish(w, &f, dir);
}

#[test]
fn per_file_redirects_to_another_path() {
    // every file under /old/ moves to /new/: the redirect target becomes an ephemeral web seed
    // that has just the files redirected to it
    let base = Arc::new(Mutex::new(HashMap::new()));
    let b2 = base.clone();
    let srv = server(Mode::Plain, move |r| {
        if let Some(rest) = r.path.strip_prefix("/old/") {
            let target = format!("/new/{}", rest.replace(' ', "%20"));
            return raw(
                &format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\n\r\n"),
                false,
            );
        }
        match b2.lock().unwrap().get(&r.path) {
            Some(d) => Reply::File(Vec::clone(d)),
            None => not_found(),
        }
    });
    // files on piece boundaries: a server that lacks some of the files does not claim the
    // pieces they share with others (libtorrent), so with unaligned files the redirects could
    // leave a boundary piece that neither server claims
    let files = [
        ("alpha.bin", 10 * PIECE),
        ("sub/beta file.bin", 12 * PIECE),
        ("sub/gamma.txt", 12_345),
        ("empty", 0),
    ];
    let f = fixture("web-redirect", &files, &[], &[]);
    *base.lock().unwrap() = files_at(&f, "/new/");
    let (mut w, id, dir, _) = download(&f, &[srv.url("/old/")], 60);
    let log = srv.log.lock().unwrap().clone();
    for file in ["alpha.bin", "sub/beta%20file.bin", "sub/gamma.txt"] {
        assert!(log.iter().any(|l| l.starts_with(&format!("/old/web-redirect/{file}"))), "{file}: {log:?}");
        assert!(log.iter().any(|l| l.starts_with(&format!("/new/web-redirect/{file}"))), "{file}: {log:?}");
    }
    // the ephemeral seed is listed, but only the original is saved
    let (urls, _) = w.web_seeds(id);
    assert!(urls.contains(&srv.url("/old/")) && urls.contains(&srv.url("")), "{urls:?}");
    finish(w, &f, dir);
}

#[test]
fn single_file_redirect() {
    // after libtorrent test_web_seed_redirect.cpp: a 16000-byte file behind /redirect
    let f = fixture("test_file", &[("test_file", 16_000)], &[], &[]);
    let file = f.files[0].1.clone();
    let srv = server(Mode::Plain, move |r| match r.path.as_str() {
        "/redirect" => raw("HTTP/1.1 301 Moved\r\nLocation: /test_file\r\nContent-Length: 0\r\n\r\n", false),
        "/test_file" => Reply::File(file.clone()),
        _ => not_found(),
    });
    let (w, _, dir, _) = download(&f, &[srv.url("/redirect")], 60);
    let log = srv.log.lock().unwrap().clone();
    assert!(log[0].starts_with("/redirect") && log.iter().any(|l| l.starts_with("/test_file")), "{log:?}");
    finish(w, &f, dir);
}

#[test]
fn server_busy_with_503_then_serving() {
    let f = fixture("busy.bin", &[("busy.bin", 200_000)], &[], &[]);
    let file = f.files[0].1.clone();
    let srv = server(Mode::Plain, move |r| {
        if r.seq == 0 {
            raw(
                "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                true,
            )
        } else {
            Reply::File(file.clone())
        }
    });
    let started = Instant::now();
    let dir = temp_dir("busy");
    let mut w = Watch::start(settings());
    let id = w.add(&f, &dir, &[srv.url("/busy.bin")]);
    let msg = w.wait(20, "url seed alert", |a| match a {
        Alert::UrlSeed { id: i, url, message } if *i == id => Some((url.clone(), message.clone())),
        _ => None,
    });
    assert_eq!(msg, (srv.url("/busy.bin"), "503 Service Unavailable".to_owned()));
    let st = w.finished(id, 60);
    assert_eq!(st.total_failed_bytes, 0);
    assert!(started.elapsed() >= Duration::from_secs(1), "waited for Retry-After");
    finish(w, &f, dir);
}

#[test]
fn bep17_http_seed() {
    let f = fixture("hoffman.bin", &[("hoffman.bin", 300_000)], &[], &[]);
    let file = f.files[0].1.clone();
    let ih = f.info_hash;
    let srv = server(Mode::Plain, move |r| {
        // /seed?info_hash=..&piece=N[&ranges=a-b]
        let (path, q) = r.raw.split_once('?').unwrap_or((&r.raw, ""));
        assert_eq!(path, "/seed");
        assert!(r.range.is_none());
        let mut piece = None;
        let mut ranges = None;
        for kv in q.split('&') {
            match kv.split_once('=') {
                Some(("info_hash", v)) => assert_eq!(percent_decode(v), ih),
                Some(("piece", v)) => piece = Some(v.parse::<usize>().unwrap()),
                Some(("ranges", v)) => {
                    let (a, b) = v.split_once('-').unwrap();
                    ranges = Some((a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()));
                }
                _ => {}
            }
        }
        let start = piece.unwrap() * PIECE;
        let end = (start + PIECE).min(file.len());
        let body = match ranges {
            Some((a, b)) => &file[start + a..=start + b],
            None => &file[start..end],
        };
        let mut s = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        s.extend_from_slice(body);
        Reply::Raw(s, false)
    });
    let f = fixture("hoffman.bin", &[("hoffman.bin", 300_000)], &[], &[srv.url("/seed")]);
    let (mut w, id, dir, _) = download(&f, &[], 60);
    let (urls, http) = w.web_seeds(id);
    assert_eq!((urls.len(), http), (0, vec![srv.url("/seed")]));
    finish(w, &f, dir);
}

#[test]
fn web_seeds_show_in_the_peer_list() {
    // a slow server: each response waits, so the connection is there to be seen
    let f = fixture("slow.bin", &[("slow.bin", 400_000)], &[], &[]);
    let file = f.files[0].1.clone();
    let srv = server(Mode::Plain, move |_| {
        std::thread::sleep(Duration::from_millis(150));
        Reply::File(file.clone())
    });
    let dir = temp_dir("peers");
    let mut w = Watch::start(settings());
    let id = w.add(&f, &dir, &[srv.url("/slow.bin")]);
    let deadline = Instant::now() + Duration::from_secs(20);
    let peer = loop {
        w.h.torrent(id).post_peer_info();
        let peers = w.wait(5, "peer info", |a| match a {
            Alert::PeerInfo { id: i, peers } if *i == id => Some(peers.clone()),
            _ => None,
        });
        if let Some(p) = peers.into_iter().find(|p| p.is_web_seed && !p.client.is_empty()) {
            break p;
        }
        assert!(Instant::now() < deadline, "no web seed in the peer list");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(peer.ip, srv.addr);
    assert_eq!(peer.url.as_deref(), Some(srv.url("/slow.bin").as_str()), "the Peers tab shows the URL");
    assert_eq!(peer.client, "test-seed/1.0");
    assert!(peer.flags.contains(patorrent_core::PeerFlags::SEED));
    let st = w.finished(id, 60);
    assert_eq!(st.total_failed_bytes, 0);
    finish(w, &f, dir);
}

#[test]
fn corrupt_web_seed_is_banned_while_a_good_one_completes_the_download() {
    // after libtorrent test_web_seed_ban.cpp: the bad server's file has every byte flipped, so
    // the first piece it sends fails, and as the only sender of a single-file torrent's piece it
    // is banned; the good server supplies everything
    let f = fixture("ban.bin", &[("ban.bin", 1_000_000)], &[], &[]);
    let good_file = f.files[0].1.clone();
    let bad_file: Vec<u8> = good_file.iter().map(|b| !b).collect();
    let good = server(Mode::Plain, move |_| {
        // a little slower, so the bad seed surely gets pieces too
        std::thread::sleep(Duration::from_millis(20));
        Reply::File(good_file.clone())
    });
    let bad = server(Mode::Plain, move |_| Reply::File(bad_file.clone()));
    let dir = temp_dir("ban");
    let mut w = Watch::start(settings());
    let id = w.add(&f, &dir, &[bad.url("/ban.bin"), good.url("/ban.bin")]);
    let banned = w.wait(60, "ban", |a| match a {
        Alert::PeerBanned { id: i, addr, .. } if *i == id => Some(*addr),
        _ => None,
    });
    assert_eq!(banned, bad.addr);
    let st = w.finished(id, 60);
    assert!(st.total_failed_bytes > 0);
    let (urls, _) = w.web_seeds(id);
    assert_eq!(urls, [good.url("/ban.bin")], "the banned seed is not listed");
    finish(w, &f, dir);
}

#[test]
fn ip_filter_blocks_a_web_seed() {
    let f = fixture("blocked.bin", &[("blocked.bin", 100_000)], &[], &[]);
    let file = f.files[0].1.clone();
    let srv = server(Mode::Plain, move |_| Reply::File(file.clone()));
    let dir = temp_dir("blocked");
    let mut w = Watch::start(settings());
    let mut filter = patorrent_core::IpFilter::default();
    filter.add_rule(
        "127.0.0.1".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
        patorrent_core::IpFilter::BLOCKED,
    );
    w.h.set_ip_filter(filter);
    let id = w.add(&f, &dir, &[srv.url("/blocked.bin")]);
    let addr = w.wait(20, "blocked", |a| match a {
        Alert::PeerBlocked { id: Some(i), addr, .. } if *i == id => Some(*addr),
        _ => None,
    });
    assert_eq!(addr, srv.addr);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(srv.hits.load(Ordering::SeqCst), 0, "never contacted");
    w.s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn web_seeds_are_saved_and_can_be_added_and_removed() {
    // nothing listens on these: the torrent is paused, so they are never contacted
    let f = fixture(
        "saved.bin",
        &[("saved.bin", 50_000)],
        &["http://127.0.0.1:9/a".into()],
        &["http://127.0.0.1:9/h".into()],
    );
    let dir = temp_dir("saved");
    let state = temp_dir("saved-state");
    {
        let mut w = Watch::start_in(settings(), Some(state.clone()));
        let mut p =
            AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
        p.flags = TorrentFlags::PAUSED;
        p.url_seeds = vec!["http://127.0.0.1:9/b".into()];
        w.h.add_torrent(p);
        let id = w.wait(5, "add", |a| match a {
            Alert::TorrentAdded { id, .. } => Some(*id),
            _ => None,
        });
        let t = w.h.torrent(id);
        t.add_url_seed("http://127.0.0.1:9/c");
        t.add_url_seed("http://127.0.0.1:9/a");
        t.remove_url_seed("http://127.0.0.1:9/b");
        t.add_http_seed("http://127.0.0.1:9/h2");
        let (urls, http) = w.web_seeds(id);
        assert_eq!(urls, ["http://127.0.0.1:9/a", "http://127.0.0.1:9/c"]);
        assert_eq!(http, ["http://127.0.0.1:9/h", "http://127.0.0.1:9/h2"]);
        w.s.shutdown();
    }
    let mut w = Watch::start_in(settings(), Some(state.clone()));
    let id2 = w.wait(10, "restored", |a| match a {
        Alert::StateUpdate(v) => v.first().map(|s| s.id),
        _ => None,
    });
    let (urls, http) = w.web_seeds(id2);
    assert_eq!(urls, ["http://127.0.0.1:9/a", "http://127.0.0.1:9/c"]);
    assert_eq!(http, ["http://127.0.0.1:9/h", "http://127.0.0.1:9/h2"]);
    w.s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(state);
}
