//! The Web UI and WebAPI over loopback: a real session plus the server, driven by a raw HTTP
//! client. Login and logout, the failed-login ban, CSRF and Host checks, adding torrents
//! (multipart file, magnet), `torrents/info`, `sync/maindata` diffs by rid, stop / start,
//! categories, delete, preferences, and the connection handling (keep-alive, pipelining,
//! `Expect: 100-continue`, size limits). Then the native API: the WebSocket upgrade and
//! frames with a raw socket client, and the session protocol (`hello`, `view` / `rows`,
//! `tick` diffs, `idle`) against the real engine and the 10,000-torrent mock profile, and the
//! detail tabs (`detail` subscriptions: General, Trackers, Files, Pieces, Speed, Peers) plus
//! `sync/torrentPeers` through the shim.

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
    let piece = 16 * 1024;
    let sizes = [("a.bin", 40_000usize), ("b.bin", 10_000)];
    let total: usize = sizes.iter().map(|s| s.1).sum();
    let data: Vec<u8> = (0..total).map(|i| (i * 7 % 251) as u8).collect();
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
    *info.key("name") = Entry::Bytes(b"webui-test".to_vec());
    *info.key("piece length") = Entry::Int(piece as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
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
        let pump = {
            let (web, stop) = (web.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut alerts = Vec::new();
                while !stop.load(Ordering::SeqCst) {
                    h.post_torrent_updates();
                    h.post_session_stats();
                    std::thread::sleep(Duration::from_millis(50));
                    h.pop_alerts(&mut alerts);
                    web.on_alerts(&alerts);
                    alerts.clear();
                }
            })
        };
        Harness { web, addr, stop, pump: Some(pump), session: Some(session), dir }
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

    fn maindata(&self, sid: &str, rid: i64) -> Json {
        let r = self.get(sid, &format!("/api/v2/sync/maindata?rid={rid}"));
        assert_eq!(r.status, 200);
        r.json()
    }

    /// Polls `torrents/info` until `f` holds for the torrent.
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
    let b = "----noxtestboundary";
    let mut out = Vec::new();
    for (n, v) in fields {
        out.extend_from_slice(
            format!("--{b}\r\nContent-Disposition: form-data; name=\"{n}\"\r\n\r\n{v}\r\n").as_bytes(),
        );
    }
    out.extend_from_slice(
        format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"torrents\"; filename=\"t.torrent\"\r\n\
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
        total += r.body.len();
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
    for other in ["/style.css", "/app.js"] {
        assert!(t.req("GET", other, &[], Body::None).header("content-security-policy").is_none(), "{other}");
    }
    // the HTML: no inline script or style anywhere, and only our own two files referenced
    let html = page.text();
    assert!(html.contains("<title>PaTorrent</title>"));
    let lower = html.to_ascii_lowercase();
    assert!(!lower.contains("<style"), "an inline <style> element");
    assert!(!lower.contains(" style=\""), "an inline style attribute");
    assert!(!lower.contains("javascript:"), "a javascript: URL");
    for (at, _) in lower.match_indices("<script") {
        let tag = &lower[at..at + lower[at..].find('>').unwrap()];
        assert!(tag.contains("src=\"app.js\""), "an inline <script>: {tag}");
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

    /// The next text frame as JSON (pings are answered on the way).
    fn recv_json(&mut self) -> Json {
        loop {
            match self.recv_frame() {
                Some((1, payload)) => {
                    let text = String::from_utf8(payload).unwrap();
                    return json::parse(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
                }
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
            let m = self.recv_json();
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
    assert_eq!(ws.recv_frame(), Some((10, b"are you there".to_vec())));

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
            let m = self.recv_json();
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
    assert_eq!(m.get("samples"), Some(&Json::Arr(vec![Json::Arr(vec![Json::Int(0), Json::Int(0)])])));
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
