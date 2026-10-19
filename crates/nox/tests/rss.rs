//! The RSS Downloader in nox over loopback: a feed served by a small HTTP server of the test,
//! the native `/pa/rss...` endpoints (feeds, filters, items, preview, a manual download, the
//! History, `rss.dat`), the `rss` version in the WebSocket's tick (through the cache only),
//! and the qBittorrent shim's `rss/*` endpoints on the same model.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{Session, SessionParams, Settings};
use patorrent_nox::webui::json::{self, Json};
use patorrent_nox::webui::{Config, WebUi};

const PASS: &str = "rss-pass";

/// A one-file torrent named `name` and its info-hash.
fn torrent(name: &str) -> (Vec<u8>, String) {
    let data = vec![7u8; 1000];
    let mut info = Entry::new_dict();
    *info.key("length") = Entry::Int(data.len() as i64);
    *info.key("name") = Entry::Bytes(name.as_bytes().to_vec());
    *info.key("piece length") = Entry::Int(16384);
    *info.key("pieces") = Entry::Bytes(Sha1::digest(&data).0.to_vec());
    let hash = Sha1::digest(&info.bencode()).to_hex();
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    (t.bencode(), hash)
}

/// Serves `files` (path, body) over HTTP/1.0 until the test ends; the requests' `Cookie`
/// headers are kept.
fn serve(files: Vec<(String, Vec<u8>)>) -> (SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let cookies = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = cookies.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let head = String::from_utf8_lossy(&buf).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
            if let Some(c) = head.lines().find_map(|l| l.strip_prefix("Cookie: ")) {
                seen.lock().unwrap().push(c.to_owned());
            }
            let out = match files.iter().find(|(p, _)| *p == path) {
                Some((_, body)) => {
                    let mut r =
                        format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
                    r.extend_from_slice(body);
                    r
                }
                None => b"HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
            };
            let _ = s.write_all(&out);
        }
    });
    (addr, cookies)
}

struct Harness {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
    session: Option<Session>,
    dir: PathBuf,
}

impl Harness {
    fn start(name: &str) -> Harness {
        let dir = std::env::temp_dir().join(format!("nox-rss-{name}-{}", std::process::id()));
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
        Harness { addr, stop, pump: Some(pump), session: Some(session), dir }
    }

    fn req(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        ctype: &str,
        body: &str,
    ) -> (u16, String) {
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut r = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n", self.addr);
        for (n, v) in headers {
            r.push_str(&format!("{n}: {v}\r\n"));
        }
        if !ctype.is_empty() {
            r.push_str(&format!("Content-Type: {ctype}\r\n"));
        }
        r.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        s.write_all(r.as_bytes()).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let text = String::from_utf8_lossy(&out).into_owned();
        let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = text.split_once("\r\n\r\n").map(|x| x.1.to_owned()).unwrap_or_default();
        (status, body)
    }

    fn login(&self) -> String {
        // on a raw socket, to read the cookie
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
        self.req("GET", path, &[("Cookie", sid)], "", "")
    }

    fn get_json(&self, sid: &str, path: &str) -> Json {
        let (s, body) = self.get(sid, path);
        assert_eq!(s, 200, "{path}: {body}");
        json::parse(&body).unwrap_or_else(|e| panic!("{e}: {body}"))
    }

    fn pa(&self, sid: &str, path: &str, body: &str) -> (u16, String) {
        let origin = format!("http://{}", self.addr);
        self.req(
            "POST",
            path,
            &[("Cookie", sid), ("Origin", &origin), ("X-Requested-With", "PaTorrent")],
            "application/json",
            body,
        )
    }

