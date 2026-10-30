//! The Web UI hosted by a desktop client (D40, `Config::hosted`) over the mock engine, driven
//! by a raw HTTP client and a fake host that collects the preferences journal: labels and
//! pause marks reach the host as its `torrent.<key>.*` keys and come back from it, a
//! Preferences change reaches it as `lt.*` without touching the engine, the login comes from
//! its options, no state file is written, the RSS Downloader and the alternative speed limits
//! are off, and `app/shutdown` tells the host.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use patorrent_core::{MockConfig, Session, Settings};
use patorrent_frontend::prefs::{Change, Prefs};
use patorrent_nox::webui::auth::PasswordHash;
use patorrent_nox::webui::json::{self, Json};
use patorrent_nox::webui::{Config, Host, Hosted, STATE_FILE, WebUi};

const PASS: &str = "s3cret-pass";

/// The desktop client: every journal batch, and whether it was told to exit.
#[derive(Default)]
struct FakeHost {
    batches: Mutex<Vec<Vec<Change>>>,
    shutdown: AtomicBool,
}

impl FakeHost {
    /// Every change handed over so far, flattened.
    fn changes(&self) -> Vec<Change> {
        self.batches.lock().unwrap().iter().flatten().cloned().collect()
    }

    fn value_of(&self, key: &str) -> Option<Option<String>> {
        self.changes().into_iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn clear(&self) {
        self.batches.lock().unwrap().clear();
    }
}

impl Host for FakeHost {
    fn options_changed(&self, changes: Vec<Change>) {
        self.batches.lock().unwrap().push(changes);
    }

    fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d =
        std::env::temp_dir().join(format!("nox-hosted-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Harness {
    web: Arc<WebUi>,
    host: Arc<FakeHost>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
    session: Option<Session>,
    dir: PathBuf,
}

impl Harness {
    /// The mock engine with three active torrents, hosted with `options` and the plain
    /// `password` (none: the login must come from the options).
    fn start(name: &str, options: Prefs, password: Option<&str>) -> Harness {
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
        let mock = MockConfig { initial_torrents: 3, active_fraction: 1.0, ..MockConfig::default() };
        let session = Session::start_mock(settings.clone(), mock);
        let h = session.handle();
        let dir = temp_dir(name);
        let host = Arc::new(FakeHost::default());
        let mut c = Config::new("127.0.0.1:0".parse().unwrap(), settings, dir.join("dl").to_string_lossy());
        c.password = password.map(str::to_owned);
        c.state_dir = Some(dir.join("state"));
        c.pbkdf2_iterations = 2;
        c.hosted = Some(Hosted { host: host.clone(), options });
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
        Harness { web, host, addr, stop, pump: Some(pump), session: Some(session), dir }
    }

    fn host_name(&self) -> String {
        self.addr.to_string()
    }

    fn login_as(&self, user: &str, pass: &str) -> Resp {
        self.req("POST", "/api/v2/auth/login", &[], Body::Form(&format!("username={user}&password={pass}")))
    }

    fn login(&self) -> String {
        let r = self.login_as("admin", PASS);
        assert_eq!((r.status, r.text().as_str()), (200, "Ok."));
        r.header("set-cookie").expect("a session cookie").split(';').next().unwrap().to_owned()
    }

    fn req(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Body<'_>) -> Resp {
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        s.write_all(&raw_request(&self.host_name(), method, path, headers, body)).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        parse_response(&out)
    }

    fn get(&self, sid: &str, path: &str) -> Resp {
        self.req("GET", path, &[("Cookie", sid)], Body::None)
    }

    fn post(&self, sid: &str, path: &str, form: &str) -> Resp {
        self.req("POST", path, &[("Cookie", sid)], Body::Form(form))
    }

    /// A native command: a JSON body with the CSRF header and a same-origin `Origin`.
    fn pa(&self, sid: &str, path: &str, body: &str) -> Resp {
        let origin = format!("http://{}", self.host_name());
        self.req(
            "POST",
            path,
            &[("Cookie", sid), ("Origin", &origin), ("X-Requested-With", "PaTorrent")],
            Body::Json(body),
        )
    }

    /// The mock's torrents' hashes (the host's keys too: they are v1 torrents), once listed.
    fn hashes(&self, sid: &str) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let list = self.get(sid, "/api/v2/torrents/info").json();
            if let Json::Arr(v) = &list {
                if v.len() == 3 {
                    return v
                        .iter()
                        .map(|t| t.get("hash").and_then(Json::as_str).unwrap().to_owned())
                        .collect();
                }
            }
            assert!(Instant::now() < deadline, "the torrents never showed up: {}", list.to_json());
            std::thread::sleep(Duration::from_millis(50));
        }
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

/// A raw WebSocket client: masked text frames out, the server's frames in.
struct Ws {
    s: TcpStream,
    buf: Vec<u8>,
}

impl Ws {
    /// Upgrades with the session cookie and a same-origin `Origin`; the `hello` comes with it.
    fn connect(t: &Harness, sid: &str) -> (Ws, Json) {
        let mut sock = TcpStream::connect(t.addr).unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let r = format!(
            "GET /pa/ws HTTP/1.1\r\nHost: {0}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nCookie: {sid}\r\nOrigin: http://{0}\r\n\r\n",
            t.host_name()
        );
        sock.write_all(r.as_bytes()).unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let end = loop {
            if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break p + 4;
            }
            let n = sock.read(&mut chunk).unwrap();
            assert!(n > 0, "closed before a response head: {}", String::from_utf8_lossy(&buf));
            buf.extend_from_slice(&chunk[..n]);
        };
        assert!(buf.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&buf[..end]));
        buf.drain(..end);
        let mut ws = Ws { s: sock, buf };
        let hello = ws.recv();
        assert_eq!(s(hello.get("t")), "hello", "{}", hello.to_json());
        (ws, hello)
    }

