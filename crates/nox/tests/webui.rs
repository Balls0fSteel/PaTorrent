//! The Web UI and WebAPI over loopback: a real session plus the server, driven by a raw HTTP
//! client. Login and logout, the failed-login ban, CSRF and Host checks, adding torrents
//! (multipart file, magnet), `torrents/info`, `sync/maindata` diffs by rid, stop / start,
//! categories, delete, preferences, and the connection handling (keep-alive, pipelining,
//! `Expect: 100-continue`, size limits). Then the native API: the WebSocket upgrade and
//! frames with a raw socket client, and the session protocol (`hello`, `view` / `rows`,
//! `tick` diffs, `idle`) against the real engine and the 10,000-torrent mock profile, and the
//! detail tabs (`detail` subscriptions: General, Trackers, Files, Pieces, Speed, Peers) plus
//! `sync/torrentPeers` through the shim, and the native commands (`POST /pa/...`: start /
//! pause / stop in the cells, queue moves, limits, labels, "all" under a filter, remove with
//! data, the two-step add with the file tree, a magnet, the session limits, CSRF).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{MockConfig, Session, SessionParams, Settings};
use patorrent_nox::webui::json::{self, Json};
use patorrent_nox::webui::{Config, STATE_FILE, WebUi};

const PASS: &str = "s3cret-pass";

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d =
        std::env::temp_dir().join(format!("nox-webui-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A two-file torrent (no data on disk) and its info-hash.
fn torrent() -> (Vec<u8>, String) {
    torrent_with(None)
}

/// The same torrent with a tracker (another info-hash is not needed: the tracker is outside
/// the info dictionary).
fn torrent_with(tracker: Option<&str>) -> (Vec<u8>, String) {
    torrent_named("webui-test", tracker)
}

/// The two-file torrent under another name (so another info-hash).
fn torrent_named(name: &str, tracker: Option<&str>) -> (Vec<u8>, String) {
    torrent_built(name, tracker, false)
}

/// The two files' contents, one after the other (`a.bin` is the first 40,000 bytes).
fn torrent_data() -> Vec<u8> {
    (0..50_000usize).map(|i| (i * 7 % 251) as u8).collect()
}

/// The two-file torrent, BEP 27 private when `private`.
fn torrent_built(name: &str, tracker: Option<&str>, private: bool) -> (Vec<u8>, String) {
    let piece = 16 * 1024;
    let sizes = [("a.bin", 40_000usize), ("b.bin", 10_000)];
    let data = torrent_data();
    let pieces: Vec<u8> = data.chunks(piece).flat_map(|c| Sha1::digest(c).0).collect();
    let mut info = Entry::new_dict();
    let mut list = Entry::new_list();
    for (name, len) in sizes {
        let mut f = Entry::new_dict();
        *f.key("length") = Entry::Int(len as i64);
        *f.key("path") = Entry::List(vec![Entry::Bytes(name.as_bytes().to_vec())]);
        list.list_mut().push(f);
    }
    *info.key("files") = list;
    *info.key("name") = Entry::Bytes(name.as_bytes().to_vec());
    *info.key("piece length") = Entry::Int(piece as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    if private {
        *info.key("private") = Entry::Int(1);
    }
    let hash = Sha1::digest(&info.bencode()).to_hex();
    let mut t = Entry::new_dict();
    if let Some(url) = tracker {
        *t.key("announce") = Entry::Bytes(url.as_bytes().to_vec());
    }
    *t.key("info") = info;
    *t.key("comment") = Entry::Bytes(b"made for a test".to_vec());
    *t.key("created by") = Entry::Bytes(b"the test".to_vec());
    *t.key("creation date") = Entry::Int(951_782_400);
    (t.bencode(), hash)
}

struct Harness {
    web: Arc<WebUi>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
    session: Option<Session>,
    dir: PathBuf,
    /// The TCP addresses the engine reported listening on (`ListenSucceeded`).
    listen: Arc<std::sync::Mutex<Vec<SocketAddr>>>,
}

impl Harness {
    fn start(name: &str) -> Harness {
        Harness::start_in(temp_dir(name), Some(PASS))
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
            ..Settings::default()
        }
    }

    fn start_in(dir: PathBuf, password: Option<&str>) -> Harness {
        let settings = Harness::settings();
        let session = Session::start(SessionParams { settings: settings.clone(), state_dir: None }).unwrap();
        Harness::with_session(dir, password, settings, session)
    }

    /// A real session with `settings` that keeps its torrents in `dir/engine`, so a harness
    /// started again on `dir` restores them.
    fn start_with(dir: PathBuf, settings: Settings) -> Harness {
        let state_dir = Some(dir.join("engine"));
        let session = Session::start(SessionParams { settings: settings.clone(), state_dir }).unwrap();
        Harness::with_session(dir, Some(PASS), settings, session)
    }

    /// The simulated engine with `n` synthetic torrents, `active` of them force-started; the
    /// rest are queued seeds or stopped, and with `limits` at zero nothing auto-managed runs,
    /// so nothing changes after the first update.
    fn start_mock(name: &str, n: usize, active: f64, limits: i32) -> Harness {
        let settings = Settings {
            active_downloads: limits,
            active_seeds: limits,
            active_limit: limits,
            ..Harness::settings()
        };
        let config = MockConfig { initial_torrents: n, active_fraction: active, ..MockConfig::default() };
        let session = Session::start_mock(settings.clone(), config);
        Harness::with_session(temp_dir(name), Some(PASS), settings, session)
    }

    fn with_session(dir: PathBuf, password: Option<&str>, settings: Settings, session: Session) -> Harness {
        let h = session.handle();
        let mut c = Config::new("127.0.0.1:0".parse().unwrap(), settings, dir.join("dl").to_string_lossy());
        c.password = password.map(str::to_owned);
        c.state_dir = Some(dir.join("state"));
        c.pbkdf2_iterations = 2;
        let web = Arc::new(WebUi::start(h.clone(), c).unwrap());
        let addr = web.local_addr();
        let stop = Arc::new(AtomicBool::new(false));
        let listen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let pump = {
            let (web, stop, listen) = (web.clone(), stop.clone(), listen.clone());
            std::thread::spawn(move || {
                let mut alerts = Vec::new();
                while !stop.load(Ordering::SeqCst) {
                    h.post_torrent_updates();
                    h.post_session_stats();
                    std::thread::sleep(Duration::from_millis(50));
                    h.pop_alerts(&mut alerts);
                    for a in &alerts {
                        if let patorrent_core::Alert::ListenSucceeded { address, protocol } = a {
                            if format!("{protocol:?}") == "Tcp" {
                                listen.lock().unwrap().push(*address);
                            }
                        }
                    }
                    web.on_alerts(&alerts);
                    alerts.clear();
                }
            })
        };
        Harness { web, addr, stop, pump: Some(pump), session: Some(session), dir, listen }
    }

    fn host(&self) -> String {
        self.addr.to_string()
    }

    fn login(&self) -> String {
        let r = self.req(
            "POST",
            "/api/v2/auth/login",
            &[],
            Body::Form(&format!("username=admin&password={PASS}")),
        );
        assert_eq!((r.status, r.text().as_str()), (200, "Ok."));
        let c = r.header("set-cookie").expect("a session cookie");
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict"), "{c}");
        c.split(';').next().unwrap().to_owned()
    }

    fn req(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Body<'_>) -> Resp {
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        s.write_all(&raw_request(&self.host(), method, path, headers, body, true)).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        parse_response(&out).0
    }

    fn get(&self, sid: &str, path: &str) -> Resp {
        self.req("GET", path, &[("Cookie", sid)], Body::None)
    }

    fn post(&self, sid: &str, path: &str, form: &str) -> Resp {
        self.req("POST", path, &[("Cookie", sid)], Body::Form(form))
    }

    /// A native command: a JSON body with the CSRF header and a same-origin `Origin`.
    fn pa(&self, sid: &str, path: &str, body: &str) -> Resp {
        let origin = format!("http://{}", self.host());
        self.req(
            "POST",
            path,
            &[("Cookie", sid), ("Origin", &origin), ("X-Requested-With", "PaTorrent")],
            Body::Json(body),
        )
    }

    /// A native upload (`/pa/add`, `/pa/torrent-info`): the torrent as the `torrent` part and
    /// the options JSON as the `options` part.
    fn pa_upload(&self, sid: &str, path: &str, options: &str, torrent: &[u8]) -> Resp {
        let (body, boundary) = multipart_named(&[("options", options)], "torrent", torrent);
        let origin = format!("http://{}", self.host());
        self.req(
            "POST",
            path,
            &[("Cookie", sid), ("Origin", &origin), ("X-Requested-With", "PaTorrent")],
            Body::Multipart(&body, boundary),
        )
    }

    fn maindata(&self, sid: &str, rid: i64) -> Json {
        let r = self.get(sid, &format!("/api/v2/sync/maindata?rid={rid}"));
        assert_eq!(r.status, 200);
        r.json()
    }

    /// Polls `torrents/info` until the torrent is gone.
    fn wait_for_gone(&self, sid: &str, hash: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let list = self.get(sid, &format!("/api/v2/torrents/info?hashes={hash}")).json();
            if matches!(&list, Json::Arr(v) if v.is_empty()) {
                return;
            }
            assert!(Instant::now() < deadline, "{hash} still listed: {}", list.to_json());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Polls `torrents/info` until `f` holds for the torrent.
    /// The port the engine listens on for TCP peers: the one it reported, not the one asked
    /// for (a configured port that is taken is moved past, `max_retry_port_bind`).
    fn listen_port(&self) -> u16 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(a) = self.listen.lock().unwrap().first() {
                return a.port();
            }
            assert!(Instant::now() < deadline, "the engine never reported a TCP listen socket");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for(&self, sid: &str, hash: &str, what: &str, f: impl Fn(&Json) -> bool) -> Json {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let list = self.get(sid, &format!("/api/v2/torrents/info?hashes={hash}")).json();
            if let Json::Arr(v) = &list {
                if let Some(t) = v.first() {
                    if f(t) {
                        return t.clone();
                    }
                }
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}: {}", list.to_json());
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(p) = self.pump.take() {
            let _ = p.join();
        }
        if let Some(s) = self.session.take() {
            s.shutdown();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

enum Body<'a> {
    None,
    Form(&'a str),
    Json(&'a str),
    Multipart(&'a [u8], &'a str),
}

fn raw_request(
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Body<'_>,
    close: bool,
) -> Vec<u8> {
    let mut r = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n");
    for (n, v) in headers {
        r.push_str(&format!("{n}: {v}\r\n"));
    }
    if close {
        r.push_str("Connection: close\r\n");
    }
    let body: &[u8] = match body {
        Body::None => b"",
        Body::Form(f) => {
            r.push_str("Content-Type: application/x-www-form-urlencoded\r\n");
            f.as_bytes()
        }
        Body::Json(j) => {
            r.push_str("Content-Type: application/json\r\n");
            j.as_bytes()
        }
        Body::Multipart(b, boundary) => {
            r.push_str(&format!("Content-Type: multipart/form-data; boundary={boundary}\r\n"));
            b
        }
    };
    r.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut out = r.into_bytes();
    out.extend_from_slice(body);
    out
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Json {
        json::parse(&self.text()).unwrap_or_else(|e| panic!("{e}: {}", self.text()))
    }
}

/// One response from the start of `buf`, and the bytes it took.
fn parse_response(buf: &[u8]) -> (Resp, usize) {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").expect("a response head") + 4;
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .map_or(0, |(_, v)| v.parse().unwrap());
    (Resp { status, headers, body: buf[end..end + len].to_vec() }, end + len)
}

fn multipart(fields: &[(&str, &str)], torrent: &[u8]) -> (Vec<u8>, &'static str) {
    multipart_named(fields, "torrents", torrent)
}

fn multipart_named(fields: &[(&str, &str)], file_part: &str, torrent: &[u8]) -> (Vec<u8>, &'static str) {
    let b = "----noxtestboundary";
    let mut out = Vec::new();
    for (n, v) in fields {
        out.extend_from_slice(
            format!("--{b}\r\nContent-Disposition: form-data; name=\"{n}\"\r\n\r\n{v}\r\n").as_bytes(),
        );
    }
    out.extend_from_slice(
        format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"{file_part}\"; filename=\"t.torrent\"\r\n\
Content-Type: application/x-bittorrent\r\n\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(torrent);
    out.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    (out, b)
}

fn s(v: Option<&Json>) -> &str {
    v.and_then(Json::as_str).unwrap_or("")
}

/// A text frame's payload as JSON.
fn parse_text(payload: Vec<u8>) -> Json {
    let text = String::from_utf8(payload).unwrap();
    json::parse(&text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// Whether a text frame's payload is a `tick` message.
fn is_tick(payload: &[u8]) -> bool {
    std::str::from_utf8(payload)
        .ok()
        .and_then(|t| json::parse(t).ok())
        .is_some_and(|m| s(m.get("t")) == "tick")
}

#[test]
fn login_logout_and_versions() {
    let t = Harness::start("login");
    assert_eq!(t.req("GET", "/api/v2/app/version", &[], Body::None).status, 403);
    let bad = t.req("POST", "/api/v2/auth/login", &[], Body::Form("username=admin&password=wrong"));
    assert_eq!((bad.status, bad.text().as_str()), (200, "Fails."));
    assert!(bad.header("set-cookie").is_none());
    assert_eq!(t.req("GET", "/api/v2/auth/login?username=admin&password=x", &[], Body::None).status, 405);

    let sid = t.login();
    let v = t.get(&sid, "/api/v2/app/version");
    assert_eq!((v.status, v.text().as_str()), (200, "v5.0.0"));
    assert_eq!(v.header("x-frame-options"), Some("SAMEORIGIN"));
    assert_eq!(t.get(&sid, "/api/v2/app/webapiVersion").text(), "2.11.2");
    let b = t.get(&sid, "/api/v2/app/buildInfo").json();
    assert_eq!(s(b.get("libtorrent")), "2.0.11.0");
    assert!(b.get("bitness").and_then(Json::as_i64).is_some());
    assert!(t.get(&sid, "/api/v2/app/defaultSavePath").text().ends_with("dl"));
    assert_eq!(t.get(&sid, "/api/v2/nope").status, 404);
    // a state change by GET
    assert_eq!(t.get(&sid, "/api/v2/torrents/stop?hashes=all").status, 405);
    assert_eq!(t.req("PUT", "/api/v2/app/version", &[("Cookie", &sid)], Body::None).status, 405);
    // the page needs no login (it holds no data)
    let page = t.req("GET", "/", &[], Body::None);
    assert_eq!(page.status, 200);
    assert!(page.text().contains("<title>PaTorrent</title>"));
    assert!(page.header("content-security-policy").is_some());

    assert_eq!(t.post(&sid, "/api/v2/auth/logout", "").status, 200);
    assert_eq!(t.get(&sid, "/api/v2/app/version").status, 403);
    assert_eq!(t.get("SID=forged", "/api/v2/app/version").status, 403);
}

#[test]
fn static_files_have_their_headers_and_no_inline_code() {
    let t = Harness::start("static");
    let files = [
        ("/", "text/html; charset=UTF-8"),
        ("/style.css", "text/css; charset=UTF-8"),
        ("/app.js", "text/javascript; charset=UTF-8"),
        ("/lang.js", "text/javascript; charset=UTF-8"),
    ];
    let mut total = 0;
    for (path, content_type) in files {
        // no login needed: the files hold no data
        let r = t.req("GET", path, &[], Body::None);
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(r.header("content-type"), Some(content_type), "{path}");
        assert_eq!(r.header("x-content-type-options"), Some("nosniff"), "{path}");
        assert_eq!(r.header("x-frame-options"), Some("SAMEORIGIN"), "{path}");
        assert_eq!(r.header("referrer-policy"), Some("no-referrer"), "{path}");
        assert_eq!(r.header("cache-control"), Some("no-cache"), "{path}");
        assert_eq!(
            r.headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case("cache-control")).count(),
            1,
            "{path}: one Cache-Control"
        );
        let etag = r.header("etag").unwrap_or_else(|| panic!("{path}: no ETag")).to_owned();
        assert!(etag.starts_with('"') && etag.ends_with('"') && etag.len() > 10, "{path}: {etag}");
        assert!(!r.body.is_empty());
        // the string table has a budget of its own
        if path == "/lang.js" {
            assert!(r.body.len() < 96 * 1024, "lang.js is {} bytes", r.body.len());
        } else {
            total += r.body.len();
        }
        // the browser's revalidation
        let again = t.req("GET", path, &[("If-None-Match", &etag)], Body::None);
        assert_eq!(again.status, 304, "{path}");
        assert!(again.body.is_empty(), "{path}");
        assert_eq!(again.header("etag"), Some(etag.as_str()), "{path}");
        let other = t.req("GET", path, &[("If-None-Match", "\"nope\", W/\"x\"")], Body::None);
        assert_eq!(other.status, 200, "{path}");
        // HEAD: the same head (Content-Length of the body included) and nothing after it
        let mut s = TcpStream::connect(t.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        s.write_all(&raw_request(&t.host(), "HEAD", path, &[], Body::None, true)).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{path}: {text}");
        assert!(text.ends_with("\r\n\r\n"), "{path}: a body after a HEAD");
        assert!(text.contains(&format!("ETag: {etag}\r\n")), "{path}: {text}");
        assert!(text.contains(&format!("Content-Length: {}\r\n", r.body.len())), "{path}: {text}");
        assert_eq!(t.req("POST", path, &[], Body::Form("")).status, 405, "{path}");
    }
    assert!(total < 200 * 1024, "the three files are {total} bytes; the budget is 200 KB");
    let page = t.req("GET", "/", &[], Body::None);
    let csp = page.header("content-security-policy").expect("a CSP on the page");
    assert!(csp.starts_with("default-src 'self'") && !csp.contains("unsafe"), "{csp}");
    assert_eq!(t.req("GET", "/index.html", &[], Body::None).header("etag"), page.header("etag"));
    for other in ["/style.css", "/app.js", "/lang.js"] {
        assert!(t.req("GET", other, &[], Body::None).header("content-security-policy").is_none(), "{other}");
    }
    // the HTML: no inline script or style anywhere, and only our own files referenced
    let html = page.text();
    assert!(html.contains("<title>PaTorrent</title>"));
    let lower = html.to_ascii_lowercase();
    assert!(!lower.contains("<style"), "an inline <style> element");
    assert!(!lower.contains(" style=\""), "an inline style attribute");
    assert!(!lower.contains("javascript:"), "a javascript: URL");
    for (at, _) in lower.match_indices("<script") {
        let tag = &lower[at..at + lower[at..].find('>').unwrap()];
        assert!(
            tag.contains("src=\"app.js\"") || tag.contains("src=\"lang.js\""),
            "an inline <script>: {tag}"
        );
        assert!(lower[at..].trim_start_matches(tag).starts_with("></script>"), "a <script> with content");
    }
    for (at, _) in lower.match_indices(" on") {
        // onclick="..." and friends
        let rest = &lower[at + 3..];
        let name: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        assert!(!rest[name.len()..].starts_with("=\""), "an inline event handler on{name}");
    }
    assert!(lower.contains("<link rel=\"stylesheet\" href=\"style.css\">"));
    assert!(
        !lower.contains("http://") && !lower.contains("https://"),
        "the page loads nothing from elsewhere"
    );
}

#[test]
fn failed_logins_get_the_address_banned() {
    let t = Harness::start("ban");
    for _ in 0..5 {
        let r = t.req("POST", "/api/v2/auth/login", &[], Body::Form("username=admin&password=nope"));
        assert_eq!(r.text(), "Fails.");
    }
    let r = t.req("POST", "/api/v2/auth/login", &[], Body::Form(&format!("username=admin&password={PASS}")));
    assert_eq!(r.status, 403);
    assert!(r.text().contains("banned"));
    assert_eq!(t.req("GET", "/api/v2/app/version", &[], Body::None).status, 403);
}

#[test]
fn csrf_and_host_header_checks() {
    let t = Harness::start("csrf");
    let sid = t.login();
    let host = t.host();
    let origin = format!("http://{host}");
    let ok = t.req("GET", "/api/v2/app/version", &[("Cookie", &sid), ("Origin", &origin)], Body::None);
    assert_eq!(ok.status, 200);
    let referer = format!("http://{host}/index.html");
    assert_eq!(
        t.req("GET", "/api/v2/app/version", &[("Cookie", &sid), ("Referer", &referer)], Body::None).status,
        200
    );
    for (n, v) in
        [("Origin", "http://evil.example"), ("Referer", "http://evil.example/x"), ("Origin", "null")]
    {
        let r = t.req("POST", "/api/v2/torrents/stop", &[("Cookie", &sid), (n, v)], Body::Form("hashes=all"));
        assert_eq!(r.status, 401, "{n}: {v}");
    }
    // a cross-site login attempt is refused too
    let r = t.req(
        "POST",
        "/api/v2/auth/login",
        &[("Origin", "http://evil.example")],
        Body::Form(&format!("username=admin&password={PASS}")),
    );
    assert_eq!(r.status, 401);

    // DNS rebinding: a request for another host name
    let mut s = TcpStream::connect(t.addr).unwrap();
    s.write_all(
        format!("GET /api/v2/app/version HTTP/1.1\r\nHost: rebind.example:{}\r\nCookie: {sid}\r\nConnection: close\r\n\r\n", t.addr.port())
            .as_bytes(),
    )
    .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    assert_eq!(parse_response(&out).0.status, 400);

    // ... accepted once the name is in the domain list
    let prefs = r#"{"web_ui_domain_list":"*.example"}"#;
    assert_eq!(t.post(&sid, "/api/v2/app/setPreferences", &format!("json={}", enc(prefs))).status, 200);
    let mut s = TcpStream::connect(t.addr).unwrap();
    s.write_all(
        format!("GET /api/v2/app/version HTTP/1.1\r\nHost: rebind.example:{}\r\nCookie: {sid}\r\nConnection: close\r\n\r\n", t.addr.port())
            .as_bytes(),
    )
    .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    assert_eq!(parse_response(&out).0.status, 200);
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

#[test]
fn torrent_lifecycle_and_maindata_diffs() {
    let t = Harness::start("lifecycle");
    let sid = t.login();
    let (bytes, hash) = torrent();

    // rid 0: a full update with no torrents yet
    let d = t.maindata(&sid, 0);
    assert_eq!(d.get("full_update"), Some(&Json::Bool(true)));
    let rid = d.get("rid").and_then(Json::as_i64).unwrap();
    assert_eq!(d.get("torrents").map(|v| v.entries().len()), Some(0));
    assert!(d.get("server_state").and_then(|s| s.get("connection_status")).is_some());
    assert_eq!(d.get("tags"), Some(&Json::Arr(vec![])));

    assert_eq!(
        t.post(&sid, "/api/v2/torrents/createCategory", "category=tv&savePath=%2Fsrv%2Ftv").status,
        200
    );
    assert_eq!(t.post(&sid, "/api/v2/torrents/createCategory", "category=tv").status, 409);
    assert_eq!(t.post(&sid, "/api/v2/torrents/createCategory", "category=").status, 400);

    let (body, boundary) = multipart(&[("category", "tv"), ("paused", "false")], &bytes);
    let r = t.req("POST", "/api/v2/torrents/add", &[("Cookie", &sid)], Body::Multipart(&body, boundary));
    assert_eq!((r.status, r.text().as_str()), (200, "Ok."));

    let info = t.wait_for(&sid, &hash, "the added torrent", |x| s(x.get("name")) == "webui-test");
    assert_eq!(s(info.get("category")), "tv");
    assert_eq!(info.get("total_size").and_then(Json::as_i64), Some(50_000));
    assert!(!s(info.get("state")).starts_with("paused"), "{}", info.to_json());

    // the diff since `rid`: the new torrent in full, the new category, no full_update
    let d = t.maindata(&sid, rid);
    assert_eq!(d.get("full_update"), None, "{}", d.to_json());
    let rid2 = d.get("rid").and_then(Json::as_i64).unwrap();
    assert!(rid2 > rid);
    let full = d.get("torrents").and_then(|m| m.get(&hash)).expect("the torrent");
    assert!(full.entries().len() > 40, "{}", full.to_json());
    assert_eq!(s(d.get("categories").and_then(|c| c.get("tv")).and_then(|c| c.get("savePath"))), "/srv/tv");

    // unchanged since: nothing about it, or only changed fields
    let d = t.maindata(&sid, rid2);
    let rid3 = d.get("rid").and_then(Json::as_i64).unwrap();
    if let Some(o) = d.get("torrents").and_then(|m| m.get(&hash)) {
        assert!(o.entries().len() < 10 && o.get("name").is_none(), "{}", o.to_json());
    }
    // a stale rid gets a full update
    let d = t.maindata(&sid, rid);
    assert_eq!(d.get("full_update"), Some(&Json::Bool(true)));
    let mut rid = d.get("rid").and_then(Json::as_i64).unwrap();
    assert!(rid > rid3);

    // per-torrent detail endpoints
    let p = t.get(&sid, &format!("/api/v2/torrents/properties?hash={hash}")).json();
    assert_eq!(s(p.get("comment")), "made for a test");
    assert_eq!(p.get("piece_size").and_then(Json::as_i64), Some(16 * 1024));
    assert_eq!(p.get("pieces_num").and_then(Json::as_i64), Some(4));
    assert_eq!(t.get(&sid, "/api/v2/torrents/properties?hash=00").status, 404);
    let files = t.get(&sid, &format!("/api/v2/torrents/files?hash={hash}")).json();
    let Json::Arr(files) = files else { panic!() };
    assert_eq!(files.len(), 2);
    assert_eq!(s(files[0].get("name")), "webui-test/a.bin");
    assert_eq!(files[1].get("size").and_then(Json::as_i64), Some(10_000));
    assert_eq!(files[1].get("priority").and_then(Json::as_i64), Some(1));
    let tr = t.get(&sid, &format!("/api/v2/torrents/trackers?hash={hash}")).json();
    let Json::Arr(tr) = tr else { panic!() };
    assert_eq!(s(tr[0].get("url")), "** [DHT] **");
    assert_eq!(tr.len(), 3);
    let pieces = t.get(&sid, &format!("/api/v2/torrents/pieceStates?hash={hash}")).json();
    assert_eq!(pieces, Json::Arr(vec![Json::Int(0); 4]));
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/filePrio", &format!("hash={hash}&id=1&priority=0")).status,
        200
    );
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/filePrio", &format!("hash={hash}&id=1&priority=3")).status,
        400
    );
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/filePrio", &format!("hash={hash}&id=9&priority=0")).status,
        409
    );
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/filePrio", &format!("hash={hash}&id=x&priority=0")).status,
        400
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let Json::Arr(f) = t.get(&sid, &format!("/api/v2/torrents/files?hash={hash}")).json() else {
            panic!()
        };
        if f[1].get("priority").and_then(Json::as_i64) == Some(0) {
            break;
        }
        assert!(Instant::now() < deadline, "file priority not applied");
        std::thread::sleep(Duration::from_millis(50));
    }

    // renameFile / renameFolder: refused names, then a file and the torrent's folder
    let rename = |what: &str, old: &str, new: &str| {
        let form = format!("hash={hash}&oldPath={}&newPath={}", enc(old), enc(new));
        t.post(&sid, &format!("/api/v2/torrents/{what}"), &form).status
    };
    let names = || -> Vec<String> {
        let Json::Arr(f) = t.get(&sid, &format!("/api/v2/torrents/files?hash={hash}")).json() else {
            panic!()
        };
        f.iter().map(|x| s(x.get("name")).to_owned()).collect()
    };
    let wait_names = |want: [&str; 2]| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while names() != want {
            assert!(Instant::now() < deadline, "rename not applied: {:?}", names());
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    assert_eq!(t.post(&sid, "/api/v2/torrents/renameFile", &format!("hash={hash}&oldPath=x")).status, 400);
    assert_eq!(rename("renameFile", "webui-test/b.bin", "webui-test/../b.bin"), 409);
    assert_eq!(rename("renameFile", "webui-test/b.bin", "/abs/b.bin"), 409);
    assert_eq!(rename("renameFile", "webui-test/nope.bin", "webui-test/c.bin"), 409);
    assert_eq!(rename("renameFile", "webui-test/b.bin", "webui-test/a.bin"), 409);
    assert_eq!(t.get(&sid, &format!("/api/v2/torrents/renameFile?hash={hash}")).status, 405);
    assert_eq!(rename("renameFile", "webui-test/b.bin", "webui-test/sub/b2.bin"), 200);
    wait_names(["webui-test/a.bin", "webui-test/sub/b2.bin"]);
    assert_eq!(rename("renameFolder", "webui-test/nope", "x"), 409);
    assert_eq!(rename("renameFolder", "webui-test", "renamed-root"), 200);
    wait_names(["renamed-root/a.bin", "renamed-root/sub/b2.bin"]);
    assert_eq!(rename("renameFolder", "renamed-root/sub", "renamed-root/deeper/sub"), 200);
    wait_names(["renamed-root/a.bin", "renamed-root/deeper/sub/b2.bin"]);

    // torrents/info parameters
    let all = t
        .get(&sid, "/api/v2/torrents/info?filter=all&category=tv&sort=name&reverse=true&limit=5&offset=0")
        .json();
    assert!(matches!(&all, Json::Arr(v) if v.len() == 1));
    let none = t.get(&sid, "/api/v2/torrents/info?category=movies").json();
    assert_eq!(none, Json::Arr(vec![]));
    let none = t.get(&sid, "/api/v2/torrents/info?filter=stopped").json();
    assert_eq!(none, Json::Arr(vec![]));

    // stop: the state changes, and the diff carries it
    assert_eq!(t.post(&sid, "/api/v2/torrents/stop", &format!("hashes={hash}")).status, 200);
    t.wait_for(&sid, &hash, "pausedDL", |x| s(x.get("state")) == "pausedDL");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let d = t.maindata(&sid, rid);
        assert_eq!(d.get("full_update"), None);
        rid = d.get("rid").and_then(Json::as_i64).unwrap();
        if s(d.get("torrents").and_then(|m| m.get(&hash)).and_then(|o| o.get("state"))) == "pausedDL" {
            break;
        }
        assert!(Instant::now() < deadline, "no state diff");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        matches!(t.get(&sid, "/api/v2/torrents/info?filter=stopped").json(), Json::Arr(v) if v.len() == 1)
    );

    assert_eq!(t.post(&sid, "/api/v2/torrents/start", &format!("hashes={hash}")).status, 200);
    t.wait_for(&sid, &hash, "running", |x| !s(x.get("state")).starts_with("paused"));

    // category changes show in the diff
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/setCategory", &format!("hashes={hash}&category=nope")).status,
        409
    );
    assert_eq!(t.post(&sid, "/api/v2/torrents/setCategory", &format!("hashes={hash}&category=")).status, 200);
    let d = t.maindata(&sid, rid);
    rid = d.get("rid").and_then(Json::as_i64).unwrap();
    assert_eq!(
        d.get("torrents").and_then(|m| m.get(&hash)).and_then(|o| o.get("category")),
        Some(&Json::from(""))
    );
    assert_eq!(t.post(&sid, "/api/v2/torrents/removeCategories", "categories=tv").status, 200);
    let d = t.maindata(&sid, rid);
    rid = d.get("rid").and_then(Json::as_i64).unwrap();
    assert_eq!(d.get("categories_removed"), Some(&Json::Arr(vec!["tv".into()])));

    // limits
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/setDownloadLimit", &format!("hashes={hash}&limit=2048")).status,
        200
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while t.post(&sid, "/api/v2/torrents/downloadLimit", &format!("hashes={hash}")).json().get(&hash)
        != Some(&Json::Int(2048))
    {
        assert!(Instant::now() < deadline, "limit not applied");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(t.post(&sid, "/api/v2/torrents/recheck", &format!("hashes={hash}")).status, 200);
    assert_eq!(t.post(&sid, "/api/v2/torrents/reannounce", &format!("hashes={hash}")).status, 200);

    // delete: torrents_removed
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/delete", &format!("hashes={hash}&deleteFiles=false")).status,
        200
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let d = t.maindata(&sid, rid);
        rid = d.get("rid").and_then(Json::as_i64).unwrap();
        if d.get("torrents_removed") == Some(&Json::Arr(vec![Json::from(hash.as_str())])) {
            break;
        }
        assert!(Instant::now() < deadline, "no torrents_removed");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(t.get(&sid, "/api/v2/torrents/info").json(), Json::Arr(vec![]));
}

#[test]
fn adds_magnets_and_refuses_bad_torrents() {
    let t = Harness::start("magnet");
    let sid = t.login();
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let urls = enc(&format!("magnet:?xt=urn:btih:{hash}&dn=from+a+magnet"));
    let r = t.post(&sid, "/api/v2/torrents/add", &format!("urls={urls}&stopped=true"));
    assert_eq!((r.status, r.text().as_str()), (200, "Ok."));
    let info = t.wait_for(&sid, hash, "the magnet", |x| s(x.get("state")) == "pausedDL");
    assert_eq!(s(info.get("name")), "from a magnet");
    // no metadata yet: no files to prioritize
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/filePrio", &format!("hash={hash}&id=0&priority=0")).status,
        409
    );

    let (body, boundary) = multipart(&[], b"not a torrent");
    let r = t.req("POST", "/api/v2/torrents/add", &[("Cookie", &sid)], Body::Multipart(&body, boundary));
    assert_eq!(r.status, 415);
    assert_eq!(t.post(&sid, "/api/v2/torrents/add", "urls=magnet%3A%3Fxt%3Dbad").text(), "Fails.");
    assert_eq!(t.post(&sid, "/api/v2/torrents/count", "").text(), "1");
}

#[test]
fn connection_handling() {
    let t = Harness::start("conn");
    let sid = t.login();
    let host = t.host();

    // keep-alive and pipelining: three requests in one write, three responses
    let mut s = TcpStream::connect(t.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut batch = Vec::new();
    for path in ["/api/v2/app/version", "/api/v2/app/webapiVersion"] {
        batch.extend(raw_request(&host, "GET", path, &[("Cookie", &sid)], Body::None, false));
    }
    batch.extend(raw_request(
        &host,
        "POST",
        "/api/v2/transfer/info",
        &[("Cookie", &sid)],
        Body::Form(""),
        true,
    ));
    s.write_all(&batch).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let (a, n1) = parse_response(&out);
    let (b, n2) = parse_response(&out[n1..]);
    let (c, _) = parse_response(&out[n1 + n2..]);
    assert_eq!((a.text().as_str(), b.text().as_str()), ("v5.0.0", "2.11.2"));
    assert_eq!(a.header("connection"), Some("keep-alive"));
    assert!(c.json().get("dl_info_speed").is_some());
    assert_eq!(c.header("connection"), Some("close"));

    // Expect: 100-continue gets the interim response before the body is sent
    let mut s = TcpStream::connect(t.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let body = "limit=1024";
    s.write_all(
        format!(
            "POST /api/v2/transfer/setDownloadLimit HTTP/1.1\r\nHost: {host}\r\nCookie: {sid}\r\n\
Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    )
    .unwrap();
    let mut first = [0u8; 25];
    s.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"HTTP/1.1 100 Continue\r\n\r\n");
    s.write_all(body.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    assert_eq!(parse_response(&out).0.status, 200);
    assert_eq!(t.get(&sid, "/api/v2/transfer/downloadLimit").text(), "1024");
    assert_eq!(t.post(&sid, "/api/v2/transfer/toggleSpeedLimitsMode", "").status, 200);
    assert_eq!(t.get(&sid, "/api/v2/transfer/speedLimitsMode").text(), "1");

    // limits: a large body before login, chunked bodies, a huge head, garbage
    let big = "x".repeat(70 * 1024);
    let r = t.req("POST", "/api/v2/auth/login", &[], Body::Form(&big));
    assert_eq!(r.status, 413);
    let mut s = TcpStream::connect(t.addr).unwrap();
    s.write_all(
        format!(
            "POST /api/v2/auth/login HTTP/1.1\r\nHost: {host}\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
        )
        .as_bytes(),
    )
    .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    assert_eq!(parse_response(&out).0.status, 501);
    let mut s = TcpStream::connect(t.addr).unwrap();
    let _ = s
        .write_all(format!("GET / HTTP/1.1\r\nHost: {host}\r\nX: {}\r\n\r\n", "y".repeat(20_000)).as_bytes());
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    assert_eq!(parse_response(&out).0.status, 431);
    let mut s = TcpStream::connect(t.addr).unwrap();
    s.write_all(b"\x16\x03\x01 garbage\r\n\r\n").unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    assert_eq!(parse_response(&out).0.status, 400);
}

#[test]
fn preferences_and_stored_credentials() {
    let dir = temp_dir("prefs");
    {
        let t = Harness::start_in(dir.clone(), Some(PASS));
        let sid = t.login();
        let p = t.get(&sid, "/api/v2/app/preferences").json();
        assert_eq!(p.get("dht"), Some(&Json::Bool(false)));
        assert_eq!(p.get("web_ui_csrf_protection_enabled"), Some(&Json::Bool(true)));
        assert_eq!(p.get("bypass_local_auth"), Some(&Json::Bool(false)));
        assert!(p.get("web_ui_password").is_none());
        let set = r#"{"dl_limit":100,"alt_up_limit":20,"save_path":"/data/new","web_ui_password":"another-one","no_such_key":1}"#;
        assert_eq!(t.post(&sid, "/api/v2/app/setPreferences", &format!("json={}", enc(set))).status, 200);
        assert_eq!(t.post(&sid, "/api/v2/app/setPreferences", "json=nope").status, 400);
        let p = t.get(&sid, "/api/v2/app/preferences").json();
        assert_eq!(p.get("dl_limit").and_then(Json::as_i64), Some(100));
        assert_eq!(p.get("alt_up_limit").and_then(Json::as_i64), Some(20));
        assert_eq!(t.get(&sid, "/api/v2/transfer/downloadLimit").text(), "102400");
        assert_eq!(t.get(&sid, "/api/v2/app/defaultSavePath").text(), "/data/new");
        let file = std::fs::read_to_string(t.dir.join("state").join(STATE_FILE)).unwrap();
        assert!(!file.contains(PASS) && !file.contains("another-one"), "{file}");
        assert!(file.contains("pbkdf2-sha256:"));
        // keep the directory for the second run: the harness removes this one instead
        let mut t = t;
        t.dir = temp_dir("prefs-scratch");
    }
    // restarted without a password: the stored hash and preferences are used
    let t = Harness::start_in(dir, None);
    assert!(t.web.temporary_password().is_none());
    let r = t.req("POST", "/api/v2/auth/login", &[], Body::Form("username=admin&password=another-one"));
    assert_eq!(r.text(), "Ok.");
    let sid = r.header("set-cookie").unwrap().split(';').next().unwrap().to_owned();
    assert_eq!(t.get(&sid, "/api/v2/app/defaultSavePath").text(), "/data/new");
    assert_eq!(t.get(&sid, "/api/v2/transfer/downloadLimit").text(), "102400");
}

#[test]
fn a_temporary_password_without_one_given() {
    let t = Harness::start_in(temp_dir("temp"), None);
    let p = t.web.temporary_password().expect("a temporary password").to_owned();
    let r = t.req("POST", "/api/v2/auth/login", &[], Body::Form(&format!("username=admin&password={p}")));
    assert_eq!(r.text(), "Ok.");
}

// ---------------------------------------------------------------------------------------------
// The native API: WebSocket and session protocol

/// The RFC 6455 section 1.3 example key and its accept value.
const WS_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const WS_ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

/// A raw WebSocket client: masked frames out, server frames in.
struct Ws {
    s: TcpStream,
    buf: Vec<u8>,
}

/// Sends the upgrade request and returns the response head (status, headers) and the socket.
fn ws_upgrade(t: &Harness, headers: &[(&str, &str)]) -> (Resp, TcpStream, Vec<u8>) {
    let mut s = TcpStream::connect(t.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut r = format!(
        "GET /pa/ws HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {WS_KEY}\r\n",
        t.host()
    );
    for (n, v) in headers {
        r.push_str(&format!("{n}: {v}\r\n"));
    }
    r.push_str("\r\n");
    s.write_all(r.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(p) = out.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        let n = s.read(&mut chunk).unwrap();
        assert!(n > 0, "closed before a response head: {}", String::from_utf8_lossy(&out));
        out.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&out[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .map_or(0, |(_, v)| v.parse().unwrap());
    let mut body = out[end..].to_vec();
    if status != 101 {
        while body.len() < len {
            let n = s.read(&mut chunk).unwrap();
            assert!(n > 0);
            body.extend_from_slice(&chunk[..n]);
        }
        return (Resp { status, headers, body }, s, Vec::new());
    }
    (Resp { status, headers, body: Vec::new() }, s, body)
}

impl Ws {
    /// Upgrades with the session cookie and a same-origin `Origin`, and reads the `hello`.
    fn connect(t: &Harness, sid: &str) -> (Ws, Json) {
        let origin = format!("http://{}", t.host());
        let (r, s, rest) =
            ws_upgrade(t, &[("Cookie", sid), ("Origin", &origin), ("Sec-WebSocket-Version", "13")]);
        assert_eq!(r.status, 101, "{}", r.text());
        assert_eq!(r.header("sec-websocket-accept"), Some(WS_ACCEPT));
        assert_eq!(r.header("upgrade").map(str::to_ascii_lowercase).as_deref(), Some("websocket"));
        let mut ws = Ws { s, buf: rest };
        let hello = ws.recv_json();
        assert_eq!(hello.get("t").and_then(Json::as_str), Some("hello"));
        (ws, hello)
    }

    fn send_frame(&mut self, fin: bool, opcode: u8, payload: &[u8], masked: bool) {
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode];
        let m = if masked { 0x80 } else { 0 };
        let len = payload.len();
        if len < 126 {
            out.push(m | len as u8);
        } else if len <= u16::MAX as usize {
            out.push(m | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(m | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        if masked {
            out.extend_from_slice(&mask);
            out.extend(payload.iter().enumerate().map(|(i, &b)| b ^ mask[i % 4]));
        } else {
            out.extend_from_slice(payload);
        }
        self.s.write_all(&out).unwrap();
    }

    fn send_text(&mut self, text: &str) {
        self.send_frame(true, 1, text.as_bytes(), true);
    }

    /// The next server frame: (opcode, payload); `None` on a clean EOF.
    fn recv_frame(&mut self) -> Option<(u8, Vec<u8>)> {
        let mut chunk = [0u8; 65536];
        loop {
            if self.buf.len() >= 2 {
                let b0 = self.buf[0];
                assert!(b0 & 0x80 != 0, "fragmented server frames are not expected");
                assert_eq!(self.buf[1] & 0x80, 0, "server frames must not be masked");
                let (len, at) = match self.buf[1] & 0x7f {
                    n @ 0..=125 => (n as usize, 2),
                    126 if self.buf.len() >= 4 => {
                        (u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize, 4)
                    }
                    127 if self.buf.len() >= 10 => {
                        (u64::from_be_bytes(self.buf[2..10].try_into().unwrap()) as usize, 10)
                    }
                    _ => (usize::MAX, 0),
                };
                if at > 0 && self.buf.len() >= at + len {
                    let payload = self.buf[at..at + len].to_vec();
                    self.buf.drain(..at + len);
                    return Some((b0 & 0x0f, payload));
                }
            }
            match self.s.read(&mut chunk) {
                Ok(0) => return None,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) => panic!("read: {e}"),
            }
        }
    }

    /// The next server frame that is not a `tick`. Once a view is subscribed the status ticks
    /// come on their own schedule, so a reply a test waits for may have one in front of it.
    fn recv_frame_skipping_ticks(&mut self) -> Option<(u8, Vec<u8>)> {
        loop {
            let f = self.recv_frame();
            if let Some((1, payload)) = &f {
                if is_tick(payload) {
                    continue;
                }
            }
            return f;
        }
    }

    /// The next text frame as JSON (pings are answered on the way), ticks included.
    fn recv_message(&mut self) -> Json {
        loop {
            match self.recv_frame() {
                Some((1, payload)) => return parse_text(payload),
                Some((9, payload)) => self.send_frame(true, 10, &payload, true),
                Some((op, payload)) => panic!("unexpected frame {op}: {payload:?}"),
                None => panic!("closed while waiting for a message"),
            }
        }
    }

    /// The next text frame as JSON that is not a `tick` (pings are answered on the way): the
    /// reply to what was just sent.
    fn recv_json(&mut self) -> Json {
        loop {
            match self.recv_frame_skipping_ticks() {
                Some((1, payload)) => return parse_text(payload),
                Some((9, payload)) => self.send_frame(true, 10, &payload, true),
                Some((op, payload)) => panic!("unexpected frame {op}: {payload:?}"),
                None => panic!("closed while waiting for a message"),
            }
        }
    }

    /// The next text frame and its size on the wire (the payload length).
    fn recv_sized(&mut self) -> (Json, usize) {
        loop {
            match self.recv_frame() {
                Some((1, payload)) => {
                    let n = payload.len();
                    let text = String::from_utf8(payload).unwrap();
                    return (json::parse(&text).unwrap_or_else(|e| panic!("{e}: {text}")), n);
                }
                Some((9, payload)) => self.send_frame(true, 10, &payload, true),
                Some((op, _)) => panic!("unexpected frame {op}"),
                None => panic!("closed while waiting for a message"),
            }
        }
    }

    /// Ticks until `f` accepts one; panics after 20 s.
    fn tick_until(&mut self, what: &str, mut f: impl FnMut(&Json) -> bool) -> Json {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let m = self.recv_message();
            if m.get("t").and_then(Json::as_str) == Some("tick") && f(&m) {
                return m;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; last: {}", m.to_json());
        }
    }

    /// Asserts that no frame arrives for `d`.
    fn expect_silence(&mut self, d: Duration) {
        self.s.set_read_timeout(Some(d)).unwrap();
        let mut chunk = [0u8; 64];
        let r = self.s.read(&mut chunk);
        self.s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        match r {
            Ok(n) => panic!("{n} bytes arrived while idle: {:?}", &chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => panic!("read: {e}"),
        }
        assert!(self.buf.is_empty(), "buffered: {:?}", self.buf);
    }
}

/// Reads ticks (and rows) until the cells for `hash` satisfy `f`; the cells are the view's
/// columns in order. Cells arrive only when a row changed, so the last ones seen are kept.
fn cells_until(
    ws: &mut Ws,
    seen: &mut std::collections::HashMap<String, Vec<String>>,
    hash: &str,
    what: &str,
    f: impl Fn(&[String], &str) -> bool,
) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(c) = seen.get(hash) {
            let state = seen.get(&format!("{hash}:state")).map_or("", |v| v[0].as_str()).to_owned();
            if f(c, &state) {
                return c.clone();
            }
        }
        let m = ws.recv_message();
        assert!(s(m.get("t")) != "error", "{}", m.to_json());
        if let Some(cells) = m.get("cells") {
            for (h, cell) in cells.entries() {
                let Some(Json::Arr(c)) = cell.get("c") else { continue };
                seen.insert(h.clone(), c.iter().map(|x| s(Some(x)).to_owned()).collect());
                seen.insert(format!("{h}:state"), vec![s(cell.get("state")).to_owned()]);
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}; cells: {:?}", seen.get(hash));
    }
}

fn ids_of(m: &Json) -> Vec<String> {
    match m.get("ids") {
        Some(Json::Arr(v)) => v.iter().map(|x| s(Some(x)).to_owned()).collect(),
        _ => Vec::new(),
    }
}

fn cells_of(m: &Json) -> Vec<String> {
    m.get("cells").map(|c| c.entries().iter().map(|(k, _)| k.clone()).collect()).unwrap_or_default()
}

#[test]
fn websocket_handshake_and_frames() {
    let t = Harness::start("ws");
    let sid = t.login();
    let (mut ws, hello) = Ws::connect(&t, &sid);
    // hello: the 24 columns with ids, titles and widths, the categories, no labels yet
    let Some(Json::Arr(cols)) = hello.get("columns") else { panic!("{}", hello.to_json()) };
    assert_eq!(cols.len(), 24);
    assert_eq!(s(cols[0].get("id")), "name");
    assert_eq!(s(cols[0].get("title")), "Name");
    assert_eq!(cols[0].get("width").and_then(Json::as_i64), Some(220));
    assert_eq!(s(cols[0].get("align")), "left");
    assert_eq!(s(cols[7].get("id")), "down_speed");
    assert_eq!(s(cols[7].get("align")), "right");
    assert!(!s(hello.get("version")).is_empty());
    let Some(Json::Arr(cats)) = hello.get("categories") else { panic!() };
    assert_eq!(cats.len(), 5);
    assert_eq!(s(cats[0].get("id")), "all");
    assert_eq!(hello.get("labels"), Some(&Json::Arr(vec![])));

    // a masked text frame round trip: a subscription answered with rows
    ws.send_text(r#"{"t":"view","columns":["name"],"window":[0,10]}"#);
    let rows = ws.recv_json();
    assert_eq!(s(rows.get("t")), "rows");
    assert_eq!(rows.get("total"), Some(&Json::Int(0)));
    // a fragmented message is assembled
    let msg = br#"{"t":"nothing"}"#;
    ws.send_frame(false, 1, &msg[..5], true);
    ws.send_frame(true, 0, &msg[5..], true);
    let err = ws.recv_json();
    assert_eq!(s(err.get("t")), "error");
    assert!(s(err.get("msg")).contains("nothing"), "{}", err.to_json());
    ws.send_text("not json");
    assert_eq!(s(ws.recv_json().get("t")), "error");

    // ping answered with pong, payload echoed
    ws.send_frame(true, 9, b"are you there", true);
    assert_eq!(ws.recv_frame_skipping_ticks(), Some((10, b"are you there".to_vec())));

    // a 16-bit length frame
    let long = format!(r#"{{"t":"view","filter":{{"search":"{}"}},"columns":["name"]}}"#, "x".repeat(300));
    ws.send_text(&long);
    assert_eq!(s(ws.recv_json().get("t")), "rows");

    // the close handshake: our 1000 is echoed, then the socket ends
    ws.send_frame(true, 8, &[0x03, 0xe8, b'b', b'y', b'e'], true);
    let close = loop {
        match ws.recv_frame() {
            Some((8, p)) => break p,
            Some((1, _)) => continue, // a tick that was on its way
            other => panic!("expected a close frame, got {other:?}"),
        }
    };
    assert_eq!(&close[..2], &[0x03, 0xe8]);
    assert_eq!(ws.recv_frame(), None);

    // an unmasked client frame is a protocol error: close 1002
    let (mut ws, _) = Ws::connect(&t, &sid);
    ws.send_frame(true, 1, br#"{"t":"idle"}"#, false);
    let close = loop {
        match ws.recv_frame() {
            Some((8, p)) => break p,
            Some((1, _)) => continue,
            other => panic!("expected a close frame, got {other:?}"),
        }
    };
    assert_eq!(u16::from_be_bytes([close[0], close[1]]), 1002);
    // a binary message: close 1003
    let (mut ws, _) = Ws::connect(&t, &sid);
    ws.send_frame(true, 2, b"\x00\x01", true);
    let close = loop {
        match ws.recv_frame() {
            Some((8, p)) => break p,
            Some((1, _)) => continue,
            other => panic!("expected a close frame, got {other:?}"),
        }
    };
    assert_eq!(u16::from_be_bytes([close[0], close[1]]), 1003);
    // a message over the limit: close 1009
    let (mut ws, _) = Ws::connect(&t, &sid);
    ws.send_frame(true, 1, &vec![b' '; 65 * 1024], true);
    let close = loop {
        match ws.recv_frame() {
            Some((8, p)) => break p,
            Some((1, _)) => continue,
            other => panic!("expected a close frame, got {other:?}"),
        }
    };
    assert_eq!(u16::from_be_bytes([close[0], close[1]]), 1009);
}

#[test]
fn websocket_upgrade_is_refused_without_origin_or_session() {
    let t = Harness::start("ws-refuse");
    let sid = t.login();
    let origin = format!("http://{}", t.host());
    // a bad Origin: 403
    let (r, _, _) = ws_upgrade(
        &t,
        &[("Cookie", &sid), ("Origin", "http://evil.example"), ("Sec-WebSocket-Version", "13")],
    );
    assert_eq!(r.status, 403, "{}", r.text());
    let (r, _, _) = ws_upgrade(&t, &[("Cookie", &sid), ("Origin", "null"), ("Sec-WebSocket-Version", "13")]);
    assert_eq!(r.status, 403);
    // no session: 403
    let (r, _, _) = ws_upgrade(&t, &[("Origin", &origin), ("Sec-WebSocket-Version", "13")]);
    assert_eq!(r.status, 403);
    let (r, _, _) =
        ws_upgrade(&t, &[("Cookie", "SID=forged"), ("Origin", &origin), ("Sec-WebSocket-Version", "13")]);
    assert_eq!(r.status, 403);
    // a wrong version: 426 naming ours
    let (r, _, _) = ws_upgrade(&t, &[("Cookie", &sid), ("Origin", &origin), ("Sec-WebSocket-Version", "8")]);
    assert_eq!(r.status, 426);
    assert_eq!(r.header("sec-websocket-version"), Some("13"));
    // a plain GET of the path: 400
    let plain = t.req("GET", "/pa/ws", &[("Cookie", &sid)], Body::None);
    assert_eq!(plain.status, 400);
    // no Origin at all (a script): accepted, like the CSRF rule for requests
    let (r, _, _) = ws_upgrade(&t, &[("Cookie", &sid), ("Sec-WebSocket-Version", "13")]);
    assert_eq!(r.status, 101);
    // a logged-out session ends the socket
    let (mut ws, _) = Ws::connect(&t, &sid);
    ws.send_text(r#"{"t":"view","columns":["name"]}"#);
    assert_eq!(s(ws.recv_json().get("t")), "rows");
    assert_eq!(t.post(&sid, "/api/v2/auth/logout", "").status, 200);
    let close = loop {
        match ws.recv_frame() {
            Some((8, p)) => break p,
            Some((1, _)) => continue,
            other => panic!("expected a close frame, got {other:?}"),
        }
    };
    assert_eq!(u16::from_be_bytes([close[0], close[1]]), 1008);
}

#[test]
fn native_view_rows_and_tick_diffs() {
    let t = Harness::start("native");
    let sid = t.login();
    let (bytes, hash) = torrent();
    let (body, boundary) = multipart(&[("paused", "false")], &bytes);
    let r = t.req("POST", "/api/v2/torrents/add", &[("Cookie", &sid)], Body::Multipart(&body, boundary));
    assert_eq!(r.status, 200);
    let magnet_hash = "0123456789abcdef0123456789abcdef01234567";
    let urls = enc(&format!("magnet:?xt=urn:btih:{magnet_hash}&dn=from+a+magnet"));
    assert_eq!(t.post(&sid, "/api/v2/torrents/add", &format!("urls={urls}&stopped=true")).status, 200);
    // past the initial check, so the Status cell reads Downloading
    t.wait_for(&sid, &hash, "the added torrent", |x| {
        s(x.get("name")) == "webui-test" && matches!(s(x.get("state")), "downloading" | "stalledDL")
    });
    t.wait_for(&sid, magnet_hash, "the magnet", |x| s(x.get("state")) == "pausedDL");

    let (mut ws, _) = Ws::connect(&t, &sid);
    ws.send_text(
        r#"{"t":"view","filter":{"category":"all","label":null,"search":""},"sort":["name",false],
"columns":["name","size","done","status","down_speed","save_as"],"window":[0,60]}"#,
    );
    let rows = ws.recv_json();
    assert_eq!(s(rows.get("t")), "rows");
    assert_eq!(rows.get("total"), Some(&Json::Int(2)));
    assert_eq!(rows.get("first"), Some(&Json::Int(0)));
    assert_eq!(ids_of(&rows), [magnet_hash.to_owned(), hash.clone()], "sorted by name");
    let cell = rows.get("cells").unwrap().get(&hash).expect("the torrent's cells");
    let Some(Json::Arr(c)) = cell.get("c") else { panic!("{}", cell.to_json()) };
    assert_eq!(s(Some(&c[0])), "webui-test");
    assert_eq!(s(Some(&c[1])), "48.8 kB");
    assert_eq!(s(Some(&c[2])), "0.0 %");
    assert!(s(Some(&c[3])).contains("Downloading"), "{}", c[3].to_json());
    assert_eq!(s(Some(&c[4])), "", "idle speed cells are blank");
    assert!(s(Some(&c[5])).ends_with("dl"), "{}", c[5].to_json());
    assert_eq!(cell.get("done"), Some(&Json::Int(0)));
    assert_eq!(s(cell.get("state")), "downloading");
    assert!(cell.get("v").and_then(Json::as_i64).is_some());
    let magnet = rows.get("cells").unwrap().get(magnet_hash).unwrap();
    assert_eq!(s(magnet.get("state")), "stopped");
    let Some(Json::Arr(mc)) = magnet.get("c") else { panic!() };
    assert_eq!(s(Some(&mc[3])), "Stopped");

    // ticks come once per status tick with counts and the status bar
    let tick = ws.tick_until("a tick", |_| true);
    let counts = tick.get("counts").unwrap();
    assert_eq!(counts.get("all"), Some(&Json::Int(2)));
    assert_eq!(counts.get("downloading"), Some(&Json::Int(2)));
    assert_eq!(counts.get("completed"), Some(&Json::Int(0)));
    let status = tick.get("status").unwrap();
    assert_eq!(s(status.get("down")), "0.0 kB/s");
    assert_eq!(s(status.get("down_total")), "0 B");
    assert!(status.get("dht").and_then(Json::as_i64).is_some());
    assert!(tick.get("ids").is_none(), "the order did not move: {}", tick.to_json());

    // a status change: a tick carrying that row, and never the unchanged magnet
    assert_eq!(t.post(&sid, "/api/v2/torrents/stop", &format!("hashes={hash}")).status, 200);
    let tick = ws.tick_until("the stopped row", |m| {
        let cells = cells_of(m);
        assert!(!cells.iter().any(|c| c == magnet_hash), "the unchanged row was sent: {}", m.to_json());
        m.get("cells").and_then(|c| c.get(&hash)).is_some_and(|c| s(c.get("state")) == "stopped")
    });
    let c = tick.get("cells").unwrap().get(&hash).unwrap();
    let Some(Json::Arr(c)) = c.get("c") else { panic!() };
    assert_eq!(s(Some(&c[3])), "Stopped");

    // a column change re-sends the window with the new cells
    ws.send_text(r#"{"t":"view","sort":["name",false],"columns":["status","name"],"window":[1,1]}"#);
    let rows = ws.recv_json();
    assert_eq!(s(rows.get("t")), "rows");
    assert_eq!(rows.get("first"), Some(&Json::Int(1)));
    assert_eq!(ids_of(&rows), std::slice::from_ref(&hash));
    let Some(Json::Arr(c)) = rows.get("cells").unwrap().get(&hash).unwrap().get("c") else { panic!() };
    assert_eq!(c.len(), 2);
    assert_eq!(s(Some(&c[1])), "webui-test");

    // the category filter and a search narrow the view
    ws.send_text(r#"{"t":"view","filter":{"category":"completed"},"columns":["name"]}"#);
    let rows = ws.recv_json();
    assert_eq!(rows.get("total"), Some(&Json::Int(0)));
    ws.send_text(r#"{"t":"view","filter":{"search":"MAGNET"},"columns":["name"]}"#);
    let rows = ws.recv_json();
    assert_eq!(rows.get("total"), Some(&Json::Int(1)));
    assert_eq!(ids_of(&rows), [magnet_hash.to_owned()]);
    ws.send_text(r#"{"t":"view","filter":{"category":"nope"},"columns":["name"]}"#);
    assert_eq!(s(ws.recv_json().get("t")), "error");

    // a label set through the shim's categories shows in the Label column and in hello
    assert_eq!(t.post(&sid, "/api/v2/torrents/createCategory", "category=linux").status, 200);
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/setCategory", &format!("hashes={hash}&category=linux")).status,
        200
    );
    ws.send_text(r#"{"t":"view","filter":{"label":"linux"},"columns":["name","label"]}"#);
    let rows = ws.recv_json();
    assert_eq!(ids_of(&rows), std::slice::from_ref(&hash));
    let Some(Json::Arr(c)) = rows.get("cells").unwrap().get(&hash).unwrap().get("c") else { panic!() };
    assert_eq!(s(Some(&c[1])), "linux");
    let (_, hello) = Ws::connect(&t, &sid);
    assert_eq!(hello.get("labels"), Some(&Json::Arr(vec!["linux".into()])));

    // the detail subscription is accepted (content comes with slice 4), idle stops the ticks
    ws.send_text(&format!(r#"{{"t":"detail","id":"{hash}","tab":"peers"}}"#));
    ws.send_text(r#"{"t":"idle"}"#);
    // ticks already on the wire may still arrive; after a moment there must be none
    std::thread::sleep(Duration::from_millis(200));
    ws.s.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    let mut chunk = [0u8; 65536];
    loop {
        match ws.s.read(&mut chunk) {
            Ok(0) => panic!("closed"),
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    ws.buf.clear();
    ws.expect_silence(Duration::from_millis(400));
    // a view after idle answers with rows and the ticks resume
    ws.send_text(r#"{"t":"view","columns":["name"]}"#);
    assert_eq!(s(ws.recv_json().get("t")), "rows");
    ws.tick_until("ticks after idle", |_| true);

    // a removal leaves the view
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/delete", &format!("hashes={hash}&deleteFiles=false")).status,
        200
    );
    ws.tick_until("the total after removal", |m| m.get("total") == Some(&Json::Int(1)));
}

#[test]
fn native_sort_by_speed_resends_the_order_when_it_moves() {
    // 40 synthetic torrents, all force-started: their rates random-walk every tick
    let t = Harness::start_mock("native-sort", 40, 1.0, -1);
    let sid = t.login();
    let (mut ws, _) = Ws::connect(&t, &sid);
    ws.send_text(
        r#"{"t":"view","sort":["down_speed",true],"columns":["name","down_speed"],"window":[0,10]}"#,
    );
    let rows = loop {
        let m = ws.recv_json();
        if s(m.get("t")) == "rows" && m.get("total") == Some(&Json::Int(40)) {
            break m;
        }
        if s(m.get("t")) == "rows" {
            // the first update had not landed yet: ask again once it has (ticks pass by)
            std::thread::sleep(Duration::from_millis(50));
            ws.send_text(
                r#"{"t":"view","sort":["down_speed",true],"columns":["name","down_speed"],"window":[0,10]}"#,
            );
        }
    };
    let mut order = ids_of(&rows);
    assert_eq!(order.len(), 10);
    let mut moved = 0;
    let mut stayed = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while moved < 3 && Instant::now() < deadline {
        let tick = ws.tick_until("a tick", |_| true);
        let ids = ids_of(&tick);
        if ids.is_empty() {
            stayed += 1;
            continue;
        }
        assert_ne!(ids, order, "ids sent although the order stayed: {}", tick.to_json());
        assert_eq!(ids.len(), 10);
        // rows new to the window come with their cells; the rest only when they changed
        for id in &ids {
            if !order.contains(id) {
                assert!(
                    cells_of(&tick).contains(id),
                    "a row new to the window without cells: {}",
                    tick.to_json()
                );
            }
        }
        order = ids;
        moved += 1;
    }
    assert_eq!(moved, 3, "the order by speed never moved ({stayed} ticks without ids)");
    // sorted by name, the order never moves
    ws.send_text(r#"{"t":"view","sort":["name",false],"columns":["name","down_speed"],"window":[0,10]}"#);
    assert_eq!(s(ws.recv_json().get("t")), "rows");
    for _ in 0..5 {
        let tick = ws.tick_until("a tick", |_| true);
        assert!(tick.get("ids").is_none(), "{}", tick.to_json());
    }
}

#[test]
fn native_idle_client_gets_nothing() {
    let t = Harness::start_mock("native-idle", 10, 1.0, -1);
    let sid = t.login();
    let (mut ws, _) = Ws::connect(&t, &sid);
    // without a view: nothing either
    ws.expect_silence(Duration::from_millis(300));
    ws.send_text(r#"{"t":"view","columns":["name","down_speed"],"window":[0,10]}"#);
    assert_eq!(s(ws.recv_json().get("t")), "rows");
    ws.tick_until("a tick", |_| true);
    ws.send_text(r#"{"t":"idle"}"#);
    // drain what was in flight, then two ticks' worth of silence (the harness ticks every 50 ms)
    ws.s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    let mut chunk = [0u8; 65536];
    while ws.s.read(&mut chunk).is_ok_and(|n| n > 0) {}
    ws.buf.clear();
    ws.expect_silence(Duration::from_millis(500));
}

#[test]
fn native_ten_thousand_torrent_profile_fits_the_budgets() {
    // The desktop's scale profile: 10,000 synthetic torrents, nothing running.
    let t = Harness::start_mock("native-10k", 10_000, 0.0, 0);
    let sid = t.login();
    let (mut ws, _) = Ws::connect(&t, &sid);
    let all: Vec<&str> = vec![
        "name",
        "queue_position",
        "size",
        "done",
        "status",
        "seeds",
        "peers",
        "down_speed",
        "up_speed",
        "eta",
        "uploaded",
        "downloaded",
        "ratio",
        "availability",
        "label",
        "added_on",
        "completed_on",
        "tracker",
        "down_limit",
        "up_limit",
        "remaining",
        "seeds_peers",
        "save_as",
        "wasted",
    ];
    let cols: Vec<String> = all.iter().map(|c| format!("\"{c}\"")).collect();
    let view = format!(
        r#"{{"t":"view","filter":{{"category":"all"}},"sort":["name",false],"columns":[{}],"window":[0,60]}}"#,
        cols.join(",")
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    ws.send_text(&view);
    let (rows, size) = loop {
        let (m, n) = ws.recv_sized();
        let total = m.get("total").and_then(Json::as_i64).unwrap_or(0);
        if s(m.get("t")) == "rows" && total == 10_000 {
            break (m, n);
        }
        assert!(Instant::now() < deadline, "the 10,000 torrents never arrived: {total}");
        if s(m.get("t")) == "rows" {
            // the first update had not landed yet: ask again once it has
            std::thread::sleep(Duration::from_millis(100));
            ws.send_text(&view);
        }
    };
    let ids = ids_of(&rows);
    assert_eq!(ids.len(), 60);
    assert_eq!(rows.get("cells").unwrap().entries().len(), 60);
    let Some(Json::Arr(c)) = rows.get("cells").unwrap().get(&ids[0]).unwrap().get("c") else { panic!() };
    assert_eq!(c.len(), 24);
    eprintln!("10,000 torrents: rows message with 60 rows and 24 columns: {size} bytes");
    assert!(size < 30 * 1024, "a 60-row, 24-column rows message is {size} bytes");
    // a tick with nothing changed: no ids, no cells, under 1 KB
    let deadline = Instant::now() + Duration::from_secs(20);
    let (tick, size) = loop {
        let (m, n) = ws.recv_sized();
        assert_eq!(s(m.get("t")), "tick");
        if m.get("cells").is_none() && m.get("ids").is_none() {
            break (m, n);
        }
        assert!(Instant::now() < deadline, "every tick carried changes: {}", m.to_json());
    };
    assert_eq!(tick.get("counts").unwrap().get("all"), Some(&Json::Int(10_000)));
    eprintln!("10,000 torrents: idle tick: {size} bytes");
    assert!(size < 1024, "an idle tick is {size} bytes: {}", tick.to_json());
    // scrolling to the end: the window clamps
    let view = view.replace("\"window\":[0,60]", "\"window\":[9970,60]");
    ws.send_text(&view);
    let (rows, _) = ws.recv_sized();
    assert_eq!(s(rows.get("t")), "rows");
    assert_eq!(rows.get("first"), Some(&Json::Int(9970)));
    assert_eq!(ids_of(&rows).len(), 30);
}

impl Ws {
    /// Messages until a `detail` one for `tab` arrives (ticks pass by); panics after 20 s.
    fn detail_until(&mut self, tab: &str) -> Json {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let m = self.recv_json();
            if s(m.get("t")) == "detail" && s(m.get("tab")) == tab {
                return m;
            }
            assert!(s(m.get("t")) != "error", "{}", m.to_json());
            assert!(Instant::now() < deadline, "timed out waiting for the {tab} tab; last: {}", m.to_json());
        }
    }

    /// After an unsubscribe: messages already in flight may be anything (at most `inflight` of
    /// them), then `quiet` ticks in a row must come with no detail message between them.
    fn expect_only_ticks(&mut self, inflight: usize, quiet: usize) {
        let mut n = 0;
        let mut ticks = 0;
        while ticks < quiet {
            let m = self.recv_message();
            n += 1;
            if s(m.get("t")) == "tick" {
                ticks += 1;
            } else {
                assert!(n <= inflight, "message {n} after the unsubscribe: {}", m.to_json());
                ticks = 0;
            }
        }
    }
}

fn pairs_of(m: &Json, group: &str) -> Vec<(String, String)> {
    let Some(Json::Arr(groups)) = m.get("groups") else { panic!("{}", m.to_json()) };
    let g = groups.iter().find(|g| s(g.get("title")) == group).unwrap_or_else(|| panic!("{}", m.to_json()));
    let Some(Json::Arr(rows)) = g.get("rows") else { panic!() };
    rows.iter()
        .map(|r| match r {
            Json::Arr(v) => (s(v.first()).to_owned(), s(v.get(1)).to_owned()),
            _ => panic!(),
        })
        .collect()
}

fn value_of(pairs: &[(String, String)], label: &str) -> String {
    pairs.iter().find(|(l, _)| l == label).map(|(_, v)| v.clone()).unwrap_or_else(|| panic!("no {label}"))
}

fn detail_rows(m: &Json) -> Vec<Vec<String>> {
    let Some(Json::Arr(rows)) = m.get("rows") else { panic!("no rows: {}", m.to_json()) };
    rows.iter()
        .map(|r| match r.get("c") {
            Some(Json::Arr(c)) => c.iter().map(|x| s(Some(x)).to_owned()).collect(),
            _ => panic!("{}", r.to_json()),
        })
        .collect()
}

#[test]
fn native_detail_tabs_against_the_engine() {
    let t = Harness::start("detail");
    let sid = t.login();
    // a tracker nothing listens on: the Trackers tab gets a row with an error status
    let (bytes, hash) = torrent_with(Some("http://127.0.0.1:9/announce"));
    let (body, boundary) = multipart(&[("paused", "false")], &bytes);
    let r = t.req("POST", "/api/v2/torrents/add", &[("Cookie", &sid)], Body::Multipart(&body, boundary));
    assert_eq!(r.status, 200);
    t.wait_for(&sid, &hash, "the added torrent", |x| {
        s(x.get("name")) == "webui-test" && matches!(s(x.get("state")), "downloading" | "stalledDL")
    });
    let (mut ws, hello) = Ws::connect(&t, &sid);
    let defs = hello.get("detail").expect("the detail column definitions");
    assert_eq!(defs.get("files").map(|v| matches!(v, Json::Arr(v) if v.len() == 7)), Some(true));
    ws.send_text(r#"{"t":"view","columns":["name"],"window":[0,10]}"#);
    assert_eq!(s(ws.recv_json().get("t")), "rows");

    // General: at once, formatted by the frontend
    ws.send_text(&format!(r#"{{"t":"detail","id":"{hash}","tab":"general"}}"#));
    let m = ws.detail_until("general");
    assert_eq!(s(m.get("id")), hash);
    let g = pairs_of(&m, "General");
    assert_eq!(value_of(&g, "Total Size:"), "48.8 kB");
    assert_eq!(value_of(&g, "Hash:"), hash.to_uppercase());
    assert_eq!(value_of(&g, "Comment:"), "made for a test");
    assert_eq!(value_of(&g, "Created By:"), "the test");
    assert_eq!(value_of(&g, "Created On:"), "2000-02-29 00:00:00");
    assert!(value_of(&g, "Pieces:").starts_with("4 x 16.0 kB"), "{}", value_of(&g, "Pieces:"));
    assert!(value_of(&g, "Save As:").ends_with("dl"));
    let tr = pairs_of(&m, "Transfer");
    assert_eq!(value_of(&tr, "Download Speed:"), "0.0 kB/s");
    assert_eq!(value_of(&tr, "Share Ratio:"), "0.000");
    assert!(value_of(&tr, "Status:").starts_with("Downloading"), "{}", value_of(&tr, "Status:"));
    let Some(Json::Arr(bars)) = m.get("bars") else { panic!() };
    assert_eq!(bars[0], Json::Arr(vec!["Downloaded:".into(), "0.0 %".into(), 0i64.into()]));

    // Trackers: the tracker's row, status from the engine's attempt
    ws.send_text(&format!(r#"{{"t":"detail","id":"{hash}","tab":"trackers"}}"#));
    let m = ws.detail_until("trackers");
    let rows = detail_rows(&m);
    assert_eq!(rows.len(), 1, "{}", m.to_json());
    assert_eq!(rows[0][0], "http://127.0.0.1:9/announce");
    assert_eq!(rows[0].len(), 6);

    // Files: both files, their sizes, done, percent, pieces, priority, and the engine index
    ws.send_text(&format!(r#"{{"t":"detail","id":"{hash}","tab":"files"}}"#));
    let m = ws.detail_until("files");
    let rows = detail_rows(&m);
    assert_eq!(rows.len(), 2, "{}", m.to_json());
    assert_eq!(rows[0], ["webui-test/a.bin", "39.1 kB", "0 B", "0.0 %", "3", "0", "Normal"]);
    assert_eq!(rows[1][0], "webui-test/b.bin");
    assert_eq!(rows[1][1], "9.77 kB");
    let Some(Json::Arr(raw)) = m.get("rows") else { panic!() };
    assert_eq!(raw[1].get("i"), Some(&Json::Int(1)));
    assert_eq!(raw[1].get("p"), Some(&Json::Int(0)));
    assert_eq!(raw[1].get("prio"), Some(&Json::Int(4)));
    // a priority change through the shim shows in the next update
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/filePrio", &format!("hash={hash}&id=1&priority=0")).status,
        200
    );
    let m = ws.detail_until("files");
    let rows = detail_rows(&m);
    assert_eq!(rows[1][6], "Don't Download", "{}", m.to_json());

    // Pieces: four pieces, none had, none downloading (no peers)
    ws.send_text(&format!(r#"{{"t":"detail","id":"{hash}","tab":"pieces"}}"#));
    let m = ws.detail_until("pieces");
    assert_eq!(m.get("n"), Some(&Json::Int(4)));
    assert_eq!(m.get("have"), Some(&Json::Int(0)));
    assert_eq!(m.get("rle"), Some(&Json::Arr(vec![Json::Int(4)])));
    assert_eq!(m.get("downloading"), Some(&Json::Arr(vec![])));

    // Speed: the ring whole (empty, with the axis), then one sample per tick
    ws.send_text(&format!(r#"{{"t":"detail","id":"{hash}","tab":"speed"}}"#));
    let m = ws.detail_until("speed");
    assert_eq!(m.get("all"), Some(&Json::Bool(true)));
    assert_eq!(m.get("keep"), Some(&Json::Int(600)));
    let scale = m.get("scale").expect("the axis");
    assert_eq!(scale.get("top"), Some(&Json::Int(10 * 1024)));
    assert_eq!(
        s(scale.get("labels").and_then(|l| if let Json::Arr(v) = l { v.first() } else { None })),
        "10.0 kB/s"
    );
    let m = ws.detail_until("speed");
    assert_eq!(m.get("all"), None, "{}", m.to_json());
    // one sample per tick, but a slow machine can run two ticks before the update goes out
    let Some(Json::Arr(samples)) = m.get("samples") else { panic!("{}", m.to_json()) };
    assert!(!samples.is_empty(), "{}", m.to_json());
    assert!(samples.iter().all(|x| *x == Json::Arr(vec![Json::Int(0), Json::Int(0)])), "{}", m.to_json());
    assert_eq!(
        m.get("legend"),
        Some(&Json::Arr(vec!["Download: 0.0 kB/s".into(), "Upload: 0.0 kB/s".into()]))
    );

    // Logger: the session's lines (listening ...) and the torrent's (added), as [time, text]
    ws.send_text(&format!(r#"{{"t":"detail","id":"{hash}","tab":"logger"}}"#));
    let m = ws.detail_until("logger");
    assert_eq!(m.get("all"), Some(&Json::Bool(true)));
    let Some(Json::Arr(lines)) = m.get("lines") else { panic!() };
    let texts: Vec<&str> =
        lines.iter().map(|l| if let Json::Arr(v) = l { s(v.get(1)) } else { "" }).collect();
    assert!(texts.iter().any(|l| l.starts_with("listening on")), "{texts:?}");
    assert!(texts.contains(&"webui-test: added"), "{texts:?}");
    assert!(
        lines.iter().all(|l| matches!(l, Json::Arr(v) if v[0].as_i64().is_some_and(|t| t > 1_700_000_000)))
    );

    // a switch back to General sends it once more, at once, and the Logger tab stops
    ws.send_text(&format!(r#"{{"t":"detail","id":"{hash}","tab":"general"}}"#));
    let m = ws.detail_until("general");
    assert!(m.get("groups").is_some());
    // the unsubscribe: ticks keep coming, detail messages do not (the General tab would
    // otherwise send one every second, as Time Elapsed moves)
    ws.send_text(r#"{"t":"detail","id":null}"#);
    ws.expect_only_ticks(3, 12);
    // an unknown torrent: nothing (the pane stays empty), ticks go on
    ws.send_text(r#"{"t":"detail","id":"0000000000000000000000000000000000000000","tab":"peers"}"#);
    ws.expect_only_ticks(0, 6);
    // removed while subscribed: said once
    ws.send_text(&format!(r#"{{"t":"detail","id":"{hash}","tab":"pieces"}}"#));
    ws.detail_until("pieces");
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/delete", &format!("hashes={hash}&deleteFiles=false")).status,
        200
    );
    let m = ws.detail_until("pieces");
    assert_eq!(m.get("gone"), Some(&Json::Bool(true)), "{}", m.to_json());
}

#[test]
fn peers_tab_and_sync_torrent_peers_on_the_mock() {
    // the simulated engine answers peer queries for its active torrents
    let t = Harness::start_mock("peers", 20, 1.0, -1);
    let sid = t.login();
    // the torrent with the most connected peers, once the first status update landed
    let deadline = Instant::now() + Duration::from_secs(20);
    let hash = loop {
        let list = t.get(&sid, "/api/v2/torrents/info?sort=num_leechs&reverse=true&limit=1").json();
        let Json::Arr(list) = list else { panic!() };
        let connected = |k: &str| list.first().and_then(|x| x.get(k)).and_then(Json::as_i64).unwrap_or(0);
        if connected("num_leechs") + connected("num_seeds") > 0 {
            break s(list[0].get("hash")).to_owned();
        }
        assert!(Instant::now() < deadline, "no torrent with peers");
        std::thread::sleep(Duration::from_millis(50));
    };

    // the shim: a full update at rid 0, then a diff with only changed fields
    let p = t.get(&sid, &format!("/api/v2/sync/torrentPeers?hash={hash}&rid=0")).json();
    assert_eq!(p.get("full_update"), Some(&Json::Bool(true)));
    assert_eq!(p.get("show_flags"), Some(&Json::Bool(true)));
    let rid = p.get("rid").and_then(Json::as_i64).unwrap();
    let peers = p.get("peers").expect("peers");
    assert!(!peers.entries().is_empty(), "{}", p.to_json());
    let (addr, first) = &peers.entries()[0];
    assert!(addr.contains(':'), "{addr}");
    assert_eq!(
        s(first.get("ip")).to_owned() + ":" + &first.get("port").and_then(Json::as_i64).unwrap().to_string(),
        *addr
    );
    for f in ["client", "connection", "flags", "progress", "dl_speed", "up_speed", "downloaded", "uploaded"] {
        assert!(first.get(f).is_some(), "{f} in {}", first.to_json());
    }
    let p2 = t.get(&sid, &format!("/api/v2/sync/torrentPeers?hash={hash}&rid={rid}")).json();
    assert_eq!(p2.get("full_update"), None, "{}", p2.to_json());
    assert!(p2.get("rid").and_then(Json::as_i64).unwrap() > rid);
    if let Some(diff) = p2.get("peers").and_then(|p| p.get(addr)) {
        assert!(diff.get("client").is_none(), "unchanged fields are not re-sent: {}", diff.to_json());
    }
    assert_eq!(t.get(&sid, "/api/v2/sync/torrentPeers?hash=00&rid=0").status, 404);

    // the Peers tab: the frontend's nine columns with the µTorrent flag letters
    let (mut ws, _) = Ws::connect(&t, &sid);
    ws.send_text(&format!(r#"{{"t":"detail","id":"{hash}","tab":"peers"}}"#));
    let m = ws.detail_until("peers");
    let rows = detail_rows(&m);
    assert!(!rows.is_empty(), "{}", m.to_json());
    assert_eq!(rows[0].len(), 9);
    assert!(rows.iter().any(|r| r[1].contains(' ')), "a client name: {rows:?}");
    assert!(rows.iter().all(|r| r[2].chars().all(|c| "DdUuOSIEePHXL".contains(c))), "{rows:?}");
    let Some(Json::Arr(raw)) = m.get("rows") else { panic!() };
    assert!(
        raw[0].get("k").is_some_and(|k| matches!(k, Json::Arr(v) if v.len() == 9 && v[3].as_i64().is_some()))
    );
}

#[test]
fn native_commands_against_the_engine() {
    let t = Harness::start("commands");
    let sid = t.login();
    let (a_bytes, a) = torrent_named("alpha", None);
    let (b_bytes, b) = torrent_named("bravo", None);
    let (c_bytes, c) = torrent_named("charlie", None);
    // data on disk for bravo, so "remove with data" has something to delete
    let bravo_dir = t.dir.join("dl").join("bravo");
    std::fs::create_dir_all(&bravo_dir).unwrap();
    std::fs::write(bravo_dir.join("a.bin"), vec![1u8; 40_000]).unwrap();
    for bytes in [&a_bytes, &b_bytes, &c_bytes] {
        let r = t.pa_upload(&sid, "/pa/add", r#"{"start":true}"#, bytes);
        assert_eq!((r.status, r.text().as_str()), (204, ""));
    }
    for h in [&a, &b, &c] {
        t.wait_for(&sid, h, "the added torrent", |x| {
            x.get("priority").is_some_and(|q| q.as_i64().unwrap_or(0) >= 1)
        });
    }

    // the cells of a view sorted by name: name, status, #, Down Limit, Up Limit, Label
    let (mut ws, _) = Ws::connect(&t, &sid);
    ws.send_text(r#"{"t":"view","sort":["name",false],"columns":["name","status","queue_position","down_limit","up_limit","label"],"window":[0,10]}"#);
    let rows = ws.recv_json();
    assert_eq!(s(rows.get("t")), "rows");
    assert_eq!(ids_of(&rows), [a.clone(), b.clone(), c.clone()]);
    let mut seen = std::collections::HashMap::new();
    for (h, cell) in rows.get("cells").unwrap().entries() {
        let Some(Json::Arr(cc)) = cell.get("c") else { panic!() };
        seen.insert(h.clone(), cc.iter().map(|x| s(Some(x)).to_owned()).collect::<Vec<_>>());
        seen.insert(format!("{h}:state"), vec![s(cell.get("state")).to_owned()]);
    }
    let a_cells = cells_until(&mut ws, &mut seen, &a, "alpha running", |c, _| {
        c[1] == "Downloading" || c[1] == "Queued"
    });
    assert_eq!(a_cells[2], "1", "queue positions follow the add order: {a_cells:?}");

    // Pause shows as Paused (the user-paused record), Stop as Stopped, Start runs again
    let r = t.pa(&sid, "/pa/torrents/pause", &format!(r#"{{"ids":["{a}"]}}"#));
    assert_eq!((r.status, r.text().as_str()), (204, ""));
    cells_until(&mut ws, &mut seen, &a, "Paused", |c, state| c[1] == "Paused" && state == "paused");
    assert_eq!(
        t.pa(&sid, "/pa/torrents/stop", &format!(r#"{{"ids":["{}"]}}"#, a.to_uppercase())).status,
        204
    );
    cells_until(&mut ws, &mut seen, &a, "Stopped", |c, state| c[1] == "Stopped" && state == "stopped");
    assert_eq!(t.pa(&sid, "/pa/torrents/start", &format!(r#"{{"ids":["{a}"]}}"#)).status, 204);
    cells_until(&mut ws, &mut seen, &a, "running again", |c, state| {
        c[1] != "Stopped" && c[1] != "Paused" && state != "stopped"
    });
    // the shim's stop is µTorrent's Stop too, and clears a Pause
    assert_eq!(t.pa(&sid, "/pa/torrents/pause", &format!(r#"{{"ids":["{a}"]}}"#)).status, 204);
    cells_until(&mut ws, &mut seen, &a, "Paused", |c, _| c[1] == "Paused");
    assert_eq!(t.post(&sid, "/api/v2/torrents/stop", &format!("hashes={a}")).status, 200);
    cells_until(&mut ws, &mut seen, &a, "Stopped by the shim", |c, _| c[1] == "Stopped");
    assert_eq!(t.pa(&sid, "/pa/torrents/force-start", &format!(r#"{{"ids":["{a}"]}}"#)).status, 204);
    cells_until(&mut ws, &mut seen, &a, "[F]", |c, _| c[1].starts_with("[F] "));
    assert_eq!(t.pa(&sid, "/pa/torrents/start", &format!(r#"{{"ids":["{a}"]}}"#)).status, 204);
    cells_until(&mut ws, &mut seen, &a, "back in the queue", |c, _| !c[1].starts_with("[F]"));

    // queue moves on three torrents: charlie to the top, then bravo down below it
    assert_eq!(t.pa(&sid, "/pa/torrents/queue", &format!(r#"{{"ids":["{c}"],"move":"top"}}"#)).status, 204);
    cells_until(&mut ws, &mut seen, &c, "charlie first", |c, _| c[2] == "1");
    cells_until(&mut ws, &mut seen, &a, "alpha second", |c, _| c[2] == "2");
    cells_until(&mut ws, &mut seen, &b, "bravo third", |c, _| c[2] == "3");
    assert_eq!(
        t.pa(&sid, "/pa/torrents/queue", &format!(r#"{{"ids":["{a}","{b}"],"move":"up"}}"#)).status,
        204
    );
    cells_until(&mut ws, &mut seen, &a, "alpha first", |c, _| c[2] == "1");
    cells_until(&mut ws, &mut seen, &b, "bravo second", |c, _| c[2] == "2");
    cells_until(&mut ws, &mut seen, &c, "charlie last", |c, _| c[2] == "3");
    assert_eq!(
        t.pa(&sid, "/pa/torrents/queue", &format!(r#"{{"ids":["{a}"],"move":"bottom"}}"#)).status,
        204
    );
    cells_until(&mut ws, &mut seen, &a, "alpha last", |c, _| c[2] == "3");
    assert_eq!(
        t.pa(&sid, "/pa/torrents/queue", &format!(r#"{{"ids":["{a}"],"move":"sideways"}}"#)).status,
        400
    );

    // limits read back in the cells (and through the shim)
    assert_eq!(
        t.pa(&sid, "/pa/torrents/limits", &format!(r#"{{"ids":["{a}"],"down":2048,"up":1024}}"#)).status,
        204
    );
    cells_until(&mut ws, &mut seen, &a, "limits", |c, _| c[3] == "2.0 kB/s" && c[4] == "1.0 kB/s");
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/uploadLimit", &format!("hashes={a}")).json().get(&a),
        Some(&Json::Int(1024))
    );
    assert_eq!(t.pa(&sid, "/pa/torrents/limits", &format!(r#"{{"ids":["{a}"],"down":0}}"#)).status, 204);
    cells_until(&mut ws, &mut seen, &a, "unlimited down", |c, _| c[3].is_empty() && c[4] == "1.0 kB/s");
    assert_eq!(t.pa(&sid, "/pa/torrents/limits", &format!(r#"{{"ids":["{a}"]}}"#)).status, 400);
    assert_eq!(t.pa(&sid, "/pa/torrents/limits", &format!(r#"{{"ids":["{a}"],"down":-5}}"#)).status, 400);

    // a label (created on the fly) in the cell and the shim's categories
    assert_eq!(t.pa(&sid, "/pa/torrents/label", &format!(r#"{{"ids":["{a}"],"label":"tv"}}"#)).status, 204);
    cells_until(&mut ws, &mut seen, &a, "label", |c, _| c[5] == "tv");
    assert!(t.get(&sid, "/api/v2/torrents/categories").json().get("tv").is_some());
    assert_eq!(t.pa(&sid, "/pa/torrents/label", &format!(r#"{{"ids":["{a}"],"label":"/bad"}}"#)).status, 400);

    // "all" with the view's filter: only the labelled torrent stops
    assert_eq!(
        t.pa(
            &sid,
            "/pa/torrents/stop",
            r#"{"all":true,"filter":{"category":"all","label":"tv","search":""}}"#
        )
        .status,
        204
    );
    cells_until(&mut ws, &mut seen, &a, "alpha stopped by the filter", |c, _| c[1] == "Stopped");
    let b_cells = cells_until(&mut ws, &mut seen, &b, "bravo", |_, _| true);
    assert_ne!(b_cells[1], "Stopped", "bravo has no label: {b_cells:?}");
    // a search that matches nothing is fine; every torrent otherwise
    assert_eq!(t.pa(&sid, "/pa/torrents/stop", r#"{"all":true,"filter":{"search":"zzz"}}"#).status, 204);
    assert_eq!(t.pa(&sid, "/pa/torrents/pause", r#"{"all":true,"filter":{"search":"ARL"}}"#).status, 204);
    cells_until(&mut ws, &mut seen, &c, "charlie paused by the search", |c, _| c[1] == "Paused");
    assert_eq!(t.pa(&sid, "/pa/torrents/start", r#"{"all":true}"#).status, 204);
    cells_until(&mut ws, &mut seen, &c, "charlie running", |c, _| c[1] != "Paused");
    cells_until(&mut ws, &mut seen, &a, "alpha running", |c, _| c[1] != "Stopped");

    // super seeding, recheck, reannounce, location: accepted
    assert_eq!(t.pa(&sid, "/pa/torrents/super-seed", &format!(r#"{{"ids":["{a}"],"on":true}}"#)).status, 204);
    assert_eq!(t.pa(&sid, "/pa/torrents/recheck", &format!(r#"{{"ids":["{a}"]}}"#)).status, 204);
    assert_eq!(t.pa(&sid, "/pa/torrents/reannounce", &format!(r#"{{"ids":["{a}"]}}"#)).status, 204);
    let moved = t.dir.join("moved").to_string_lossy().replace('\\', "\\\\");
    assert_eq!(
        t.pa(&sid, "/pa/torrents/location", &format!(r#"{{"ids":["{c}"],"path":"{moved}"}}"#)).status,
        204
    );
    t.wait_for(&sid, &c, "charlie moved", |x| s(x.get("save_path")).contains("moved"));
    assert_eq!(t.pa(&sid, "/pa/torrents/location", &format!(r#"{{"ids":["{c}"],"path":" "}}"#)).status, 400);

    // errors: an unknown id, a missing command, a body that is not JSON
    let r = t.pa(&sid, "/pa/torrents/start", r#"{"ids":["0000000000000000000000000000000000000000"]}"#);
    assert_eq!(r.status, 404);
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(s(r.json().get("error")), "no such torrent");
    assert_eq!(t.pa(&sid, "/pa/torrents/explode", &format!(r#"{{"ids":["{a}"]}}"#)).status, 404);
    assert_eq!(t.pa(&sid, "/pa/torrents/start", "nope").status, 400);
    assert_eq!(t.pa(&sid, "/pa/torrents/start", "{}").status, 400);
    assert_eq!(
        t.req(
            "POST",
            "/pa/torrents/start",
            &[("Cookie", &sid), ("X-Requested-With", "PaTorrent")],
            Body::Form("ids=x")
        )
        .status,
        415
    );
    assert_eq!(t.get(&sid, "/pa/torrents/start").status, 405);

    // remove with data: bravo's files go
    assert!(bravo_dir.join("a.bin").exists());
    assert_eq!(t.pa(&sid, "/pa/torrents/remove", &format!(r#"{{"ids":["{b}"],"data":true}}"#)).status, 204);
    let deadline = Instant::now() + Duration::from_secs(20);
    while bravo_dir.join("a.bin").exists() {
        assert!(Instant::now() < deadline, "bravo's data was not deleted");
        std::thread::sleep(Duration::from_millis(50));
    }
    t.wait_for_gone(&sid, &b);
    assert_eq!(
        t.pa(&sid, "/pa/torrents/remove", &format!(r#"{{"ids":["{a}","{c}"],"data":false}}"#)).status,
        204
    );
    t.wait_for_gone(&sid, &a);
    t.wait_for_gone(&sid, &c);
}

#[test]
fn shim_queue_moves_and_set_location() {
    let t = Harness::start("shim-queue");
    let sid = t.login();
    let (a_bytes, a) = torrent_named("alpha", None);
    let (b_bytes, b) = torrent_named("bravo", None);
    let (c_bytes, c) = torrent_named("charlie", None);
    for bytes in [&a_bytes, &b_bytes, &c_bytes] {
        assert_eq!(t.pa_upload(&sid, "/pa/add", r#"{"start":true}"#, bytes).status, 204);
    }
    let prio = |h: &str, want: i64, what: &str| {
        t.wait_for(&sid, h, what, |x| x.get("priority").and_then(Json::as_i64) == Some(want));
    };
    prio(&a, 1, "alpha first");
    prio(&b, 2, "bravo second");
    prio(&c, 3, "charlie third");

    // topPrio / bottomPrio / increasePrio / decreasePrio, several hashes keep their order
    assert_eq!(t.post(&sid, "/api/v2/torrents/topPrio", &format!("hashes={c}")).status, 200);
    prio(&c, 1, "charlie to the top");
    prio(&a, 2, "alpha below it");
    prio(&b, 3, "bravo last");
    assert_eq!(t.post(&sid, "/api/v2/torrents/increasePrio", &format!("hashes={a}|{b}")).status, 200);
    prio(&a, 1, "alpha up");
    prio(&b, 2, "bravo up");
    prio(&c, 3, "charlie down");
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/bottomPrio", &format!("hashes={}", a.to_uppercase())).status,
        200
    );
    prio(&a, 3, "alpha to the bottom");
    assert_eq!(t.post(&sid, "/api/v2/torrents/decreasePrio", &format!("hashes={b}")).status, 200);
    prio(&b, 2, "bravo down");
    prio(&c, 1, "charlie first again");
    // all: the order stays (every torrent moves the same way)
    assert_eq!(t.post(&sid, "/api/v2/torrents/topPrio", "hashes=all").status, 200);
    prio(&c, 1, "charlie still first");
    prio(&b, 2, "bravo still second");
    prio(&a, 3, "alpha still last");
    // POST only; an unknown hash is no error
    assert_eq!(t.get(&sid, "/api/v2/torrents/topPrio?hashes=all").status, 405);
    assert_eq!(
        t.post(&sid, "/api/v2/torrents/topPrio", "hashes=0000000000000000000000000000000000000000").status,
        200
    );

    // setLocation: creates the directory, moves the files
    let moved = t.dir.join("moved").join("deeper");
    let r = t.post(
        &sid,
        "/api/v2/torrents/setLocation",
        &format!("hashes={a}|{b}&location={}", enc(&moved.to_string_lossy())),
    );
    assert_eq!((r.status, r.text().as_str()), (200, ""));
    assert!(moved.is_dir());
    for h in [&a, &b] {
        t.wait_for(&sid, h, "moved", |x| s(x.get("save_path")).contains("deeper"));
    }
    let c_info = t.wait_for(&sid, &c, "charlie", |_| true);
    assert!(!s(c_info.get("save_path")).contains("deeper"), "charlie stays: {}", c_info.to_json());
    // an empty location is 400, a directory that cannot be created 409 (a file is in the way)
    assert_eq!(t.post(&sid, "/api/v2/torrents/setLocation", &format!("hashes={c}&location=")).status, 400);
    assert_eq!(t.post(&sid, "/api/v2/torrents/setLocation", &format!("hashes={c}&location=%20")).status, 400);
    let blocker = t.dir.join("a-file");
    std::fs::write(&blocker, b"x").unwrap();
    let r = t.post(
        &sid,
        "/api/v2/torrents/setLocation",
        &format!("hashes={c}&location={}", enc(&blocker.join("sub").to_string_lossy())),
    );
    assert_eq!(r.status, 409);
    assert_eq!(t.get(&sid, &format!("/api/v2/torrents/setLocation?hashes={c}&location=x")).status, 405);
}

/// The ready magnets in `GET /pa/add`, as (hash, name).
fn ready_adds(t: &Harness, sid: &str) -> Vec<(String, String)> {
    let setup = t.get(sid, "/pa/add").json();
    let Some(Json::Arr(v)) = setup.get("ready") else { panic!("{}", setup.to_json()) };
    v.iter().map(|r| (s(r.get("hash")).to_owned(), s(r.get("name")).to_owned())).collect()
}

fn wait_ready(t: &Harness, sid: &str, hash: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready_adds(t, sid).iter().any(|(h, _)| h == hash) {
        assert!(Instant::now() < deadline, "the magnet's metadata never arrived");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn native_magnet_waits_for_the_add_dialog() {
    // the simulated engine fetches a magnet's metadata after a few ticks
    let t = Harness::start_mock("native-magnet-dialog", 0, 0.0, 0);
    let sid = t.login();
    let hash = "1123456789abcdef0123456789abcdef01234567";
    let body = format!(r#"{{"url":"magnet:?xt=urn:btih:{hash}&dn=waiting","dialog":true}}"#);
    let r = t.pa(&sid, "/pa/add", &body);
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(s(r.json().get("hash")), hash);
    t.wait_for(&sid, hash, "the waiting magnet", |_| true);
    assert_eq!(t.pa(&sid, "/pa/add", &body).status, 409, "already in the list");
    // nothing to show before the metadata, and an unknown hash
    assert_eq!(t.get(&sid, "/pa/torrent-info?hash=ffff").status, 404);
    wait_ready(&t, &sid, hash);
    let info = t.get(&sid, &format!("/pa/torrent-info?hash={hash}")).json();
    let Some(Json::Arr(files)) = info.get("files") else { panic!("{}", info.to_json()) };
    assert!(!files.is_empty());
    // in upload mode while it waits: nothing downloaded
    let x = t.wait_for(&sid, hash, "status", |_| true);
    assert_eq!(x.get("downloaded"), Some(&Json::Int(0)), "{}", x.to_json());

    // OK: the choices go to the torrent in the session
    let save = t.dir.join("chosen").to_string_lossy().replace('\\', "\\\\");
    let ok = format!(r#"{{"hash":"{hash}","save":"{save}","label":"tv","start":false}}"#);
    let r = t.pa(&sid, "/pa/add", &ok);
    assert_eq!((r.status, r.text().as_str()), (204, ""));
    assert!(ready_adds(&t, &sid).is_empty());
    assert_eq!(t.pa(&sid, "/pa/add", &ok).status, 404, "answered once");
    let x = t.wait_for(&sid, hash, "the chosen folder and label", |x| {
        s(x.get("save_path")).contains("chosen")
            && s(x.get("category")) == "tv"
            && s(x.get("state")) == "pausedDL"
    });
    assert_eq!(x.get("force_start"), Some(&Json::Bool(false)));

    // Cancel removes a waiting magnet
    let other = "2223456789abcdef0123456789abcdef01234567";
    let r = t.pa(&sid, "/pa/add", &format!(r#"{{"url":"magnet:?xt=urn:btih:{other}","dialog":true}}"#));
    assert_eq!(r.status, 200);
    wait_ready(&t, &sid, other);
    let r = t.pa(&sid, "/pa/add", &format!(r#"{{"hash":"{other}","cancel":true}}"#));
    assert_eq!(r.status, 204);
    t.wait_for_gone(&sid, other);
    assert!(ready_adds(&t, &sid).is_empty());
}

#[test]
fn native_add_dialog_flow_and_magnet() {
    let t = Harness::start("native-add");
    let sid = t.login();
    let (bytes, hash) = torrent();

    // step one: the Add dialog's Torrent Contents, nothing added
    let r = t.pa_upload(&sid, "/pa/torrent-info", "{}", &bytes);
    assert_eq!(r.status, 200, "{}", r.text());
    let info = r.json();
    assert_eq!(s(info.get("name")), "webui-test");
    assert_eq!(s(info.get("comment")), "made for a test");
    assert_eq!(s(info.get("created_by")), "the test");
    assert_eq!(info.get("size"), Some(&Json::Int(50_000)));
    assert_eq!(s(info.get("size_text")), "48.8 kB");
    assert_eq!(info.get("date"), Some(&Json::Int(951_782_400)));
    assert_eq!(s(info.get("date_text")), "2000-02-29 00:00:00");
    assert_eq!(s(info.get("hash")), hash);
    let Some(Json::Arr(files)) = info.get("files") else { panic!("{}", info.to_json()) };
    assert_eq!(files.len(), 2);
    assert_eq!(
        (files[0].get("i"), s(files[0].get("path")), files[0].get("size")),
        (Some(&Json::Int(0)), "a.bin", Some(&Json::Int(40_000)))
    );
    assert_eq!((s(files[1].get("path")), s(files[1].get("text"))), ("b.bin", "9.77 kB"));
    assert_eq!(t.get(&sid, "/api/v2/torrents/info").json(), Json::Arr(vec![]), "torrent-info adds nothing");
    let r = t.pa_upload(&sid, "/pa/torrent-info", "{}", b"not a torrent");
    assert_eq!(r.status, 415);

    // the dialog's defaults: the Save In list and the labels
    let setup = t.get(&sid, "/pa/add").json();
    let Some(Json::Arr(dirs)) = setup.get("save_in") else { panic!("{}", setup.to_json()) };
    assert_eq!(dirs.len(), 1);
    assert!(s(dirs.first()).ends_with("dl"));
    assert_eq!(setup.get("labels"), Some(&Json::Arr(vec![])));
    assert_eq!(setup.get("start"), Some(&Json::Bool(true)));

    // step two: add with a new name, one file off, a label, stopped, into another folder
    let save = t.dir.join("elsewhere").to_string_lossy().replace('\\', "\\\\");
    let options = format!(
        r#"{{"save":"{save}","name":"renamed","label":"iso","start":false,"skip_check":false,"top":true,"priorities":{{"1":0,"0":7}}}}"#
    );
    let r = t.pa_upload(&sid, "/pa/add", &options, &bytes);
    assert_eq!((r.status, r.text().as_str()), (204, ""));
    // stopped: paused outside the queue (a torrent file waits in checkingResumeData until it
    // is started, as in libtorrent; a magnet has nothing to check and is pausedDL)
    let info = t.wait_for(&sid, &hash, "the added torrent", |x| {
        matches!(s(x.get("state")), "pausedDL" | "checkingResumeData") && s(x.get("category")) == "iso"
    });
    assert_eq!(info.get("force_start"), Some(&Json::Bool(false)));
    assert!(s(info.get("save_path")).contains("elsewhere"), "{}", info.to_json());
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let Json::Arr(f) = t.get(&sid, &format!("/api/v2/torrents/files?hash={hash}")).json() else {
            panic!()
        };
        let names: Vec<&str> = f.iter().map(|x| s(x.get("name"))).collect();
        let prios: Vec<i64> =
            f.iter().map(|x| x.get("priority").and_then(Json::as_i64).unwrap_or(-1)).collect();
        if names == ["renamed/a.bin", "renamed/b.bin"] && prios == [7, 0] {
            break;
        }
        assert!(Instant::now() < deadline, "renamed root and priorities not applied: {names:?} {prios:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    // the folder is remembered first in the Save In list
    let setup = t.get(&sid, "/pa/add").json();
    let Some(Json::Arr(dirs)) = setup.get("save_in") else { panic!() };
    assert_eq!(dirs.len(), 2);
    assert!(s(dirs.get(1)).contains("elsewhere"), "{}", setup.to_json());
    assert_eq!(setup.get("labels"), Some(&Json::Arr(vec!["iso".into()])));
    // the same torrent again is refused by the engine, not the API; a wrong priority index is
    let r = t.pa_upload(&sid, "/pa/add", r#"{"priorities":{"7":1}}"#, &bytes);
    assert_eq!(r.status, 400);
    assert_eq!(t.pa_upload(&sid, "/pa/add", r#"{"start":"maybe"}"#, &bytes).status, 400);
    assert_eq!(t.pa_upload(&sid, "/pa/add", "{}", b"junk").status, 415);

    // a magnet through the JSON body, stopped
    let magnet_hash = "0123456789abcdef0123456789abcdef01234567";
    let body = format!(
        r#"{{"url":"magnet:?xt=urn:btih:{magnet_hash}&dn=from+a+magnet","start":false,"label":"iso"}}"#
    );
    let r = t.pa(&sid, "/pa/add", &body);
    assert_eq!((r.status, r.text().as_str()), (204, ""));
    let info = t.wait_for(&sid, magnet_hash, "the magnet", |x| s(x.get("state")) == "pausedDL");
    assert_eq!(s(info.get("name")), "from a magnet");
    assert_eq!(s(info.get("category")), "iso");
    let r = t.pa(&sid, "/pa/add", r#"{"url":"magnet:?xt=bad"}"#);
    assert_eq!(r.status, 400);
    assert_eq!(s(r.json().get("error")), "invalid magnet link");
    assert_eq!(t.pa(&sid, "/pa/add", r#"{}"#).status, 400);
    assert_eq!(t.post(&sid, "/api/v2/torrents/count", "").text(), "2");

    // the session limits, read back natively and through the shim
    assert_eq!(t.pa(&sid, "/pa/session/limits", r#"{"down":4096,"up":2048}"#).status, 204);
    let l = t.get(&sid, "/pa/session/limits").json();
    assert_eq!(
        (l.get("down"), l.get("up"), l.get("alt")),
        (Some(&Json::Int(4096)), Some(&Json::Int(2048)), Some(&Json::Bool(false)))
    );
    assert_eq!(t.get(&sid, "/api/v2/transfer/downloadLimit").text(), "4096");
    assert_eq!(t.pa(&sid, "/pa/session/limits", r#"{"alt":true,"down":512}"#).status, 204);
    let l = t.get(&sid, "/pa/session/limits").json();
    assert_eq!(
        (l.get("alt"), l.get("alt_down"), l.get("down")),
        (Some(&Json::Bool(true)), Some(&Json::Int(512)), Some(&Json::Int(4096)))
    );
    assert_eq!(t.get(&sid, "/api/v2/transfer/speedLimitsMode").text(), "1");
    assert_eq!(t.get(&sid, "/api/v2/transfer/downloadLimit").text(), "512");
    assert_eq!(t.pa(&sid, "/pa/session/limits", r#"{"down":"lots"}"#).status, 400);

    // shutdown
    assert!(!t.web.shutdown_requested());
    assert_eq!(t.pa(&sid, "/pa/session/shutdown", "").status, 204);
    assert!(t.web.shutdown_requested());
}

#[test]
fn native_commands_are_csrf_checked() {
    let t = Harness::start("native-csrf");
    let sid = t.login();
    let host = t.host();
    let origin = format!("http://{host}");
    // the custom header is required, with a same-origin Origin when one is sent
    let r = t.req(
        "POST",
        "/pa/torrents/start",
        &[("Cookie", &sid), ("Origin", &origin)],
        Body::Json(r#"{"all":true}"#),
    );
    assert_eq!(r.status, 403, "{}", r.text());
    assert!(s(r.json().get("error")).contains("X-Requested-With"), "{}", r.text());
    let r = t.req(
        "POST",
        "/pa/torrents/start",
        &[("Cookie", &sid), ("Origin", "http://evil.example"), ("X-Requested-With", "PaTorrent")],
        Body::Json(r#"{"all":true}"#),
    );
    assert_eq!(r.status, 403);
    assert_eq!(s(r.json().get("error")), "origin not allowed");
    let r = t.req(
        "POST",
        "/pa/torrents/start",
        &[("Cookie", &sid), ("Origin", "null"), ("X-Requested-With", "PaTorrent")],
        Body::Json(r#"{"all":true}"#),
    );
    assert_eq!(r.status, 403);
    // no session: forbidden before anything is parsed
    let r = t.req(
        "POST",
        "/pa/torrents/start",
        &[("X-Requested-With", "PaTorrent")],
        Body::Json(r#"{"all":true}"#),
    );
    assert_eq!(r.status, 403);
    assert_eq!(s(r.json().get("error")), "forbidden");
    assert_eq!(t.req("GET", "/pa/session/limits", &[], Body::None).status, 403);
    // the real thing passes, header case and all
    let r = t.req(
        "POST",
        "/pa/torrents/start",
        &[("Cookie", &sid), ("Origin", &origin), ("x-requested-with", "patorrent")],
        Body::Json(r#"{"all":true}"#),
    );
    assert_eq!(r.status, 204, "{}", r.text());
    // and without any Origin (curl) the header alone does
    assert_eq!(
        t.req(
            "POST",
            "/pa/torrents/start",
            &[("Cookie", &sid), ("X-Requested-With", "PaTorrent")],
            Body::Json(r#"{"all":true}"#)
        )
        .status,
        204
    );
    // the shim's rule is unchanged: Origin alone, 401 when wrong
    let r = t.req(
        "POST",
        "/api/v2/torrents/stop",
        &[("Cookie", &sid), ("Origin", "http://evil.example")],
        Body::Form("hashes=all"),
    );
    assert_eq!(r.status, 401);
}

// ---------------------------------------------------------------------------------------------
// Preferences (slice 6): GET / POST /pa/prefs

#[test]
fn native_prefs_model_and_partial_post() {
    let t = Harness::start("prefs-native");
    let sid = t.login();
    assert_eq!(t.req("GET", "/pa/prefs", &[], Body::None).status, 403);
    let r = t.get(&sid, "/pa/prefs");
    assert_eq!(r.status, 200);
    let m = r.json();
    let Some(Json::Arr(pages)) = m.get("pages") else { panic!("{}", m.to_json()) };
    let ids: Vec<&str> = pages.iter().map(|p| s(p.get("id"))).collect();
    assert_eq!(
        ids,
        [
            "general",
            "ui",
            "directories",
            "connection",
            "bandwidth",
            "bittorrent",
            "queueing",
            "scheduler",
            "webui",
            "advanced",
            "diskcache"
        ]
    );
    let fields = m.get("fields").expect("fields");
    let Some(Json::Arr(advanced)) = m.get("advanced") else { panic!() };
    assert!(advanced.len() > 100);
    for id in advanced {
        assert!(fields.get(id.as_str().unwrap()).is_some(), "{} has no field", id.to_json());
    }
    let field = |id: &str| fields.get(id).unwrap_or_else(|| panic!("no field {id}")).clone();
    // the harness starts with DHT off; the default is on
    assert_eq!(field("enable_dht").get("value"), Some(&Json::Bool(false)));
    assert_eq!(field("enable_dht").get("default"), Some(&Json::Bool(true)));
    assert_eq!(s(field("enable_dht").get("label")), "Enable DHT Network");
    assert_eq!(s(field("enable_dht").get("type")), "bool");
    assert_eq!(s(field("webui.listen").get("value")), t.host());
    assert_eq!(field("webui.listen").get("readonly"), Some(&Json::Bool(true)));
    assert_eq!(s(field("dir.download").get("value")), t.dir.join("dl").to_string_lossy());
    assert_eq!(s(field("webui.password").get("type")), "password");
    assert_eq!(s(field("webui.password").get("value")), "");
    assert_eq!(
        field("proxy_kind").get("options").map(|o| matches!(o, Json::Arr(v) if v.len() == 4)),
        Some(true)
    );
    assert_eq!(field("connections_limit").get("min").and_then(Json::as_i64), Some(i32::MIN as i64));

    // bad fields: the first one is named
    let bad = |body: &str| {
        let r = t.pa(&sid, "/pa/prefs", body);
        assert_eq!(r.status, 400, "{body}: {}", r.text());
        s(r.json().get("error")).to_owned()
    };
    assert_eq!(bad(r#"{"connections_limit":"lots"}"#), "connections_limit: \"lots\" is not a number");
    assert_eq!(bad(r#"{"enable_dht":false,"nope":1}"#), "nope: unknown preference");
    assert_eq!(bad(r#"{"webui.listen":"x"}"#), "webui.listen: read-only");
    assert_eq!(bad(r#"{"listen_port":70000}"#), "listen_port: 70000 is outside 1..65535");
    assert_eq!(bad(r#"{"proxy_kind":2}"#), "proxy_hostname: Please enter the proxy server's address.");
    let ipf = bad(r#"{"ipfilter.enable":true,"ipfilter.path":"/no/such/list.dat"}"#);
    assert!(ipf.starts_with("ipfilter.path: "), "{ipf}");
    // nothing of a refused body is applied
    let m = t.get(&sid, "/pa/prefs").json();
    assert_eq!(m.get("fields").unwrap().get("enable_dht").unwrap().get("value"), Some(&Json::Bool(false)));

    // a valid partial change: applied at once, seen natively and through the shim, persisted
    let elsewhere = t.dir.join("elsewhere");
    let body = format!(
        r#"{{"connections_limit":77,"download_rate_limit":51200,"dir.download":"{}","webui.domain_list":"*.example","webui.password":"new-pass","ui.show_add_dialog":false,"bt.max_peers":50,"alt_up_limit":2048}}"#,
        elsewhere.to_string_lossy().replace('\\', "\\\\")
    );
    let r = t.pa(&sid, "/pa/prefs", &body);
    assert_eq!(r.status, 204, "{}", r.text());
    let m = t.get(&sid, "/pa/prefs").json();
    let fields = m.get("fields").unwrap();
    let value = |id: &str| fields.get(id).unwrap().get("value").cloned().unwrap();
    assert_eq!(value("connections_limit"), Json::Int(77));
    assert_eq!(value("download_rate_limit"), Json::Int(51200));
    assert_eq!(value("bt.max_peers"), Json::Int(50));
    assert_eq!(value("alt_up_limit"), Json::Int(2048));
    assert_eq!(value("ui.show_add_dialog"), Json::Bool(false));
    let p = t.get(&sid, "/api/v2/app/preferences").json();
    assert_eq!(p.get("max_connec").and_then(Json::as_i64), Some(77));
    assert_eq!(p.get("dl_limit").and_then(Json::as_i64), Some(50));
    assert_eq!(p.get("alt_up_limit").and_then(Json::as_i64), Some(2));
    assert_eq!(s(p.get("save_path")), elsewhere.to_string_lossy());
    assert_eq!(s(p.get("web_ui_domain_list")), "*.example");
    assert_eq!(t.get(&sid, "/api/v2/transfer/downloadLimit").text(), "51200");
    assert_eq!(t.get(&sid, "/pa/add").json().get("show_dialog"), Some(&Json::Bool(false)));
    let file = std::fs::read_to_string(t.dir.join("state").join(STATE_FILE)).unwrap();
    assert!(file.contains(r#""lt.connections_limit":"77""#), "{file}");
    assert!(file.contains(r#""ui.show_add_dialog":"0""#), "{file}");
    assert!(file.contains(r#""webui.domain_list":"*.example""#), "{file}");
    assert!(!file.contains("new-pass") && !file.contains("ui.close_to_tray"), "{file}");

    // the domain list took effect for the next request
    let mut sock = TcpStream::connect(t.addr).unwrap();
    let req = format!(
        "GET /api/v2/app/version HTTP/1.1\r\nHost: rebind.example:{}\r\nCookie: {sid}\r\nConnection: close\r\n\r\n",
        t.addr.port()
    );
    sock.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    sock.read_to_end(&mut out).unwrap();
    assert_eq!(parse_response(&out).0.status, 200);

    // the password changed: the old one fails, the new one logs in
    let r = t.req("POST", "/api/v2/auth/login", &[], Body::Form(&format!("username=admin&password={PASS}")));
    assert_eq!(r.text(), "Fails.");
    let r = t.req("POST", "/api/v2/auth/login", &[], Body::Form("username=admin&password=new-pass"));
    assert_eq!(r.text(), "Ok.");

    // the shim's setPreferences lands on the same state (and clamps as before)
    let set = r#"{"max_connec":-5,"encryption":1,"web_ui_domain_list":""}"#;
    assert_eq!(t.post(&sid, "/api/v2/app/setPreferences", &format!("json={}", enc(set))).status, 200);
    let m = t.get(&sid, "/pa/prefs").json();
    let fields = m.get("fields").unwrap();
    let value = |id: &str| fields.get(id).unwrap().get("value").cloned().unwrap();
    assert_eq!(value("connections_limit"), Json::Int(-1));
    assert_eq!(value("enc_out"), Json::Int(2), "Forced");
    assert_eq!(value("enc_legacy"), Json::Bool(false));
    assert_eq!(value("webui.domain_list"), Json::Str(String::new()));
    // an empty body changes nothing and is fine
    assert_eq!(t.pa(&sid, "/pa/prefs", "{}").status, 204);
}

#[test]
fn native_prefs_survive_a_restart() {
    let dir = temp_dir("prefs-restart");
    // a state file from before slice 6: the shim's keys under `preferences`, migrated once
    let state = dir.join("state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join(STATE_FILE), r#"{"preferences":{"dl_limit":100,"no_such":1}}"#).unwrap();
    let port = 47123i64;
    let download = dir.join("restart-dl");
    {
        let t = Harness::start_in(dir.clone(), Some(PASS));
        let sid = t.login();
        assert_eq!(t.get(&sid, "/api/v2/transfer/downloadLimit").text(), "102400");
        let file = std::fs::read_to_string(state.join(STATE_FILE)).unwrap();
        assert!(
            !file.contains("preferences") && file.contains(r#""lt.download_rate_limit":"102400""#),
            "{file}"
        );

        let body = format!(
            r#"{{"listen_port":{port},"dir.download":"{}","enable_lsd":true,"webui.session_timeout":120}}"#,
            download.to_string_lossy().replace('\\', "\\\\")
        );
        let r = t.pa(&sid, "/pa/prefs", &body);
        assert_eq!(r.status, 204, "{}", r.text());
        let p = t.get(&sid, "/api/v2/app/preferences").json();
        assert_eq!(p.get("listen_port").and_then(Json::as_i64), Some(port));
        assert_eq!(s(p.get("save_path")), download.to_string_lossy());
        assert_eq!(p.get("lsd"), Some(&Json::Bool(true)));
        let m = t.get(&sid, "/pa/prefs").json();
        let li = m.get("fields").unwrap().get("listen_interfaces").unwrap().get("value").cloned();
        assert_eq!(li, Some(Json::Str(format!("127.0.0.1:{port}"))));
        // keep the directory for the second run: the harness removes this one instead
        let mut t = t;
        t.dir = temp_dir("prefs-restart-scratch");
    }
    let t = Harness::start_in(dir, None);
    let sid = t.login();
    let m = t.get(&sid, "/pa/prefs").json();
    let fields = m.get("fields").unwrap();
    let value = |id: &str| fields.get(id).unwrap().get("value").cloned().unwrap();
    assert_eq!(value("listen_port"), Json::Int(port));
    assert_eq!(value("dir.download"), Json::Str(download.to_string_lossy().into_owned()));
    assert_eq!(value("enable_lsd"), Json::Bool(true));
    assert_eq!(value("webui.session_timeout"), Json::Int(120));
    assert_eq!(value("download_rate_limit"), Json::Int(102400));
    // the harness's own settings that were never stored are kept (DHT off)
    assert_eq!(value("enable_dht"), Json::Bool(false));
    let p = t.get(&sid, "/api/v2/app/preferences").json();
    assert_eq!(p.get("listen_port").and_then(Json::as_i64), Some(port));
    assert_eq!(s(p.get("save_path")), download.to_string_lossy());
    assert_eq!(t.get(&sid, "/api/v2/transfer/downloadLimit").text(), "102400");
}

/// Waits until `f` holds (files appearing or going away in the `.torrent` folders).
fn wait_until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn torrent_folders_store_copies_and_auto_load() {
    let t = Harness::start("torrent-folders");
    let sid = t.login();
    let (store, watch) = (t.dir.join("store"), t.dir.join("watch"));
    std::fs::create_dir_all(&watch).unwrap();
    let esc = |p: &PathBuf| p.to_string_lossy().replace('\\', "\\\\");
    // a box without its folder is refused, naming the folder
    let r = t.pa(&sid, "/pa/prefs", r#"{"dir.autoload":true}"#);
    assert_eq!(r.status, 400);
    assert!(r.text().contains("dir.autoload_dir"), "{}", r.text());
    let body = format!(
        r#"{{"dir.store_torrents":true,"dir.torrents":"{}","dir.autoload":true,"dir.autoload_dir":"{}"}}"#,
        esc(&store),
        esc(&watch)
    );
    let r = t.pa(&sid, "/pa/prefs", &body);
    assert_eq!(r.status, 204, "{}", r.text());
    let m = t.get(&sid, "/pa/prefs").json();
    let field = |id: &str| m.get("fields").unwrap().get(id).unwrap().get("value").cloned().unwrap();
    assert_eq!(field("dir.autoload"), Json::Bool(true));
    assert_eq!(field("dir.torrents"), Json::Str(store.to_string_lossy().into_owned()));

    // added through the page: a copy named after the torrent
    let (bytes, hash) = torrent_named("folders-added", None);
    assert_eq!(t.pa_upload(&sid, "/pa/add", r#"{"start":false}"#, &bytes).status, 204);
    t.wait_for(&sid, &hash, "the added torrent", |_| true);
    wait_until("the stored copy", || store.join("folders-added.torrent").exists());
    assert_eq!(std::fs::read(store.join("folders-added.torrent")).unwrap(), bytes);

    // dropped into the watched folder: added without a dialog, renamed to .torrent.loaded
    let (bytes, hash) = torrent_named("folders-auto", None);
    std::fs::write(watch.join("dropped.torrent"), &bytes).unwrap();
    t.wait_for(&sid, &hash, "the auto-loaded torrent", |_| true);
    wait_until("the .loaded rename", || watch.join("dropped.torrent.loaded").exists());
    assert!(!watch.join("dropped.torrent").exists());
    wait_until("the auto-loaded copy", || store.join("folders-auto.torrent").exists());

    // with "Delete loaded .torrents" the file goes; one already in the list is finished too
    let r = t.pa(&sid, "/pa/prefs", r#"{"dir.autoload_delete":true}"#);
    assert_eq!(r.status, 204, "{}", r.text());
    std::fs::write(watch.join("again.torrent"), &bytes).unwrap();
    wait_until("the duplicate deleted", || !watch.join("again.torrent").exists());
    let (bytes, hash) = torrent_named("folders-deleted", None);
    std::fs::write(watch.join("gone.torrent"), &bytes).unwrap();
    t.wait_for(&sid, &hash, "the third torrent", |_| true);
    wait_until("the file deleted", || !watch.join("gone.torrent").exists());
    assert!(!watch.join("gone.torrent.loaded").exists());

    // a file that is not a torrent stays where it is
    std::fs::write(watch.join("broken.torrent"), b"not bencode").unwrap();
    std::thread::sleep(Duration::from_secs(5));
    assert!(watch.join("broken.torrent").exists());
}

/// Properties on a selection larger than the reply cache: every torrent's tracker list is
/// asked for and read back (a missing one would load the box blank).
#[test]
fn torrent_properties_on_a_whole_library() {
    let t = Harness::start_mock("properties-all", 300, 0.0, 0);
    let sid = t.login();
    let deadline = Instant::now() + Duration::from_secs(10);
    let all = || t.get(&sid, "/pa/torrents/properties?all=1").json();
    while all().get("count").and_then(Json::as_i64) != Some(300) {
        assert!(Instant::now() < deadline, "the library never loaded");
        std::thread::sleep(Duration::from_millis(50));
    }
    let tracker = |p: &Json| s(p.get("fields").and_then(|f| f.get("trackers"))).to_owned();
    assert_eq!(tracker(&all()), "", "the generated lists differ");
    let body = r#"{"all":true,"fields":{"trackers":"udp://one.example:1/announce"}}"#;
    let r = t.pa(&sid, "/pa/torrents/properties", body);
    assert_eq!(r.status, 204, "{}", r.text());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let p = all();
        if tracker(&p) == "udp://one.example:1/announce" {
            break;
        }
        assert!(Instant::now() < deadline, "{}", p.to_json());
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn torrent_properties_and_the_shims_tracker_and_share_limit_edits() {
    let t = Harness::start("properties");
    let sid = t.login();
    let (bytes, hash) = torrent_with(Some("udp://127.0.0.1:9/announce"));
    let (body, boundary) = multipart(&[("paused", "true")], &bytes);
    let r = t.req("POST", "/api/v2/torrents/add", &[("Cookie", &sid)], Body::Multipart(&body, boundary));
    assert_eq!(r.status, 200);
    t.wait_for(&sid, &hash, "the added torrent", |x| s(x.get("name")) == "webui-test");
    let props = || t.get(&sid, &format!("/pa/torrents/properties?ids={hash}")).json();
    let field = |p: &Json, k: &str| s(p.get("fields").and_then(|f| f.get(k))).to_owned();
    let p = props();
    assert_eq!(p.get("count").and_then(Json::as_i64), Some(1));
    assert_eq!(field(&p, "trackers"), "udp://127.0.0.1:9/announce");
    assert_eq!(field(&p, "upload_kib"), "0");
    assert_eq!((field(&p, "override_goal"), field(&p, "dht")), ("off".to_owned(), "on".to_owned()));
    assert_eq!(t.get(&sid, "/pa/torrents/properties?ids=00").status, 404);
    // the page's dialog title and double-click
    assert_eq!(s(p.get("name")), "webui-test");
    assert_eq!(p.get("finished").and_then(Json::as_bool), Some(false));

    // several torrents, and the page's Ctrl+A: `all=1` with the view's filter
    let (other_bytes, other) = torrent_named("other-props", Some("udp://127.0.0.1:9/other"));
    let (body, boundary) = multipart(&[("paused", "true")], &other_bytes);
    let r = t.req("POST", "/api/v2/torrents/add", &[("Cookie", &sid)], Body::Multipart(&body, boundary));
    assert_eq!(r.status, 200);
    t.wait_for(&sid, &other, "the second torrent", |x| s(x.get("name")) == "other-props");
    let both = t.get(&sid, &format!("/pa/torrents/properties?ids={hash},{other}")).json();
    assert_eq!(both.get("count").and_then(Json::as_i64), Some(2));
    assert_eq!(field(&both, "trackers"), "", "differing trackers load blank");
    assert_eq!(field(&both, "dht"), "on");
    assert!(both.get("name").is_none(), "no name for several");
    let all = t.get(&sid, "/pa/torrents/properties?all=1").json();
    assert_eq!(all.get("count").and_then(Json::as_i64), Some(2));
    let one = t.get(&sid, "/pa/torrents/properties?all=1&category=all&search=OTHER").json();
    assert_eq!((one.get("count").and_then(Json::as_i64), s(one.get("name"))), (Some(1), "other-props"));
    assert_eq!(field(&one, "trackers"), "udp://127.0.0.1:9/other");
    assert_eq!(t.get(&sid, "/pa/torrents/properties?all=1&label=").status, 200, "No Label");
    assert_eq!(t.get(&sid, "/pa/torrents/properties?all=1&search=zzz").status, 404);
    assert_eq!(t.get(&sid, "/pa/torrents/properties?all=1&category=bogus").status, 400);
    let r = t.pa(
        &sid,
        "/pa/torrents/properties",
        r#"{"all":true,"filter":{"search":"other"},"fields":{"pex":"off"}}"#,
    );
    assert_eq!(r.status, 204, "{}", r.text());
    let deadline = Instant::now() + Duration::from_secs(10);
    while field(&t.get(&sid, &format!("/pa/torrents/properties?ids={other}")).json(), "pex") != "off" {
        assert!(Instant::now() < deadline, "pex never went off");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(field(&props(), "pex"), "on", "outside the filter");
    let mixed = t.get(&sid, &format!("/pa/torrents/properties?ids={hash},{other}")).json();
    assert_eq!(field(&mixed, "pex"), "mixed");
    let r = t.pa(&sid, "/pa/torrents/remove", &format!(r#"{{"ids":["{other}"]}}"#));
    assert_eq!(r.status, 204, "{}", r.text());

    // the dialog's OK: only the fields sent change
    let body = format!(
        r#"{{"ids":["{hash}"],"fields":{{"trackers":"udp://127.0.0.1:9/announce\n\nhttp://127.0.0.1:9/b","upload_kib":"7","override_goal":"on","goal_ratio":"250","dht":"off"}}}}"#
    );
    let r = t.pa(&sid, "/pa/torrents/properties", &body);
    assert_eq!(r.status, 204, "{}", r.text());
    let deadline = Instant::now() + Duration::from_secs(10);
    let p = loop {
        let p = props();
        if field(&p, "upload_kib") == "7" && field(&p, "dht") == "off" {
            break p;
        }
        assert!(Instant::now() < deadline, "{}", p.to_json());
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(field(&p, "trackers"), "udp://127.0.0.1:9/announce\n\nhttp://127.0.0.1:9/b");
    assert_eq!(field(&p, "override_goal"), "on");
    assert_eq!((field(&p, "goal_ratio"), field(&p, "goal_minutes")), ("250".to_owned(), "0".to_owned()));
    let bad = format!(r#"{{"ids":["{hash}"],"fields":{{"trackers":"nope"}}}}"#);
    let r = t.pa(&sid, "/pa/torrents/properties", &bad);
    assert_eq!(r.status, 400);
    assert!(s(r.json().get("error")).starts_with("trackers:"), "{}", r.text());
    let bogus = format!(r#"{{"ids":["{hash}"],"fields":{{"bogus":"1"}}}}"#);
    assert_eq!(t.pa(&sid, "/pa/torrents/properties", &bogus).status, 400);

    // the shim's tracker edits
    let urls = || -> Vec<String> {
        let Json::Arr(v) = t.get(&sid, &format!("/api/v2/torrents/trackers?hash={hash}")).json() else {
            panic!("a list")
        };
        v.iter().map(|x| s(x.get("url")).to_owned()).filter(|u| !u.starts_with("** [")).collect()
    };
    let edit = |orig: &str, new: &str| {
        let q = format!("hash={hash}&origUrl={}&newUrl={}", enc(orig), enc(new));
        t.post(&sid, "/api/v2/torrents/editTracker", &q).status
    };
    assert_eq!(edit("http://127.0.0.1:9/b", "http://127.0.0.1:9/c"), 200);
    assert_eq!(urls(), ["udp://127.0.0.1:9/announce", "http://127.0.0.1:9/c"]);
    assert_eq!(edit("http://127.0.0.1:9/b", "http://127.0.0.1:9/d"), 409, "the old URL is gone");
    assert_eq!(edit("http://127.0.0.1:9/c", "udp://127.0.0.1:9/announce"), 409, "the new one is there");
    assert_eq!(edit("http://127.0.0.1:9/c", "not a url"), 400);
    let add = format!("hash={hash}&urls={}", enc("udp://x.example:1\nudp://y.example:2\nnot-a-url"));
    assert_eq!(t.post(&sid, "/api/v2/torrents/addTrackers", &add).status, 200);
    assert_eq!(urls().len(), 4);
    let remove = format!("hash={hash}&urls={}", enc("udp://x.example:1|udp://y.example:2"));
    assert_eq!(t.post(&sid, "/api/v2/torrents/removeTrackers", &remove).status, 200);
    assert_eq!(urls(), ["udp://127.0.0.1:9/announce", "http://127.0.0.1:9/c"]);
    assert_eq!(t.post(&sid, "/api/v2/torrents/removeTrackers", &remove).status, 409);
    assert_eq!(t.post(&sid, "/api/v2/torrents/editTracker", "hash=00&origUrl=a&newUrl=b").status, 404);

    // setShareLimits: the torrent's own seeding goal (-1 = no target, -2 = the default)
    let limits =
        |q: &str| t.post(&sid, "/api/v2/torrents/setShareLimits", &format!("hashes={hash}&{q}")).status;
    assert_eq!(limits("ratioLimit=-1&seedingTimeLimit=30"), 200);
    let p = props();
    assert_eq!(field(&p, "override_goal"), "on");
    assert_eq!((field(&p, "goal_ratio"), field(&p, "goal_minutes")), ("0".to_owned(), "30".to_owned()));
    assert_eq!(limits("ratioLimit=1.5&seedingTimeLimit=-2"), 200);
    let p = props();
    assert_eq!((field(&p, "goal_ratio"), field(&p, "goal_minutes")), ("150".to_owned(), "0".to_owned()));
    assert_eq!(limits("ratioLimit=-2&seedingTimeLimit=-2"), 200);
    assert_eq!(field(&props(), "override_goal"), "off");
    assert_eq!(limits("ratioLimit=1"), 400);

    // the seeding goal is on the Queueing page
    let r =
        t.pa(&sid, "/pa/prefs", r#"{"queue.seed_ratio":0,"queue.seed_limit":true,"queue.seed_limit_kib":5}"#);
    assert_eq!(r.status, 204, "{}", r.text());
    let m = t.get(&sid, "/pa/prefs").json();
    let pref = |id: &str| m.get("fields").unwrap().get(id).unwrap().get("value").cloned().unwrap();
    assert_eq!((pref("queue.seed_ratio"), pref("queue.seed_limit")), (Json::Int(0), Json::Bool(true)));
}

/// A BEP 15 tracker that answers connects, and scrapes with 17 seeds and 23 peers for every
/// info-hash. Returns its announce URL and the number of info-hashes it was asked about.
fn scrape_tracker() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    let url = format!("udp://{}/announce", s.local_addr().unwrap());
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = asked.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(60) {
            let Ok((n, from)) = s.recv_from(&mut buf) else { continue };
            if n < 16 {
                continue;
            }
            let action = u32::from_be_bytes(buf[8..12].try_into().unwrap());
            let mut out = Vec::new();
            out.extend_from_slice(&action.to_be_bytes());
            out.extend_from_slice(&buf[12..16]);
            match action {
                0 => out.extend_from_slice(&0x0bad_cafeu64.to_be_bytes()),
                2 => {
                    let hashes = (n - 16) / 20;
                    counter.fetch_add(hashes, Ordering::SeqCst);
                    for _ in 0..hashes {
                        for v in [17u32, 5, 23] {
                            out.extend_from_slice(&v.to_be_bytes());
                        }
                    }
                }
                _ => continue,
            }
            let _ = s.send_to(&out, from);
        }
    });
    (url, asked)
}

#[test]
fn stopped_torrents_get_swarm_counts_from_the_periodic_scrape() {
    let (url, asked) = scrape_tracker();
    let t = Harness::start("scrape");
    let sid = t.login();
    let add = |bytes: &[u8]| {
        let (body, boundary) = multipart(&[("paused", "true")], bytes);
        let r = t.req("POST", "/api/v2/torrents/add", &[("Cookie", &sid)], Body::Multipart(&body, boundary));
        assert_eq!(r.status, 200);
    };
    let (bytes, hash) = torrent_named("scraped", Some(&url));
    add(&bytes);
    // no trackers: never scraped, the counts stay unknown
    let (bare_bytes, bare) = torrent_named("no-trackers", None);
    add(&bare_bytes);
    // the first round after the add (5 s at most) asks the tracker; nothing announces
    let x = t.wait_for(&sid, &hash, "the scrape counts", |x| {
        x.get("num_complete") == Some(&Json::Int(17)) && x.get("num_incomplete") == Some(&Json::Int(23))
    });
    // nothing connected: the cells read "0 (17)" and "0 (23)"
    assert_eq!((x.get("num_seeds"), x.get("num_leechs")), (Some(&Json::Int(0)), Some(&Json::Int(0))));
    let y = t.wait_for(&sid, &bare, "the bare torrent", |_| true);
    assert_eq!(
        (y.get("num_complete"), y.get("num_incomplete")),
        (Some(&Json::Int(-1)), Some(&Json::Int(-1)))
    );
    // one scrape, not one per tick
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

/// `torrents/properties` of a torrent the engine restored from its state directory (added in
/// an earlier run, so the shim never saw its `.torrent`): the private flag from the info
/// dictionary, the comment, creator and date from the resume data, asked of the engine
/// (`TorrentHandle::metadata`); the tracker list's DHT / PeX / LSD rows say it is private.
#[test]
fn properties_of_a_restored_torrent_come_from_the_engine() {
    let dir = temp_dir("restored-meta");
    let (bytes, hash) = torrent_built("restored-private", None, true);
    let check = |t: &Harness, sid: &str| {
        let p = t.get(sid, &format!("/api/v2/torrents/properties?hash={hash}")).json();
        assert_eq!(p.get("isPrivate"), Some(&Json::Bool(true)), "{}", p.to_json());
        assert_eq!(s(p.get("comment")), "made for a test");
        assert_eq!(s(p.get("created_by")), "the test");
        assert_eq!(p.get("creation_date").and_then(Json::as_i64), Some(951_782_400));
    };
    {
        let mut t = Harness::start_with(dir.clone(), Harness::settings());
        let sid = t.login();
        let (body, boundary) = multipart(&[("paused", "true")], &bytes);
        let r = t.req("POST", "/api/v2/torrents/add", &[("Cookie", &sid)], Body::Multipart(&body, boundary));
        assert_eq!((r.status, r.text().as_str()), (200, "Ok."));
        t.wait_for(&sid, &hash, "the added torrent", |x| s(x.get("name")) == "restored-private");
        check(&t, &sid);
        // keep the directory for the second run: the harness removes this one instead
        t.dir = temp_dir("restored-meta-scratch");
    }
    let t = Harness::start_with(dir, Harness::settings());
    let sid = t.login();
    let x = t.wait_for(&sid, &hash, "the restored torrent", |x| s(x.get("name")) == "restored-private");
    assert!(s(x.get("state")).starts_with("paused") || s(x.get("state")).starts_with("stopped"));
    check(&t, &sid);
    // asked once, then the list's `isPrivate` field knows it too
    let x = t.wait_for(&sid, &hash, "the private field", |x| x.get("isPrivate") == Some(&Json::Bool(true)));
    assert_eq!(s(x.get("name")), "restored-private");
    let Json::Arr(tr) = t.get(&sid, &format!("/api/v2/torrents/trackers?hash={hash}")).json() else {
        panic!()
    };
    assert_eq!(s(tr[0].get("msg")), "This torrent is private");
}

/// `torrents/pieceStates` sends 1 for the pieces with blocks in flight: a seed on loopback, a
/// leech limited to 4 kB/s told about it through `torrents/addPeers`, so each 16 KiB piece
/// stays requested for seconds. The seed's port is the one its engine reports: a port picked
/// in advance can be taken by the other tests' sockets by the time the seed binds it, and the
/// engine then listens one port up while the leech knocks on the wrong door (CI
/// 39923065588, Windows).
#[test]
fn piece_states_report_the_pieces_being_downloaded() {
    let (bytes, hash) = torrent();
    let seed = Harness::start_with(temp_dir("piece-seed"), Harness::settings());
    let port = seed.listen_port();
    let data = seed.dir.join("data").join("webui-test");
    std::fs::create_dir_all(&data).unwrap();
    let all = torrent_data();
    std::fs::write(data.join("a.bin"), &all[..40_000]).unwrap();
    std::fs::write(data.join("b.bin"), &all[40_000..]).unwrap();
    let seed_sid = seed.login();
    let save = seed.dir.join("data").to_string_lossy().into_owned();
    let (body, boundary) = multipart(&[("savepath", &save), ("paused", "false")], &bytes);
    let r =
        seed.req("POST", "/api/v2/torrents/add", &[("Cookie", &seed_sid)], Body::Multipart(&body, boundary));
    assert_eq!((r.status, r.text().as_str()), (200, "Ok."));
    seed.wait_for(&seed_sid, &hash, "the seed", |x| matches!(s(x.get("state")), "uploading" | "stalledUP"));

    let t = Harness::start("piece-leech");
    let sid = t.login();
    let (body, boundary) = multipart(&[("paused", "false"), ("dlLimit", "4000")], &bytes);
    let r = t.req("POST", "/api/v2/torrents/add", &[("Cookie", &sid)], Body::Multipart(&body, boundary));
    assert_eq!((r.status, r.text().as_str()), (200, "Ok."));
    t.wait_for(&sid, &hash, "the leech", |x| x.get("dl_limit") == Some(&Json::Int(4000)));
    // a bad peer list is refused; the seed is connected to
    let bad = t.post(&sid, "/api/v2/torrents/addPeers", &format!("hashes={hash}&peers=nowhere"));
    assert_eq!(bad.status, 400);
    let peers = enc(&format!("127.0.0.1:{port}|[::1]:0"));
    let r = t.post(&sid, "/api/v2/torrents/addPeers", &format!("hashes={hash}&peers={peers}"));
    assert_eq!(r.status, 200);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let Json::Arr(v) = t.get(&sid, &format!("/api/v2/torrents/pieceStates?hash={hash}")).json() else {
            panic!()
        };
        assert_eq!(v.len(), 4);
        if v.contains(&Json::Int(1)) {
            break;
        }
        assert!(Instant::now() < deadline, "no piece reported downloading: {v:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// An HTTP proxy on loopback that answers every absolute-form `GET` itself with `body` (the
/// URLs name a host that does not resolve, so only the proxy can answer them). Its log holds
/// the request lines.
fn torrent_proxy(body: Vec<u8>) -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log2 = log.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match c.read(&mut b) {
                    Ok(1) => head.push(b[0]),
                    _ => break,
                }
            }
            let line = String::from_utf8_lossy(&head).lines().next().unwrap_or_default().to_owned();
            log2.lock().unwrap().push(line);
            let mut out =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                    .into_bytes();
            out.extend_from_slice(&body);
            let _ = c.write_all(&out);
        }
    });
    (port, log)
}

/// `torrents/add` and the native add with an `http://` link fetch it through the session's
/// proxy, also with `proxy_tracker_connections` off (web fetches follow the proxy whenever one
/// is set, as libtorrent's `.torrent` URL download did).
#[test]
fn url_adds_go_through_the_sessions_proxy() {
    let (bytes, hash) = torrent_named("webui-proxied", None);
    let (port, log) = torrent_proxy(bytes);
    let settings = Settings {
        proxy_type: patorrent_core::ProxyType::Http,
        proxy_hostname: "127.0.0.1".into(),
        proxy_port: port,
        proxy_tracker_connections: false,
        proxy_peer_connections: false,
        ..Harness::settings()
    };
    let t = Harness::start_with(temp_dir("url-proxy"), settings);
    let sid = t.login();
    let url = "http://indexer.invalid/get/1.torrent";
    let r = t.post(&sid, "/api/v2/torrents/add", &format!("urls={}&paused=true", enc(url)));
    assert_eq!((r.status, r.text().as_str()), (200, "Ok."));
    t.wait_for(&sid, &hash, "the fetched torrent", |x| s(x.get("name")) == "webui-proxied");
    assert_eq!(log.lock().unwrap().as_slice(), [format!("GET {url} HTTP/1.1")]);
    // the native add's URL takes the same way (the same torrent again: the engine keeps one)
    let r = t.pa(&sid, "/pa/add", r#"{"url":"http://indexer.invalid/get/2.torrent"}"#);
    assert_eq!(r.status, 204, "{}", r.text());
    assert_eq!(log.lock().unwrap().len(), 2);
    assert_eq!(log.lock().unwrap()[1], "GET http://indexer.invalid/get/2.torrent HTTP/1.1");
}