    fn shim(&self, sid: &str, path: &str, form: &str) -> (u16, String) {
        self.req("POST", path, &[("Cookie", sid)], "application/x-www-form-urlencoded", form)
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

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn arr(j: Option<&Json>) -> Vec<Json> {
    match j {
        Some(Json::Arr(v)) => v.clone(),
        other => panic!("not an array: {other:?}"),
    }
}

fn str_of(j: &Json, k: &str) -> String {
    j.get(k).and_then(Json::as_str).unwrap_or_default().to_owned()
}

#[test]
fn native_endpoints_and_the_shim_on_one_downloader() {
    let (one, one_hash) = torrent("rss-show-s01e01");
    let (two, two_hash) = torrent("rss-other-thing");
    let feed = |addr: SocketAddr| {
        format!(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Test Feed</title>
<item><title>Show.S01E01.720p.HDTV</title><guid>e1</guid><enclosure url="http://{addr}/one.torrent" type="application/x-bittorrent"/></item>
<item><title>Other.Thing.2026</title><guid>o1</guid><link>http://{addr}/two.torrent</link></item>
<item><title>Show.S01E02.720p.HDTV</title><guid>e2</guid><link>http://{addr}/missing.torrent</link></item>
</channel></rss>"#
        )
    };
    // the feed names the server's own address, so serve it from a second server
    let (files_addr, cookies) = serve(vec![("/one.torrent".into(), one), ("/two.torrent".into(), two)]);
    let (feed_addr, _) = serve(vec![("/rss".into(), feed(files_addr).into_bytes())]);

    let t = Harness::start("native");
    let sid = t.login();
    assert_eq!(arr(t.get_json(&sid, "/pa/rss").get("feeds")).len(), 0);
    // CSRF: a command without the header is refused
    let (s, _) = t.req("POST", "/pa/rss/feed-add", &[("Cookie", &sid)], "application/json", "{}");
    assert_eq!(s, 403);

    let url = format!("http://{feed_addr}/rss:COOKIE:uid=9");
    let (s, body) = t.pa(&sid, "/pa/rss/feed-add", &format!(r#"{{"url":"{url}","alias":"Shows"}}"#));
    assert_eq!(s, 200, "{body}");
    let feed_id = json::parse(&body).unwrap().get("id").and_then(Json::as_i64).unwrap();
    wait_until("the feed fetched", || {
        let m = t.get_json(&sid, "/pa/rss");
        arr(m.get("feeds"))[0].get("items").and_then(Json::as_i64) == Some(3)
    });
    let m = t.get_json(&sid, "/pa/rss");
    let f = &arr(m.get("feeds"))[0];
    assert_eq!((str_of(f, "name"), str_of(f, "alias")), ("Shows".to_owned(), "Shows".to_owned()));
    assert!(str_of(f, "status").starts_with("Updated "), "{}", f.to_json());
    assert_eq!(arr(m.get("qualities")).len(), 20);
    assert_eq!(arr(m.get("intervals")).len(), 13);
    let items = t.get_json(&sid, &format!("/pa/rss/items?feed={feed_id}"));
    let rows = arr(items.get("rows"));
    assert_eq!(rows.len(), 3);
    assert_eq!(
        arr(rows[0].get("cells"))[..3],
        [Json::from("Show.S01E01.720p.HDTV"), "1x01".into(), "HDTV|720p".into()]
    );
    assert_eq!(arr(items.get("columns")).len(), 6);
    assert!(t.get_json(&sid, "/pa/rss/items").get("rows").is_some(), "every feed's");

    // a filter: the preview, then the dialog closes and the filters run
    let (s, body) = t.pa(&sid, "/pa/rss/filter-add", r#"{"name":"Show","pattern":"*show*","label":"tv"}"#);
    assert_eq!(s, 200, "{body}");
    let filter = json::parse(&body).unwrap().get("id").and_then(Json::as_i64).unwrap();
    let preview = t.get_json(&sid, &format!("/pa/rss/preview?filter={filter}"));
    assert_eq!(arr(Some(&preview)).len(), 2);
    let (s, body) =
        t.pa(&sid, "/pa/rss/filter-edit", &format!(r#"{{"id":{filter},"not":"*S01E02*","paused":true}}"#));
    assert_eq!(s, 204, "{body}");
    assert_eq!(arr(Some(&t.get_json(&sid, &format!("/pa/rss/preview?filter={filter}")))).len(), 1);
    assert_eq!(t.pa(&sid, "/pa/rss/filter-edit", r#"{"id":999}"#).0, 404);
    assert_eq!(t.pa(&sid, "/pa/rss/process", "{}").0, 204);
    let info = |hash: &str| t.get_json(&sid, &format!("/api/v2/torrents/info?hashes={hash}"));
    wait_until("the filter's torrent added", || !arr(Some(&info(&one_hash))).is_empty());
    let added = &arr(Some(&info(&one_hash)))[0];
    assert_eq!(str_of(added, "category"), "tv");
    assert!(cookies.lock().unwrap().iter().any(|c| c == "uid=9"), "the feed's cookies go with the download");
    let history = arr(t.get_json(&sid, "/pa/rss").get("history"));
    assert_eq!(arr(Some(&history[0]))[0], Json::from("Show.S01E01.720p.HDTV"));

    // a manual download (a double-click on an item) labels it with the feed's name
    let (s, body) = t.pa(&sid, "/pa/rss/download", &format!(r#"{{"feed":{feed_id},"item":"o1"}}"#));
    assert_eq!(s, 204, "{body}");
    wait_until("the item's torrent added", || !arr(Some(&info(&two_hash))).is_empty());
    assert_eq!(str_of(&arr(Some(&info(&two_hash)))[0], "category"), "Shows");
    assert_eq!(t.pa(&sid, "/pa/rss/download", &format!(r#"{{"feed":{feed_id},"item":"nope"}}"#)).0, 404);
    let rows = arr(t.get_json(&sid, "/pa/rss/items").get("rows"));
    assert!(rows.iter().filter(|r| r.get("downloaded") == Some(&Json::Bool(true))).count() >= 2);

    // rss.dat in the config directory
    wait_until("rss.dat", || t.dir.join("config").join("rss.dat").exists());

    // the shim: rules, items, matchingArticles on the same downloader
    let rules = t.get_json(&sid, "/api/v2/rss/rules");
    let rule = rules.get("Show").expect("the filter as a rule");
    assert_eq!(
        (str_of(rule, "mustContain"), str_of(rule, "mustNotContain")),
        ("*show*".to_owned(), "*S01E02*".to_owned())
    );
    assert_eq!(rule.get("addPaused"), Some(&Json::Bool(true)));
    let def = r#"{"enabled":true,"mustContain":"other","assignedCategory":"misc","ignoreDays":1,"affectedFeeds":[]}"#;
    let (s, _) = t.shim(&sid, "/api/v2/rss/setRule", &format!("ruleName=Others&ruleDef={}", enc(def)));
    assert_eq!(s, 200);
    let m = t.get_json(&sid, "/pa/rss");
    let others = arr(m.get("filters"))
        .into_iter()
        .find(|f| str_of(f, "name") == "Others")
        .expect("the rule as a filter");
    assert_eq!(
        (str_of(&others, "label"), others.get("min_interval")),
        ("misc".to_owned(), Some(&Json::Int(86_400)))
    );
    let matching = t.get_json(&sid, "/api/v2/rss/matchingArticles?ruleName=Others");
    assert_eq!(arr(matching.get("Shows")), [Json::from("Other.Thing.2026")]);
    let items = t.get_json(&sid, "/api/v2/rss/items?withData=true");
    let shows = items.get("Shows").expect("the feed by its name");
    assert!(str_of(shows, "url").starts_with(&format!("http://{feed_addr}/rss")));
    assert_eq!(arr(shows.get("articles")).len(), 3);
    assert_eq!(t.get(&sid, "/api/v2/rss/setRule").0, 405);
    assert_eq!(t.shim(&sid, "/api/v2/rss/renameRule", "ruleName=Others&newRuleName=Misc").0, 200);
    assert_eq!(t.shim(&sid, "/api/v2/rss/removeRule", "ruleName=Misc").0, 200);
    assert_eq!(t.shim(&sid, "/api/v2/rss/removeRule", "ruleName=Misc").0, 409);
    assert_eq!(t.shim(&sid, "/api/v2/rss/markAsRead", "itemPath=Shows&articleId=e2").0, 200);
    let articles =
        arr(t.get_json(&sid, "/api/v2/rss/items?withData=true").get("Shows").unwrap().get("articles"));
    assert!(articles.iter().all(|a| a.get("isRead") == Some(&Json::Bool(true))), "added or marked");
    assert_eq!(t.shim(&sid, "/api/v2/rss/addFolder", "path=x").0, 409);
    let other_feed = format!("http://{feed_addr}/other");
    assert_eq!(t.shim(&sid, "/api/v2/rss/addFeed", &format!("url={}&path=Second", enc(&other_feed))).0, 200);
    assert_eq!(t.shim(&sid, "/api/v2/rss/addFeed", &format!("url={}&path=Third", enc(&other_feed))).0, 409);
    assert_eq!(t.shim(&sid, "/api/v2/rss/moveItem", "itemPath=Second&destPath=Renamed").0, 200);
    assert!(t.get_json(&sid, "/api/v2/rss/items").get("Renamed").is_some());
    assert_eq!(t.shim(&sid, "/api/v2/rss/refreshItem", "itemPath=Renamed").0, 200);
    assert_eq!(t.shim(&sid, "/api/v2/rss/removeItem", "path=Renamed").0, 200);
    assert_eq!(t.shim(&sid, "/api/v2/rss/removeItem", "path=Renamed").0, 409);

    // the native feed and filter commands that are left
    assert_eq!(t.pa(&sid, "/pa/rss/feed-edit", &format!(r#"{{"id":{feed_id},"enabled":false}}"#)).0, 204);
    assert_eq!(arr(t.get_json(&sid, "/pa/rss").get("feeds"))[0].get("enabled"), Some(&Json::Bool(false)));
    assert_eq!(t.pa(&sid, "/pa/rss/history-clear", "{}").0, 204);
    assert!(arr(t.get_json(&sid, "/pa/rss").get("history")).is_empty());
    assert_eq!(t.pa(&sid, "/pa/rss/filter-remove", &format!(r#"{{"id":{filter}}}"#)).0, 204);
    assert_eq!(t.pa(&sid, "/pa/rss/feed-remove", &format!(r#"{{"id":{feed_id}}}"#)).0, 204);
    let m = t.get_json(&sid, "/pa/rss");
    assert!(arr(m.get("feeds")).is_empty() && arr(m.get("filters")).is_empty());
    assert_eq!(t.pa(&sid, "/pa/rss/nope", "{}").0, 404);
    assert_eq!(t.get(&sid, "/pa/rss/feed-add").0, 405);
}