    fn send_text(&mut self, text: &str) {
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        let payload = text.as_bytes();
        let mut out = vec![0x81];
        if payload.len() < 126 {
            out.push(0x80 | payload.len() as u8);
        } else {
            out.push(0x80 | 126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, &b)| b ^ mask[i % 4]));
        self.s.write_all(&out).unwrap();
    }

    /// The next text message (fin, unmasked, a 7- or 16-bit length; pings are answered).
    fn recv(&mut self) -> Json {
        let mut chunk = [0u8; 65536];
        loop {
            if self.buf.len() >= 2 {
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
                    let op = self.buf[0] & 0x0f;
                    let payload = self.buf[at..at + len].to_vec();
                    self.buf.drain(..at + len);
                    match op {
                        1 => {
                            let text = String::from_utf8(payload).unwrap();
                            return json::parse(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
                        }
                        9 => {
                            // a ping: pong it back, masked
                            let mut out = vec![0x8a, 0x80 | payload.len() as u8, 0, 0, 0, 0];
                            out.extend_from_slice(&payload);
                            self.s.write_all(&out).unwrap();
                        }
                        op => panic!("unexpected frame {op}"),
                    }
                    continue;
                }
            }
            let n = self.s.read(&mut chunk).unwrap();
            assert!(n > 0, "closed while waiting for a message");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// Reads `rows` and `tick` messages until the torrent's cell has `state`.
    fn state_until(&mut self, hash: &str, state: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let m = self.recv();
            let seen = m.get("cells").and_then(|c| c.get(hash)).map(|c| s(c.get("state")).to_owned());
            if seen.as_deref() == Some(state) {
                return;
            }
            assert!(Instant::now() < deadline, "no {state} for {hash}: {}", m.to_json());
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
}

fn raw_request(host: &str, method: &str, path: &str, headers: &[(&str, &str)], body: Body<'_>) -> Vec<u8> {
    let mut r = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (n, v) in headers {
        r.push_str(&format!("{n}: {v}\r\n"));
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

fn parse_response(buf: &[u8]) -> Resp {
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
    Resp { status, headers, body: buf[end..end + len].to_vec() }
}

fn s(j: Option<&Json>) -> &str {
    j.and_then(Json::as_str).unwrap_or("")
}

/// The first element of a JSON array.
fn first(j: &Json) -> Json {
    match j {
        Json::Arr(v) if !v.is_empty() => v[0].clone(),
        _ => panic!("not a list with an element: {}", j.to_json()),
    }
}

/// The shim's category of a torrent.
fn category(t: &Harness, sid: &str, hash: &str) -> String {
    s(first(&t.get(sid, &format!("/api/v2/torrents/info?hashes={hash}")).json()).get("category")).to_owned()
}

/// The host's options with the login stored as the host keeps it.
fn options_with_login(user: &str, pass: &str) -> Prefs {
    let mut o = Prefs::default();
    o.set("webui.username", user);
    o.set("webui.password", PasswordHash::new(pass, 2).to_string());
    o
}

// ---- labels and pause marks, both ways ------------------------------------------------------

#[test]
fn labels_and_pause_marks_go_to_the_host_as_its_keys() {
    let t = Harness::start("labels-out", Prefs::default(), Some(PASS));
    let sid = t.login();
    let hashes = t.hashes(&sid);
    let a = &hashes[0];
    assert!(t.host.changes().is_empty(), "nothing is journaled at start: {:?}", t.host.changes());

    // (a) a label: the host's `torrent.<key>.label`; a v1 torrent's key is its hash
    assert_eq!(t.pa(&sid, "/pa/torrents/label", &format!(r#"{{"ids":["{a}"],"label":"tv"}}"#)).status, 204);
    assert_eq!(t.host.value_of(&format!("torrent.{a}.label")), Some(Some("tv".to_owned())));
    assert_eq!(category(&t, &sid, a), "tv");
    // removed: the key is dropped
    assert_eq!(t.pa(&sid, "/pa/torrents/label", &format!(r#"{{"ids":["{a}"],"label":""}}"#)).status, 204);
    assert_eq!(t.host.value_of(&format!("torrent.{a}.label")), Some(None));
    // the shim's category takes the same path
    assert_eq!(t.post(&sid, "/api/v2/torrents/setCategory", &format!("hashes={a}&category=tv")).status, 200);
    assert_eq!(t.host.value_of(&format!("torrent.{a}.label")), Some(Some("tv".to_owned())));

    // a pause mark: `torrent.<key>.paused` = 1, gone again at Start
    t.host.clear();
    assert_eq!(t.pa(&sid, "/pa/torrents/pause", &format!(r#"{{"ids":["{a}"]}}"#)).status, 204);
    assert_eq!(t.host.value_of(&format!("torrent.{a}.paused")), Some(Some("1".to_owned())));
    assert_eq!(t.pa(&sid, "/pa/torrents/start", &format!(r#"{{"ids":["{a}"]}}"#)).status, 204);
    assert_eq!(t.host.value_of(&format!("torrent.{a}.paused")), Some(None));
    // the same mark again is not a change
    t.host.clear();
    assert_eq!(t.pa(&sid, "/pa/torrents/start", &format!(r#"{{"ids":["{a}"]}}"#)).status, 204);
    assert!(t.host.changes().is_empty(), "{:?}", t.host.changes());
}

#[test]
fn the_hosts_changes_show_in_the_lists_and_a_new_hash_ends_the_sessions() {
    let t = Harness::start("labels-in", Prefs::default(), Some(PASS));
    let sid = t.login();
    let hashes = t.hashes(&sid);
    let (a, b) = (&hashes[0], &hashes[1]);

    // (b) a label from the host: the label list and the torrent's category, and nothing goes
    // back to the host
    t.web.options_changed(&[(format!("torrent.{a}.label"), Some("film".to_owned()))]);
    let labels = t.get(&sid, "/pa/add").json();
    assert!(labels.to_json().contains(r#""film""#), "{}", labels.to_json());
    assert_eq!(category(&t, &sid, a), "film");
    assert!(t.host.changes().is_empty(), "{:?}", t.host.changes());
    // a pause mark from the host turns a stopped torrent's cell into Paused (the next tick
    // carries it), and back
    let (mut ws, _) = Ws::connect(&t, &sid);
    ws.send_text(r#"{"t":"view","sort":["name",false],"columns":["name","status"],"window":[0,10]}"#);
    assert_eq!(t.pa(&sid, "/pa/torrents/stop", &format!(r#"{{"ids":["{b}"]}}"#)).status, 204);
    ws.state_until(b, "stopped");
    t.web.options_changed(&[(format!("torrent.{b}.paused"), Some("1".to_owned()))]);
    ws.state_until(b, "paused");
    t.web.options_changed(&[(format!("torrent.{b}.paused"), None)]);
    ws.state_until(b, "stopped");
    assert!(t.host.changes().is_empty(), "{:?}", t.host.changes());
    // a torrent the host removed a label from
    t.web.options_changed(&[(format!("torrent.{a}.label"), None)]);
    assert_eq!(category(&t, &sid, a), "");
    // an unknown torrent's label is kept for its first sight
    t.web.options_changed(&[("torrent.ffff.label".to_owned(), Some("later".to_owned()))]);
    assert!(t.get(&sid, "/pa/add").json().to_json().contains(r#""later""#));

    // a new password hash from the host: the session is over, the new login works
    let hash = PasswordHash::new("other-pass", 2).to_string();
    t.web.options_changed(&[("webui.password".to_owned(), Some(hash))]);
    assert_eq!(t.get(&sid, "/api/v2/app/version").status, 403);
    assert_eq!(t.login_as("admin", PASS).text(), "Fails.");
    let r = t.login_as("admin", "other-pass");
    assert_eq!((r.status, r.text().as_str()), (200, "Ok."));
    // a new user name too
    t.web.options_changed(&[("webui.username".to_owned(), Some("bob".to_owned()))]);
    assert_eq!(t.login_as("admin", "other-pass").text(), "Fails.");
    assert_eq!(t.login_as("bob", "other-pass").status, 200);
}

/// The API key is one of the host's keys (hashed): the host's key works from the start, a key
/// the host changes takes effect at once, and a browser's new key reaches the host as a hash.
#[test]
fn the_api_key_is_the_hosts_and_follows_its_changes() {
    const KEY1: &str = "first-key-0123456789";
    const KEY2: &str = "second-key-0123456789";
    let mut o = options_with_login("admin", PASS);
    o.set("webui.api_key_enabled", "1");
    o.set("webui.api_key", PasswordHash::new(KEY1, 2).to_string());
    let t = Harness::start("api-key", o, None);
    let bearer = |key: &str| {
        let auth = format!("Bearer {key}");
        t.req("GET", "/api/v2/app/version", &[("Authorization", &auth)], Body::None).status
    };
    assert_eq!(bearer(KEY1), 200);
    // a new key from the host: the old one stops working
    t.web.options_changed(&[("webui.api_key".to_owned(), Some(PasswordHash::new(KEY2, 2).to_string()))]);
    assert_eq!(bearer(KEY1), 403);
    assert_eq!(bearer(KEY2), 200);
    // switched off by the host
    t.web.options_changed(&[("webui.api_key_enabled".to_owned(), Some("0".to_owned()))]);
    assert_eq!(bearer(KEY2), 403);
    t.web.options_changed(&[("webui.api_key_enabled".to_owned(), Some("1".to_owned()))]);
    assert_eq!(bearer(KEY2), 200);

    // a browser's new key: the host stores the hash, not the key
    let sid = t.login();
    t.host.clear();
    let body = format!(r#"{{"webui.api_key":"{KEY1}","webui.current_password":"{PASS}"}}"#);
    let r = t.pa(&sid, "/pa/prefs", &body);
    assert_eq!(r.status, 204, "{}", r.text());
    let stored = t.host.value_of("webui.api_key").flatten().expect("the hash");
    assert!(
        !stored.contains(KEY1) && PasswordHash::parse(&stored).is_some_and(|h| h.verify(KEY1)),
        "{stored}"
    );
    assert_eq!(bearer(KEY1), 200);
    assert_eq!(bearer(KEY2), 403);
}

// ---- preferences ------------------------------------------------------------------------------

#[test]
fn a_preferences_change_reaches_the_host_and_not_the_engine() {
    let t = Harness::start("prefs", Prefs::default(), Some(PASS));
    let sid = t.login();
    let hashes = t.hashes(&sid);
    // the mock moves data: well above one byte a second
    t.wait_for(&sid, &hashes[0], "a speed", |x| x.get("dlspeed").and_then(Json::as_i64).unwrap_or(0) > 1);

    // (c) a global limit of one byte a second: the host gets `lt.download_rate_limit`
    let r = t.pa(&sid, "/pa/prefs", r#"{"download_rate_limit":1,"connections_limit":77}"#);
    assert_eq!(r.status, 204, "{}", r.text());
    assert_eq!(t.host.value_of("lt.download_rate_limit"), Some(Some("1".to_owned())));
    assert_eq!(t.host.value_of("lt.connections_limit"), Some(Some("77".to_owned())));
    // the pages show the new values
    let m = t.get(&sid, "/pa/prefs").json();
    let fields = m.get("fields").unwrap();
    assert_eq!(fields.get("download_rate_limit").unwrap().get("value"), Some(&Json::Int(1)));
    assert_eq!(t.get(&sid, "/api/v2/transfer/downloadLimit").text(), "1");
    // the engine was not told: the mock keeps moving data past the limit for a while
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut fast = 0;
    while Instant::now() < deadline {
        let info = t.get(&sid, "/api/v2/transfer/info").json();
        if info.get("dl_info_speed").and_then(Json::as_i64).unwrap_or(0) > 1 {
            fast += 1;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(fast > 5, "the mock engine was throttled: {fast} fast samples");

    // the host's journal back: the pages follow (a key removed is back at its default)
    t.web.options_changed(&[
        ("lt.download_rate_limit".to_owned(), None),
        ("lt.connections_limit".to_owned(), Some("123".to_owned())),
        ("ui.add_stopped".to_owned(), Some("1".to_owned())),
    ]);
    let m = t.get(&sid, "/pa/prefs").json();
    let fields = m.get("fields").unwrap();
    assert_eq!(fields.get("download_rate_limit").unwrap().get("value"), Some(&Json::Int(0)));
    assert_eq!(fields.get("connections_limit").unwrap().get("value"), Some(&Json::Int(123)));
    assert_eq!(fields.get("ui.add_stopped").unwrap().get("value"), Some(&Json::Bool(true)));
    assert_eq!(t.get(&sid, "/pa/add").json().get("start"), Some(&Json::Bool(false)));

    // the shim's setPreferences and the session limits take the same path; the host's own
    // options stay (the tray) while the Web UI's are written
    t.host.clear();
    assert_eq!(t.post(&sid, "/api/v2/app/setPreferences", r#"json={"max_connec":55}"#).status, 200);
    assert_eq!(t.host.value_of("lt.connections_limit"), Some(Some("55".to_owned())));
    assert!(t.host.value_of("ui.close_to_tray").is_none(), "{:?}", t.host.changes());
    assert_eq!(t.pa(&sid, "/pa/session/limits", r#"{"down":4096,"up":2048}"#).status, 204);
    assert_eq!(t.host.value_of("lt.download_rate_limit"), Some(Some("4096".to_owned())));
    assert_eq!(t.host.value_of("lt.upload_rate_limit"), Some(Some("2048".to_owned())));
    assert_eq!(t.post(&sid, "/api/v2/transfer/setUploadLimit", "limit=512").status, 200);
    assert_eq!(t.host.value_of("lt.upload_rate_limit"), Some(Some("512".to_owned())));
    let l = t.get(&sid, "/pa/session/limits").json();
    assert_eq!((l.get("down"), l.get("up")), (Some(&Json::Int(4096)), Some(&Json::Int(512))));

    // a new login from a browser: the host stores the hash, not the password
    t.host.clear();
    let body =
        format!(r#"{{"webui.username":"carol","webui.password":"n3w","webui.current_password":"{PASS}"}}"#);
    assert_eq!(t.pa(&sid, "/pa/prefs", &body).status, 204);
    assert_eq!(t.host.value_of("webui.username"), Some(Some("carol".to_owned())));
    let stored = t.host.value_of("webui.password").flatten().expect("the hash");
    assert!(
        !stored.contains("n3w") && PasswordHash::parse(&stored).is_some_and(|h| h.verify("n3w")),
        "{stored}"
    );
}

// ---- what is off --------------------------------------------------------------------------------

#[test]
fn rss_and_alternative_limits_are_off_and_shutdown_tells_the_host() {
    let t = Harness::start("off", Prefs::default(), Some(PASS));
    let sid = t.login();

    // (d) the page learns it from the hello
    let (_ws, hello) = Ws::connect(&t, &sid);
    assert_eq!(hello.get("hosted"), Some(&Json::Bool(true)), "{}", hello.to_json());
    // no RSS Downloader: 404 with the reason, natively and through the shim
    let r = t.get(&sid, "/pa/rss");
    assert_eq!(r.status, 404, "{}", r.text());
    assert!(s(r.json().get("error")).contains("desktop client"), "{}", r.text());
    assert_eq!(t.pa(&sid, "/pa/rss/feed-add", r#"{"url":"http://x/"}"#).status, 404);
    let r = t.get(&sid, "/api/v2/rss/items");
    assert_eq!(r.status, 404, "{}", r.text());
    assert!(s(r.json().get("error")).contains("desktop client"), "{}", r.text());
    // no alternative speed limits: the Bandwidth page has no such row, and switching them on
    // is refused (400 names the field; 409 from the limits and the shim)
    let m = t.get(&sid, "/pa/prefs").json();
    let Some(Json::Arr(pages)) = m.get("pages") else { panic!() };
    let bandwidth = pages.iter().find(|p| s(p.get("id")) == "bandwidth").unwrap().to_json();
    assert!(!bandwidth.contains("alt_enabled") && bandwidth.contains("download_rate_limit"), "{bandwidth}");
    let r = t.pa(&sid, "/pa/prefs", r#"{"alt_enabled":true}"#);
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(s(r.json().get("error")).starts_with("alt_enabled: "), "{}", r.text());
    assert_eq!(t.pa(&sid, "/pa/prefs", r#"{"alt_enabled":false}"#).status, 204);
    let r = t.pa(&sid, "/pa/session/limits", r#"{"alt":true}"#);
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(t.pa(&sid, "/pa/session/limits", r#"{"alt_down":512}"#).status, 409);
    assert_eq!(t.post(&sid, "/api/v2/transfer/toggleSpeedLimitsMode", "").status, 409);
    assert_eq!(t.post(&sid, "/api/v2/transfer/setSpeedLimitsMode", "mode=1").status, 409);
    assert_eq!(t.get(&sid, "/api/v2/transfer/speedLimitsMode").text(), "0");
    assert_eq!(t.get(&sid, "/pa/session/limits").json().get("alt"), Some(&Json::Bool(false)));

    // (e) shutdown: the host is told
    assert!(!t.host.shutdown.load(Ordering::SeqCst));
    assert_eq!(t.post(&sid, "/api/v2/app/shutdown", "").status, 200);
    assert!(t.host.shutdown.load(Ordering::SeqCst));
    assert!(t.web.shutdown_requested());
}

// ---- the start ------------------------------------------------------------------------------------

#[test]
fn the_login_and_labels_come_from_the_hosts_options_and_nothing_is_written() {
    // the mock's library is the seed's: the same hashes at every start
    let first = {
        let t = Harness::start("start-hashes", Prefs::default(), Some(PASS));
        let sid = t.login();
        t.hashes(&sid)[0].clone()
    };
    // (f) the host's login, a label of a torrent not seen yet and one of a known torrent
    let mut options = options_with_login("bob", PASS);
    options.set("torrent.ffff.label", "archive");
    options.set(format!("torrent.{first}.label").as_str(), "iso");
    options.set(format!("torrent.{first}.paused").as_str(), "1");
    options.set("lt.connections_limit", "99");
    options.set("ui.close_to_tray", "0");
    let t = Harness::start("start", options, None);
    assert!(t.web.temporary_password().is_none(), "the stored hash is the login");
    assert_eq!(t.login_as("admin", PASS).text(), "Fails.");
    let r = t.login_as("bob", PASS);
    assert_eq!((r.status, r.text().as_str()), (200, "Ok."));
    let sid = r.header("set-cookie").unwrap().split(';').next().unwrap().to_owned();
    // the label list is complete before the torrents show up
    let labels = t.get(&sid, "/pa/add").json().get("labels").unwrap().to_json();
    assert!(labels.contains(r#""archive""#) && labels.contains(r#""iso""#), "{labels}");
    // first sight: the host's label, and its pause mark (Stop clears it, so the host hears)
    t.wait_for(&sid, &first, "the label", |x| s(x.get("category")) == "iso");
    assert!(t.host.changes().is_empty(), "{:?}", t.host.changes());
    assert_eq!(t.pa(&sid, "/pa/torrents/stop", &format!(r#"{{"ids":["{first}"]}}"#)).status, 204);
    assert_eq!(t.host.value_of(&format!("torrent.{first}.paused")), Some(None));
    // the host's settings on the pages, and its own options untouched by a commit
    let m = t.get(&sid, "/pa/prefs").json();
    assert_eq!(m.get("fields").unwrap().get("connections_limit").unwrap().get("value"), Some(&Json::Int(99)));
    assert_eq!(t.pa(&sid, "/pa/prefs", r#"{"ui.confirm_delete":false}"#).status, 204);
    assert_eq!(t.host.value_of("ui.confirm_delete"), Some(Some("0".to_owned())));
    assert!(t.host.value_of("ui.close_to_tray").is_none(), "{:?}", t.host.changes());
    assert!(t.host.value_of("webui.alt_enabled").is_none(), "{:?}", t.host.changes());
    // no state file, nothing else on disk
    assert!(!t.dir.join("state").join(STATE_FILE).exists());
    assert!(!t.dir.join("state").exists());
}

// ---- Sonarr, Radarr and Prowlarr ------------------------------------------------------------

/// What the *arr apps do (docs/INTEGRATIONS.md part 1) against the GUI's hosted Web UI: the
/// window's seeding goal is the global share limit, an add's limits and category become the
/// host's own `torrent.<key>.*` keys (the window stops the torrent), and removing a category or
/// the torrent drops them again.
#[test]
fn the_arr_apps_limits_and_categories_are_the_hosts_keys() {
    // the window's goal: no ratio, 30 minutes, then stop
    let mut options = Prefs::default();
    options.set("queue.seed_ratio", 0);
    options.set("queue.seed_time", 30);
    let t = Harness::start("arr", options, Some(PASS));
    let sid = t.login();
    let num = |v: Option<&Json>| v.and_then(Json::as_f64).unwrap_or(f64::NAN);
    let limits = |x: &Json| (num(x.get("ratio_limit")), num(x.get("seeding_time_limit")));
    let key = |h: &str, k: &str| t.host.value_of(&format!("torrent.{h}.{k}"));

    let p = t.get(&sid, "/api/v2/app/preferences").json();
    assert_eq!(p.get("max_ratio_enabled").and_then(Json::as_bool), Some(false));
    assert_eq!(p.get("max_seeding_time_enabled").and_then(Json::as_bool), Some(true));
    assert_eq!(num(p.get("max_seeding_time")), 30.0);
    assert_eq!(num(p.get("max_ratio_act")), 0.0, "stop, never remove");

    // an *arr's add: capitalised booleans, the limits and the category beside the magnet
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let form = format!(
        "urls=magnet%3A%3Fxt%3Durn%3Abtih%3A{hash}%26dn%3Darr&stopped=True&category=tv-sonarr\
         &ratioLimit=2.5&seedingTimeLimit=60"
    );
    let r = t.post(&sid, "/api/v2/torrents/add", &form);
    assert_eq!((r.status, r.text().as_str()), (200, "Ok."));
    assert_eq!(key(hash, "label"), Some(Some("tv-sonarr".to_owned())));
    assert_eq!(key(hash, "seed_ratio"), Some(Some("250".to_owned())));
    assert_eq!(key(hash, "seed_time"), Some(Some("60".to_owned())));
    let x = t.wait_for(&sid, hash, "the added torrent", |x| s(x.get("category")) == "tv-sonarr");
    assert_eq!(limits(&x), (2.5, 60.0));

    // `setShareLimits` takes the same path
    t.host.clear();
    let r = t.post(
        &sid,
        "/api/v2/torrents/setShareLimits",
        &format!("hashes={hash}&ratioLimit=-1&seedingTimeLimit=90"),
    );
    assert_eq!(r.status, 200);
    assert_eq!(key(hash, "seed_time"), Some(Some("90".to_owned())));
    assert_eq!(
        limits(&first(&t.get(&sid, &format!("/api/v2/torrents/info?hashes={hash}")).json())),
        (-1.0, 90.0)
    );

    // removing the category drops the window's label too, so it does not come back
    t.host.clear();
    assert_eq!(t.post(&sid, "/api/v2/torrents/removeCategories", "categories=tv-sonarr").status, 200);
    assert_eq!(key(hash, "label"), Some(None));
    assert_eq!(category(&t, &sid, hash), "");
    let cats = t.get(&sid, "/api/v2/torrents/categories").json().to_json();
    assert!(!cats.contains("tv-sonarr"), "{cats}");

    // Remove Completed: the torrent and its keys go
    t.host.clear();
    let r = t.post(&sid, "/api/v2/torrents/delete", &format!("hashes={hash}&deleteFiles=true"));
    assert_eq!(r.status, 200);
    let deadline = Instant::now() + Duration::from_secs(20);
    while key(hash, "seed_ratio") != Some(None) {
        assert!(Instant::now() < deadline, "the goal stayed: {:?}", t.host.changes());
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(t.get(&sid, &format!("/api/v2/torrents/info?hashes={hash}")).json().to_json(), "[]");
}
