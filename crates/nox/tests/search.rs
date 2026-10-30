//! The torrent search in nox over loopback (D41): a fake Prowlarr (`support/torznab.rs`)
//! answering with captured Prowlarr documents, the native `/pa/search...` endpoints (sources
//! with the password rule, Test, start, the merged results, stop, adding a `.torrent` result
//! and a result that redirects to a magnet), and the API key never in an answer or in the
//! state file. The tick's `search` version is checked in `hosted.rs`.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use patorrent_core::{Session, SessionParams, Settings};
use patorrent_nox::webui::json::{self, Json};
use patorrent_nox::webui::{Config, STATE_FILE, WebUi};

#[path = "support/torznab.rs"]
mod torznab;

use torznab::{FAWLTY, GRAB, KEY, MAGNET_ONLY, NIGHT};

const PASS: &str = "search-pass";

struct Harness {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
    session: Option<Session>,
    dir: PathBuf,
    /// Every answer body, to check that the key never shows.
    bodies: std::sync::Mutex<Vec<String>>,
}

impl Harness {
    fn start(name: &str) -> Harness {
        let dir = std::env::temp_dir().join(format!("nox-search-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let settings = Settings {
            listen_interfaces: "127.0.0.1:0".into(),
            enable_dht: false,
            enable_lsd: false,
            enable_upnp: false,
            enable_natpmp: false,
            ..Settings::default()
        };
        let session = Session::start(SessionParams { settings: settings.clone(), state_dir: None }).unwrap();
        let h = session.handle();
        let mut c = Config::new("127.0.0.1:0".parse().unwrap(), settings, dir.join("dl").to_string_lossy());
        c.password = Some(PASS.into());
        c.state_dir = Some(dir.join("config"));
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
        Harness { addr, stop, pump: Some(pump), session: Some(session), dir, bodies: Default::default() }
    }

    fn req(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
        self.req_as(method, path, headers, "application/json", body)
    }

    fn req_as(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        content_type: &str,
        body: &str,
    ) -> (u16, String) {
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(40))).unwrap();
        let mut r = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n", self.addr);
        for (n, v) in headers {
            r.push_str(&format!("{n}: {v}\r\n"));
        }
        if !body.is_empty() {
            r.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        r.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        s.write_all(r.as_bytes()).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let text = String::from_utf8_lossy(&out).into_owned();
        let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        self.bodies.lock().unwrap().push(text.clone());
        let body = text.split_once("\r\n\r\n").map(|x| x.1.to_owned()).unwrap_or_default();
        (status, body)
    }

    fn login(&self) -> String {
        let mut st = TcpStream::connect(self.addr).unwrap();
        let body = format!("username=admin&password={PASS}");
        let r = format!(
            "POST /api/v2/auth/login HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            self.addr,
            body.len()
        );
        st.write_all(r.as_bytes()).unwrap();
        let mut out = String::new();
        st.read_to_string(&mut out).unwrap();
        let line = out.lines().find(|l| l.to_ascii_lowercase().starts_with("set-cookie:")).unwrap();
        line.split_once(':').unwrap().1.trim().split(';').next().unwrap().to_owned()
    }

    fn get(&self, sid: &str, path: &str) -> (u16, String) {
        self.req("GET", path, &[("Cookie", sid)], "")
    }

    fn model(&self, sid: &str) -> Json {
        let (s, body) = self.get(sid, "/pa/search");
        assert_eq!(s, 200, "{body}");
        json::parse(&body).unwrap_or_else(|e| panic!("{e}: {body}"))
    }

    fn pa(&self, sid: &str, path: &str, body: &str) -> (u16, String) {
        let origin = format!("http://{}", self.addr);
        self.req(
            "POST",
            path,
            &[("Cookie", sid), ("Origin", &origin), ("X-Requested-With", "PaTorrent")],
            body,
        )
    }

