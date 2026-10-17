//! The Web UI and WebAPI over loopback: a real session plus the server, driven by a raw HTTP
//! client. Login and logout, the failed-login ban, CSRF and Host checks, adding torrents
//! (multipart file, magnet), `torrents/info`, `sync/maindata` diffs by rid, stop / start,
//! categories, delete, preferences, and the connection handling (keep-alive, pipelining,
//! `Expect: 100-continue`, size limits).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{Session, SessionParams, Settings};
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
    *t.key("info") = info;
    *t.key("comment") = Entry::Bytes(b"made for a test".to_vec());
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

    fn start_in(dir: PathBuf, password: Option<&str>) -> Harness {
        let settings = Settings {
            listen_interfaces: "127.0.0.1:0".into(),
            tick_interval: 100,
            aio_threads: 2,
            enable_dht: false,
            enable_lsd: false,
            enable_upnp: false,
            enable_natpmp: false,
            ..Settings::default()
        };
        let session = Session::start(SessionParams { settings: settings.clone(), state_dir: None }).unwrap();
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