    /// `GET /pa/search` until `f` holds.
    fn model_until(&self, sid: &str, what: &str, f: impl Fn(&Json) -> bool) -> Json {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let m = self.model(sid);
            if f(&m) {
                return m;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}: {}", m.to_json());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A qBittorrent shim call: a form POST, the session cookie only.
    fn shim(&self, sid: &str, path: &str, form: &str) -> (u16, String) {
        self.req_as("POST", path, &[("Cookie", sid)], "application/x-www-form-urlencoded", form)
    }

    fn shim_json(&self, sid: &str, path: &str, form: &str) -> Json {
        let (st, body) = self.shim(sid, path, form);
        assert_eq!(st, 200, "{path} {form}: {body}");
        json::parse(&body).unwrap_or_else(|e| panic!("{e}: {body}"))
    }

    /// The listed torrents' hashes.
    fn hashes(&self, sid: &str) -> Vec<String> {
        let (_, body) = self.get(sid, "/api/v2/torrents/info");
        arr(Some(&json::parse(&body).unwrap())).iter().map(|t| s(t.get("hash")).to_owned()).collect()
    }

    fn wait_for_hash(&self, sid: &str, hash: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.hashes(sid).iter().any(|h| h == hash) {
            assert!(Instant::now() < deadline, "{hash} never listed: {:?}", self.hashes(sid));
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn names(&self, sid: &str) -> Vec<String> {
        let (_, body) = self.get(sid, "/api/v2/torrents/info");
        let list = json::parse(&body).unwrap();
        let Json::Arr(v) = list else { panic!("{body}") };
        v.iter().map(|t| s(t.get("name")).to_owned()).collect()
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

fn s(v: Option<&Json>) -> &str {
    v.and_then(Json::as_str).unwrap_or("")
}

fn arr(v: Option<&Json>) -> &[Json] {
    match v {
        Some(Json::Arr(a)) => a,
        _ => &[],
    }
}

fn int(v: Option<&Json>) -> i64 {
    v.and_then(Json::as_i64).unwrap_or(-1)
}

fn idle(m: &Json) -> bool {
    m.get("running") == Some(&Json::Bool(false))
}

fn result_id(m: &Json, name: &str) -> String {
    let r = arr(m.get("results")).iter().find(|r| s(r.get("name")) == name).expect(name);
    s(r.get("id")).to_owned()
}

#[test]
fn sources_search_and_add() {
    let fake = torznab::start();
    let t = Harness::start("main");
    let sid = t.login();
    let good = fake.url("/1/api");
    let m = t.model(&sid);
    assert!(arr(m.get("sources")).is_empty() && idle(&m));
    assert_eq!(arr(m.get("main_categories")).len(), 6);
    // nothing to search yet
    assert_eq!(t.pa(&sid, "/pa/search/start", r#"{"q":"x"}"#).0, 409);

    // ---- adding a source needs the current password
    let add = |name: &str, url: &str, key: &str, pass: Option<&str>| {
        let pass = pass.map_or(String::new(), |p| format!(r#","password":"{p}""#));
        t.pa(
            &sid,
            "/pa/search/source-add",
            &format!(r#"{{"name":"{name}","url":"{url}","key":"{key}"{pass}}}"#),
        )
    };
    let (st, body) = add("Prowlarr", &good, KEY, None);
    assert_eq!(st, 400, "{body}");
    assert!(body.contains("current Web UI password"), "{body}");
    assert_eq!(add("Prowlarr", "ftp://x/api", KEY, Some(PASS)).0, 400, "http(s) only");
    let (st, body) = add("Prowlarr", &good, KEY, Some(PASS));
    assert_eq!((st, body.as_str()), (200, r#"{"index":0}"#));
    assert_eq!(add("Bad key", &fake.url("/9/api"), "K2-secret", Some(PASS)).0, 200);
    assert_eq!(add("", &fake.url("/slow/api"), KEY, Some(PASS)).0, 200);
    // a source left out of "All sources"
    assert_eq!(t.pa(&sid, "/pa/search/source-edit", r#"{"index":2,"enabled":false}"#).0, 204);
    let m = t.model(&sid);
    let srcs = arr(m.get("sources"));
    assert_eq!(srcs.len(), 3);
    assert_eq!(s(srcs[0].get("name")), "Prowlarr");
    assert_eq!(s(srcs[0].get("url_redacted")), good);
    assert_eq!(srcs[0].get("has_key"), Some(&Json::Bool(true)));
    assert_eq!(s(srcs[2].get("name")), "127.0.0.1", "no name: the host");
    assert_eq!(srcs[2].get("enabled"), Some(&Json::Bool(false)));

    // ---- editing: a name or the switch alone needs no password; a URL or a key does
    let (st, _) = t.pa(&sid, "/pa/search/source-edit", r#"{"index":1,"name":"Jackett-like"}"#);
    assert_eq!(st, 204);
    let moved = format!(r#"{{"index":0,"url":"{}","key":""}}"#, fake.url("/elsewhere/api"));
    assert_eq!(t.pa(&sid, "/pa/search/source-edit", &moved).0, 400, "a new URL with the stored key");
    assert_eq!(t.pa(&sid, "/pa/search/source-edit", r#"{"index":0,"key":"stolen"}"#).0, 400);
    assert_eq!(t.pa(&sid, "/pa/search/source-edit", r#"{"index":7,"name":"x"}"#).0, 404);
    // the same URL and an empty key: unchanged, no password
    let same = format!(r#"{{"index":0,"url":"{good}","key":"","name":"Prowlarr"}}"#);
    assert_eq!(t.pa(&sid, "/pa/search/source-edit", &same).0, 204);

    // ---- Test: the caps, or the error, without the key
    let (st, body) = t.pa(&sid, "/pa/search/source-test", r#"{"index":0}"#);
    assert_eq!(st, 200, "{body}");
    let r = json::parse(&body).unwrap();
    assert_eq!((r.get("ok"), int(r.get("categories"))), (Some(&Json::Bool(true)), 8), "{body}");
    assert_eq!(s(r.get("text")), "Prowlarr: OK, 8 categories");
    let (_, body) = t.pa(&sid, "/pa/search/source-test", r#"{"index":1}"#);
    let r = json::parse(&body).unwrap();
    assert_eq!(r.get("ok"), Some(&Json::Bool(false)));
    assert_eq!(s(r.get("error")), "100: Invalid API Key", "{body}");

    // ---- a search over the enabled sources
    let (st, body) = t.pa(&sid, "/pa/search/start", r#"{"q":"fawlty towers","categories":[5000]}"#);
    assert_eq!(st, 204, "{body}");
    let m = t.model_until(&sid, "the answers", idle);
    let statuses = arr(m.get("statuses"));
    assert_eq!(statuses.len(), 2, "the disabled source is left out: {}", m.to_json());
    assert_eq!((s(statuses[0].get("status")), int(statuses[0].get("count"))), ("done", 3));
    assert_eq!(s(statuses[0].get("text")), "Prowlarr: 3 results");
    assert_eq!(s(statuses[1].get("status")), "failed");
    assert_eq!(s(statuses[1].get("text")), "Jackett-like: 100: Invalid API Key");
    assert_eq!(s(m.get("query").and_then(|q| q.get("q"))), "fawlty towers");
    let results = arr(m.get("results"));
    assert_eq!(results.len(), 3);
    assert_eq!(int(m.get("total")), 3);
    let fawlty = results.iter().find(|r| s(r.get("name")) == FAWLTY).unwrap();
    assert_eq!(int(fawlty.get("seeders")), 25);
    assert_eq!(int(fawlty.get("peers")), 30);
    assert_eq!(int(fawlty.get("size")), 122_170_147);
    assert_eq!(s(fawlty.get("category_text")), "TV");
    assert_eq!(s(fawlty.get("info_hash")), "58cebaa968651dc2a241c495719f2b6cbec33e32");
    assert_eq!(arr(fawlty.get("source_names")), &[Json::from("Prowlarr")]);
    assert!(fawlty.get("link").is_none(), "links carry the indexer's key");
    // what the source was asked
    let asked = fake.requests.lock().unwrap().clone();
    assert!(
        asked.iter().any(|r| r.starts_with("/1/api?t=search&q=fawlty%20towers&cat=5000&limit=100&apikey=")),
        "{asked:?}"
    );

    // ---- adding: a .torrent, a redirect to a magnet, a link that gives neither
    let (st, body) = t.pa(&sid, "/pa/search/add", &format!(r#"{{"id":"{}"}}"#, result_id(&m, FAWLTY)));
    assert_eq!(st, 200, "{body}");
    let hash = torznab::torrent("fawlty").1;
    assert!(body.contains(&hash), "{body}");
    let (st, body) = t.pa(&sid, "/pa/search/add", &format!(r#"{{"id":"{}"}}"#, result_id(&m, NIGHT)));
    assert_eq!(st, 200, "{body}");
    assert!(body.contains(&torznab::night_magnet().1), "{body}");
    let (st, body) = t.pa(&sid, "/pa/search/add", &format!(r#"{{"id":"{}"}}"#, result_id(&m, GRAB)));
    assert_eq!(st, 400, "{body}");
    assert_eq!(body, r#"{"error":"Prowlarr: HTTP 404 Not Found"}"#);
    assert_eq!(t.pa(&sid, "/pa/search/add", r#"{"id":"nope"}"#).0, 404);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !t.names(&sid).iter().any(|n| n == "fawlty") {
        assert!(Instant::now() < deadline, "{:?}", t.names(&sid));
        std::thread::sleep(Duration::from_millis(50));
    }
    // a second add of the same torrent
    let (st, _) = t.pa(&sid, "/pa/search/add", &format!(r#"{{"id":"{}"}}"#, result_id(&m, NIGHT)));
    assert_eq!(st, 409);

    // ---- a named source is searched even when switched off; stop drops its answer
    assert_eq!(t.pa(&sid, "/pa/search/start", r#"{"q":"slow","sources":[2]}"#).0, 204);
    let m = t.model(&sid);
    assert_eq!(m.get("running"), Some(&Json::Bool(true)));
    assert_eq!(s(arr(m.get("statuses"))[0].get("text")), "127.0.0.1: searching...");
    assert!(arr(m.get("results")).is_empty(), "a new search starts empty");
    assert_eq!(t.pa(&sid, "/pa/search/stop", "{}").0, 204);
    let m = t.model(&sid);
    assert!(idle(&m));
    assert_eq!(s(arr(m.get("statuses"))[0].get("status")), "stopped");
    std::thread::sleep(Duration::from_millis(3500));
    assert!(arr(t.model(&sid).get("results")).is_empty(), "the late answer is dropped");
    assert_eq!(t.pa(&sid, "/pa/search/start", r#"{"q":"x","sources":[9]}"#).0, 404);
    assert_eq!(t.pa(&sid, "/pa/search/start", r#"{"q":"  "}"#).0, 400);

    // ---- the key is sealed in the state file (Windows: DPAPI), and never in an answer
    let state = std::fs::read_to_string(t.dir.join("config").join(STATE_FILE)).unwrap();
    assert!(state.contains("search.source.0.url"), "{state}");
    if cfg!(windows) {
        assert!(!state.contains(KEY) && !state.contains("K2-secret"), "{state}");
    }
    for b in t.bodies.lock().unwrap().iter() {
        assert!(!b.contains(KEY) && !b.contains("K2-secret"), "{b}");
    }

    // ---- removing renumbers
    assert_eq!(t.pa(&sid, "/pa/search/source-remove", r#"{"index":0}"#).0, 204);
    let m = t.model(&sid);
    let names: Vec<&str> = arr(m.get("sources")).iter().map(|x| s(x.get("name"))).collect();
    assert_eq!(names, ["Jackett-like", "127.0.0.1"]);
    assert_eq!(t.pa(&sid, "/pa/search/source-remove", r#"{"index":5}"#).0, 404);
    // GET only reads; the commands need the CSRF header
    let (st, _) = t.req("POST", "/pa/search/stop", &[("Cookie", &sid)], "{}");
    assert_eq!(st, 403);
    assert_eq!(t.get(&sid, "/pa/search/stop").0, 405);
}

/// A form value, percent-encoded.
fn enc(v: &str) -> String {
    v.bytes()
        .map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// qBittorrent's `search/*` on the shim: each source a plugin (a stable name, no key), searches
/// as jobs (start, status, results paged by offset in arrival order, stop, delete, at most
/// five running), plugins switched off and uninstalled, installing refused, and results added
/// through their `fileUrl` (a magnet, or the opaque `patorrent-search:` URL the server fetches
/// itself) with `torrents/add` and `search/downloadTorrent`. The key is in no answer.
#[test]
fn qbittorrent_search_shim() {
    let fake = torznab::start();
    let t = Harness::start("shim");
    let sid = t.login();
    let src = |name: &str, path: &str, key: &str| {
        let body =
            format!(r#"{{"name":"{name}","url":"{}","key":"{key}","password":"{PASS}"}}"#, fake.url(path));
        assert_eq!(t.pa(&sid, "/pa/search/source-add", &body).0, 200);
    };
    // nothing to search yet
    assert_eq!(t.shim(&sid, "/api/v2/search/start", "pattern=x&category=all&plugins=enabled").0, 409);
    src("Prowlarr", "/1/api", KEY);
    src("Bad key", "/9/api", "K2-secret");
    src("Magnets", "/mag/api", KEY);
    src("Slow", "/slow/api", KEY);
    assert_eq!(t.pa(&sid, "/pa/search/source-edit", r#"{"index":3,"enabled":false}"#).0, 204);

    // ---- the sources as plugins
    let (st, body) = t.get(&sid, "/api/v2/search/plugins");
    assert_eq!(st, 200, "{body}");
    let plugins = json::parse(&body).unwrap();
    let plugins = arr(Some(&plugins));
    assert_eq!(plugins.len(), 4);
    let names: Vec<String> = plugins.iter().map(|p| s(p.get("name")).to_owned()).collect();
    for n in &names {
        assert!(n.starts_with("torznab-") && n.len() == 16, "{n}");
    }
    assert_eq!(names.iter().collect::<std::collections::HashSet<_>>().len(), 4, "{names:?}");
    let (prowlarr, bad, magnets, slow) = (&names[0], &names[1], &names[2], &names[3]);
    assert_eq!(s(plugins[0].get("fullName")), "Prowlarr");
    assert_eq!(s(plugins[0].get("url")), format!("http://{}", fake.addr));
    assert_eq!(plugins[0].get("enabled"), Some(&Json::Bool(true)));
    assert_eq!(plugins[3].get("enabled"), Some(&Json::Bool(false)));
    let cats: Vec<&str> = arr(plugins[0].get("supportedCategories")).iter().map(|c| s(c.get("id"))).collect();
    assert_eq!(cats, ["all", "anime", "books", "games", "movies", "music", "software", "tv"]);
    assert_eq!(t.shim(&sid, "/api/v2/search/installPlugin", "sources=http%3A%2F%2Fx%2Fp.py").0, 409);
    assert_eq!(t.shim(&sid, "/api/v2/search/updatePlugins", "").0, 200);
    assert_eq!(t.get(&sid, "/api/v2/search/start?pattern=x&category=all&plugins=all").0, 405);
    assert_eq!(t.shim(&sid, "/api/v2/search/start", "pattern=x").0, 400);
    assert_eq!(t.shim(&sid, "/api/v2/search/start", "pattern=x&category=all&plugins=nope").0, 409);

    // ---- a search over the enabled plugins
    let r = t.shim_json(&sid, "/api/v2/search/start", "pattern=fawlty%20towers&category=tv&plugins=enabled");
    assert_eq!(int(r.get("id")), 1);
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        let st = t.shim_json(&sid, "/api/v2/search/status", "id=1");
        let st = arr(Some(&st))[0].clone();
        if s(st.get("status")) == "Stopped" {
            break st;
        }
        assert!(Instant::now() < deadline, "{}", st.to_json());
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(int(status.get("total")), 4, "{}", status.to_json());
    assert_eq!((s(status.get("pattern")), s(status.get("category"))), ("fawlty towers", "tv"));
    let used: Vec<&str> = arr(status.get("plugins")).iter().map(|p| s(Some(p))).collect();
    assert_eq!(used, [prowlarr.as_str(), bad.as_str(), magnets.as_str()], "the switched-off one is left out");
    let asked = fake.requests.lock().unwrap().clone();
    assert!(asked.iter().any(|r| r.starts_with("/1/api?t=search&q=fawlty%20towers&cat=5000&")), "{asked:?}");
    // GET works for the readers; no id lists every search
    let (st, body) = t.get(&sid, "/api/v2/search/status");
    assert_eq!(st, 200);
    assert_eq!(arr(Some(&json::parse(&body).unwrap())).len(), 1);

    let all = t.shim_json(&sid, "/api/v2/search/results", "id=1");
    assert_eq!((s(all.get("status")), int(all.get("total"))), ("Stopped", 4));
    let results = arr(all.get("results"));
    assert_eq!(results.len(), 4);
    let find = |name: &str| results.iter().find(|r| s(r.get("fileName")) == name).expect(name).clone();
    let fawlty = find(FAWLTY);
    assert!(s(fawlty.get("fileUrl")).starts_with("patorrent-search:1/"), "{}", fawlty.to_json());
    assert_eq!(int(fawlty.get("fileSize")), 122_170_147);
    assert_eq!((int(fawlty.get("nbSeeders")), int(fawlty.get("nbLeechers"))), (25, 5));
    assert_eq!(s(fawlty.get("engineName")), prowlarr);
    assert_eq!(s(fawlty.get("engineFullName")), "Prowlarr");
    assert_eq!(s(fawlty.get("siteUrl")), format!("http://{}", fake.addr));
    assert!(int(fawlty.get("pubDate")) > 1_700_000_000);
    let magnet = find(MAGNET_ONLY);
    assert_eq!(s(magnet.get("fileUrl")), torznab::magnet_only().0);
    assert_eq!((int(magnet.get("nbSeeders")), int(magnet.get("nbLeechers"))), (3, -1));
    assert_eq!(s(magnet.get("engineName")), magnets);
    // paging: arrival order, offset from the end, out of range
    let page = t.shim_json(&sid, "/api/v2/search/results", "id=1&offset=1&limit=2");
    assert_eq!(arr(page.get("results")), &results[1..3]);
    assert_eq!(int(page.get("total")), 4);
    let last = t.shim_json(&sid, "/api/v2/search/results", "id=1&offset=-1");
    assert_eq!(arr(last.get("results")), &results[3..]);
    assert_eq!(arr(t.shim_json(&sid, "/api/v2/search/results", "id=1&offset=4").get("results")).len(), 0);
    assert_eq!(t.shim(&sid, "/api/v2/search/results", "id=1&offset=5").0, 409);
    assert_eq!(t.shim(&sid, "/api/v2/search/results", "id=99").0, 404);
    assert_eq!(t.shim(&sid, "/api/v2/search/status", "id=99").0, 404);

    // ---- adding by fileUrl: the opaque URL with torrents/add and downloadTorrent, a magnet
    let (st, body) = t.shim(&sid, "/api/v2/torrents/add", &format!("urls={}", enc(s(fawlty.get("fileUrl")))));
    assert_eq!((st, body.as_str()), (200, "Ok."));
    t.wait_for_hash(&sid, &torznab::torrent("fawlty").1);
    let night = find(NIGHT);
    let form = format!("torrentUrl={}&pluginName={prowlarr}", enc(s(night.get("fileUrl"))));
    assert_eq!(t.shim(&sid, "/api/v2/search/downloadTorrent", &form).0, 200);
    t.wait_for_hash(&sid, &torznab::night_magnet().1);
    let form = format!("torrentUrl={}&pluginName={magnets}", enc(s(magnet.get("fileUrl"))));
    assert_eq!(t.shim(&sid, "/api/v2/search/downloadTorrent", &form).0, 200);
    t.wait_for_hash(&sid, &torznab::magnet_only().1);
    // a link that gives no torrent, and a URL of no search
    let grab = find(GRAB);
    let form = format!("torrentUrl={}&pluginName={prowlarr}", enc(s(grab.get("fileUrl"))));
    assert_eq!(t.shim(&sid, "/api/v2/search/downloadTorrent", &form).0, 400);
    let (st, body) = t.shim(&sid, "/api/v2/torrents/add", "urls=patorrent-search%3A99%2Fx");
    assert_eq!((st, body.as_str()), (200, "Fails."));

    // ---- a named plugin is searched even when switched off; stop; five running at most
    let slow_form = format!("pattern=slow&category=all&plugins={slow}");
    let id = int(t.shim_json(&sid, "/api/v2/search/start", &slow_form).get("id"));
    assert_eq!(id, 2);
    let st = t.shim_json(&sid, "/api/v2/search/status", "id=2");
    assert_eq!(s(arr(Some(&st))[0].get("status")), "Running");
    assert_eq!(t.shim(&sid, "/api/v2/search/stop", "id=2").0, 200);
    let st = t.shim_json(&sid, "/api/v2/search/status", "id=2");
    assert_eq!(s(arr(Some(&st))[0].get("status")), "Stopped");
    assert_eq!(t.shim(&sid, "/api/v2/search/stop", "id=42").0, 404);
    for want in 3..=7 {
        assert_eq!(int(t.shim_json(&sid, "/api/v2/search/start", &slow_form).get("id")), want);
    }
    let (st, body) = t.shim(&sid, "/api/v2/search/start", &slow_form);
    assert_eq!((st, body.as_str()), (409, "Unable to create more than 5 concurrent searches."));
    assert_eq!(t.shim(&sid, "/api/v2/search/delete", "id=7").0, 200);
    assert_eq!(int(t.shim_json(&sid, "/api/v2/search/start", &slow_form).get("id")), 8);
    assert_eq!(t.shim(&sid, "/api/v2/search/delete", "id=1").0, 200);
    assert_eq!(t.shim(&sid, "/api/v2/search/results", "id=1").0, 404);
    assert_eq!(t.shim(&sid, "/api/v2/search/delete", "id=1").0, 404);
    let all_jobs = t.shim_json(&sid, "/api/v2/search/status", "");
    let ids: Vec<i64> = arr(Some(&all_jobs)).iter().map(|j| int(j.get("id"))).collect();
    assert_eq!(ids, [2, 3, 4, 5, 6, 8]);

    // ---- switching off and uninstalling: the native sources follow, the other names stay
    let form = format!("names={bad}&enable=false");
    assert_eq!(t.shim(&sid, "/api/v2/search/enablePlugin", &form).0, 200);
    let m = t.model(&sid);
    assert_eq!(arr(m.get("sources"))[1].get("enabled"), Some(&Json::Bool(false)));
    assert_eq!(t.shim(&sid, "/api/v2/search/enablePlugin", &format!("names={bad}")).0, 400);
    assert_eq!(t.get(&sid, &format!("/api/v2/search/uninstallPlugin?names={prowlarr}")).0, 405);
    let form = format!("names={prowlarr}%7C{bad}");
    assert_eq!(t.shim(&sid, "/api/v2/search/uninstallPlugin", &form).0, 200);
    let (_, body) = t.get(&sid, "/api/v2/search/plugins");
    let left: Vec<String> =
        arr(Some(&json::parse(&body).unwrap())).iter().map(|p| s(p.get("name")).to_owned()).collect();
    assert_eq!(left, [magnets.clone(), slow.clone()]);
    let m = t.model(&sid);
    let srcs: Vec<&str> = arr(m.get("sources")).iter().map(|x| s(x.get("name"))).collect();
    assert_eq!(srcs, ["Magnets", "Slow"]);

    for b in t.bodies.lock().unwrap().iter() {
        assert!(!b.contains(KEY) && !b.contains("K2-secret"), "{b}");
    }
}

#[test]
fn a_wrong_password_ends_the_session() {
    let fake = torznab::start();
    let t = Harness::start("wrong-pass");
    let sid = t.login();
    let body = format!(r#"{{"url":"{}","key":"k","password":"guess"}}"#, fake.url("/1/api"));
    let (st, body) = t.pa(&sid, "/pa/search/source-add", &body);
    assert_eq!(st, 400, "{body}");
    assert_eq!(t.get(&sid, "/pa/search").0, 403, "the session is gone");
}

/// Against a real Prowlarr or Jackett (run by hand, `--ignored`): `PATORRENT_LIVE_TORZNAB` is
/// a source's Torznab URL, `PATORRENT_LIVE_KEY_FILE` a file holding its key,
/// `PATORRENT_LIVE_QUERY` the query (default `test`). The engine listens on loopback with DHT,
/// LSD, UPnP and NAT-PMP off; one result is added. Nothing printed holds the key.
#[test]
#[ignore]
fn live_torznab_source() {
    let (Ok(url), Ok(file)) =
        (std::env::var("PATORRENT_LIVE_TORZNAB"), std::env::var("PATORRENT_LIVE_KEY_FILE"))
    else {
        panic!("set PATORRENT_LIVE_TORZNAB and PATORRENT_LIVE_KEY_FILE");
    };
    let key = std::fs::read_to_string(file).unwrap().trim().to_owned();
    let q = std::env::var("PATORRENT_LIVE_QUERY").unwrap_or_else(|_| "test".into());
    let t = Harness::start("live");
    let sid = t.login();
    let body = format!(r#"{{"name":"Live","url":"{url}","key":"{key}","password":"{PASS}"}}"#);
    assert_eq!(t.pa(&sid, "/pa/search/source-add", &body).0, 200);
    let (_, test) = t.pa(&sid, "/pa/search/source-test", r#"{"index":0}"#);
    println!("test: {test}");
    let (st, _) = t.pa(&sid, "/pa/search/start", &format!(r#"{{"q":"{q}"}}"#));
    assert_eq!(st, 204);
    let m = t.model_until(&sid, "the answer", idle);
    println!("status: {}", s(arr(m.get("statuses"))[0].get("text")));
    let results = arr(m.get("results"));
    for r in results.iter().take(5) {
        println!(
            "result: {} | {} | seeds {} | {} | {}",
            s(r.get("name")),
            s(r.get("size_text")),
            int(r.get("seeders")),
            s(r.get("category_text")),
            s(r.get("age_text"))
        );
    }
    assert!(!results.is_empty(), "{}", m.to_json());
    let first = &results[0];
    let (st, body) = t.pa(&sid, "/pa/search/add", &format!(r#"{{"id":"{}"}}"#, s(first.get("id"))));
    println!("add: {st} {body}");
    assert_eq!(st, 200, "{body}");
    let deadline = Instant::now() + Duration::from_secs(20);
    while t.names(&sid).is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("torrents: {:?}", t.names(&sid));
    for b in t.bodies.lock().unwrap().iter() {
        assert!(!b.contains(&key), "the key showed in an answer");
    }
}
