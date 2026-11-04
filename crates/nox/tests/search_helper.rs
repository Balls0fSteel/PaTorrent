//! The search helper (D51) through the Web UI's `/pa/search/helper...` endpoints, end to end
//! against fakes; no third-party code runs. A local server answers as GitHub's release listing
//! and download; the "release" it serves holds a stand-in for Jackett's program: this very test
//! binary (on Unix a two-line script that runs it), which, started with Jackett's arguments
//! (`--DataFolder`), serves the parts of Jackett's HTTP API the helper uses on the port and with
//! the key and password hash from the `ServerConfig.json` we wrote. So the Web UI downloads,
//! unpacks, starts, signs in to, configures, searches through and stops a real child process.
//!
//! `harness = false` (Cargo.toml): `main` is the fake Jackett when started as one, else it runs
//! the test; the helper is process-wide, so the one test runs alone in its own binary.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patorrent_core::crypto::Sha256;
use patorrent_core::{Session, SessionParams, Settings};
use patorrent_frontend::search::helper::{self, release, sha512};
use patorrent_nox::webui::json::{self, Json};
use patorrent_nox::webui::{Config, WebUi};

const PASS: &str = "helper-pass";
const RESULTS: &str = include_str!("../../frontend/src/search/fixtures/prowlarr-results.xml");
const TEST: &str = "the_web_ui_installs_runs_and_removes_the_helper";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--DataFolder") {
        fake_jackett(Path::new(&args[i + 1]));
        return;
    }
    // libtest's ways of asking: a listing, or a filter naming other tests
    if args.iter().any(|a| a == "--list") {
        println!("{TEST}: test");
        return;
    }
    let filters: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with('-')).collect();
    if !filters.is_empty() && !filters.iter().any(|f| TEST.contains(f.as_str())) {
        println!("running 0 tests");
        return;
    }
    println!("running 1 test");
    the_web_ui_installs_runs_and_removes_the_helper();
    println!("test {TEST} ... ok");
}

// ---- the fake Jackett (this binary, started by the helper's runner) ----------------------------

/// A request line, its headers (names lowercased) and its body.
type Request = (String, Vec<(String, String)>, Vec<u8>);

fn read_request(s: &mut TcpStream) -> Option<Request> {
    let mut raw = Vec::new();
    let mut b = [0u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).ok()? == 0 {
            return None;
        }
        raw.push(b[0]);
    }
    let head = String::from_utf8_lossy(&raw).into_owned();
    let mut lines = head.split("\r\n");
    let line = lines.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let len =
        headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).ok()?;
    Some((line, headers, body))
}

fn respond(s: &mut TcpStream, status: &str, extra: &str, body: &[u8]) {
    let head =
        format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
}

fn form_value(body: &str, name: &str) -> Option<String> {
    let v = body.split('&').find_map(|p| p.strip_prefix(&format!("{name}=")))?;
    let v = v.replace('+', " ");
    let mut out = Vec::new();
    let b = v.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(x) = u8::from_str_radix(&v[i + 1..i + 3], 16) {
                out.push(x);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Jackett as the helper meets it: `/health`, the sign-in with the password whose hash
/// `ServerConfig.json` holds, the indexer listing and the configure and remove calls behind
/// the cookie, a Torznab search of `all` with the key, and the shutdown call. Ends on that
/// call, or after two minutes whatever happens.
fn fake_jackett(data: &Path) {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(120));
        std::process::exit(3);
    });
    let text = std::fs::read_to_string(data.join("ServerConfig.json")).expect("ServerConfig.json");
    let cfg = json::parse(&text).expect("ServerConfig.json parses");
    let port = cfg.get("Port").and_then(Json::as_i64).expect("Port") as u16;
    let key = cfg.get("APIKey").and_then(Json::as_str).expect("APIKey").to_owned();
    let hash = cfg.get("AdminPassword").and_then(Json::as_str).expect("AdminPassword").to_lowercase();
    assert_eq!(cfg.get("AllowExternal").and_then(Json::as_bool), Some(false));
    let l = TcpListener::bind(("127.0.0.1", port)).expect("the port is free");
    let mut configured: Vec<String> = Vec::new();
    for s in l.incoming() {
        let Ok(mut s) = s else { continue };
        let Some((line, headers, body)) = read_request(&mut s) else { continue };
        let target = line.split(' ').nth(1).unwrap_or("").to_owned();
        let method = line.split(' ').next().unwrap_or("").to_owned();
        let signed_in =
            headers.iter().any(|(k, v)| k == "cookie" && v.split("; ").any(|c| c == "Jackett=session1"));
        let body = String::from_utf8_lossy(&body).into_owned();
        match (method.as_str(), target.as_str()) {
            ("GET", "/health") => respond(&mut s, "200 OK", "", b"{\"status\":\"OK\"}"),
            ("POST", "/UI/Dashboard") => {
                let pw = form_value(&body, "password").unwrap_or_default();
                let good = sha512::jackett_password_hash(&pw, &key).to_lowercase() == hash;
                let cookie = if good { "Set-Cookie: Jackett=session1; path=/; httponly\r\n" } else { "" };
                respond(&mut s, "302 Found", &format!("{cookie}Location: /UI/Dashboard\r\n"), b"");
            }
            (_, p) if p.starts_with("/api/v2.0/server/Shutdown") => {
                let ok = p.contains(&format!("apikey={key}"));
                respond(&mut s, if ok { "200 OK" } else { "401 Unauthorized" }, "", b"");
                if ok {
                    drop(s);
                    std::process::exit(0);
                }
            }
            (_, p) if p.starts_with("/api/v2.0/indexers/all/results/torznab") => {
                if p.contains(&format!("apikey={key}")) {
                    respond(&mut s, "200 OK", "Content-Type: application/rss+xml\r\n", RESULTS.as_bytes());
                } else {
                    let e = r#"<?xml version="1.0"?><error code="100" description="Invalid API Key" />"#;
                    respond(&mut s, "200 OK", "", e.as_bytes());
                }
            }
            (_, p) if p.starts_with("/api/") && !signed_in => {
                respond(&mut s, "302 Found", "Location: /UI/Login\r\n", b"")
            }
            ("GET", "/api/v2.0/indexers?configured=false") => {
                let item = |id: &str, name: &str, kind: &str| {
                    format!(
                        r#"{{"id":"{id}","name":"{name}","description":"","type":"{kind}","configured":{},"site_link":"https://{id}.example/","language":"en-US","last_error":"","potatoenabled":false,"caps":[]}}"#,
                        configured.iter().any(|c| c == id)
                    )
                };
                let list = format!(
                    "[{},{},{}]",
                    item("opensite", "Open Site", "public"),
                    item("cfsite", "CF Site", "public"),
                    item("privsite", "Private Site", "private")
                );
                respond(&mut s, "200 OK", "Content-Type: application/json\r\n", list.as_bytes());
            }
            ("GET", p) if p.ends_with("/Config") => {
                let items = r#"[{"id":"sitelink","type":"inputstring","name":"Site Link","value":"https://x.example/"}]"#;
                respond(&mut s, "200 OK", "Content-Type: application/json\r\n", items.as_bytes());
            }
            ("POST", p) if p.ends_with("/Config") => {
                let id = p.trim_start_matches("/api/v2.0/indexers/").trim_end_matches("/Config");
                configured.push(id.to_owned());
                respond(&mut s, "204 No Content", "", b"");
            }
            ("DELETE", p) => {
                let id = p.trim_start_matches("/api/v2.0/indexers/");
                configured.retain(|c| c != id);
                respond(&mut s, "200 OK", "", b"");
            }
            _ => respond(&mut s, "404 Not Found", "", b""),
        }
    }
}

// ---- the fake GitHub ---------------------------------------------------------------------------

/// A stored zip of `(name, contents, unix mode)`.
fn zip(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data, mode) in entries {
        let offset = out.len() as u32;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0]); // version, flags, stored
        out.extend_from_slice(&[0; 8]); // time, date, crc (not checked)
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&(3u16 << 8 | 20).to_le_bytes());
        central.extend_from_slice(&[20, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&[0; 8]);
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 8]);
        central.extend_from_slice(&(mode << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A tar.gz of `(name, contents, mode)`: ustar headers, the gzip member's deflate stream in
/// stored blocks (the reader checks no CRC).
fn tar_gz(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, data, mode) in entries {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(format!("{mode:07o}\0").as_bytes());
        h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        tar.extend_from_slice(&h);
        tar.extend_from_slice(data);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.extend_from_slice(&[0; 1024]);
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    let blocks: Vec<&[u8]> = tar.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        out.push(u8::from(i + 1 == blocks.len()));
        out.extend_from_slice(&(b.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        out.extend_from_slice(b);
    }
    out.extend_from_slice(&[0; 8]);
    out
}

/// This platform's release archive with the stand-in program and two definitions.
fn release_archive() -> (String, Vec<u8>) {
    let name = release::platform_asset().expect("an asset for this platform").to_owned();
    let me = std::env::current_exe().unwrap();
    let cf: &[u8] = b"settings:\n  - name: info_flaresolverr\n    type: info_flaresolverr\n";
    let plain: &[u8] = b"settings: []\n";
    let data = if name.ends_with(".zip") {
        let exe = std::fs::read(&me).unwrap();
        zip(&[
            ("Jackett/JackettConsole.exe", &exe, 0o755),
            ("Jackett/Definitions/cfsite.yml", cf, 0o644),
            ("Jackett/Definitions/opensite.yml", plain, 0o644),
        ])
    } else {
        let script = format!("#!/bin/sh\nexec '{}' \"$@\"\n", me.display());
        tar_gz(&[
            ("Jackett/jackett", script.as_bytes(), 0o755),
            ("Jackett/Definitions/cfsite.yml", cf, 0o644),
            ("Jackett/Definitions/opensite.yml", plain, 0o644),
        ])
    };
    (name, data)
}

/// What the fake GitHub serves: the newest tag, and whether the download trickles.
struct Github {
    addr: SocketAddr,
    tag: Arc<Mutex<(String, bool)>>,
}

fn fake_github(name: String, data: Vec<u8>) -> Github {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let digest = Sha256::digest(&data).to_hex();
    let tag = Arc::new(Mutex::new(("v9.9.9".to_owned(), false)));
    let (tag2, data) = (tag.clone(), Arc::new(data));
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let (tag, data, name, digest) = (tag2.clone(), data.clone(), name.clone(), digest.clone());
            std::thread::spawn(move || {
                let Some((line, _, _)) = read_request(&mut s) else { return };
                let path = line.split(' ').nth(1).unwrap_or("").to_owned();
                let (version, slow) = tag.lock().unwrap().clone();
                if path == "/latest" {
                    let json = format!(
                        r#"{{"tag_name":"{version}","assets":[{{"name":"{name}","size":{},"browser_download_url":"http://{addr}/download/{name}","digest":"sha256:{digest}"}}]}}"#,
                        data.len()
                    );
                    respond(&mut s, "200 OK", "Content-Type: application/json\r\n", json.as_bytes());
                } else if path == format!("/download/{name}") {
                    if !slow {
                        return respond(&mut s, "200 OK", "", &data);
                    }
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", data.len());
                    let _ = s.write_all(head.as_bytes());
                    for part in data.chunks(16 << 10) {
                        if s.write_all(part).is_err() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                } else {
                    respond(&mut s, "404 Not Found", "", b"");
                }
            });
        }
    });
    Github { addr, tag }
}

// ---- the Web UI ----------------------------------------------------------------------------

struct Web {
    addr: SocketAddr,
    _web: WebUi,
    _session: Session,
}

impl Web {
    fn req(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let mut r = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n", self.addr);
        for (n, v) in headers {
            r.push_str(&format!("{n}: {v}\r\n"));
        }
        if !body.is_empty() {
            r.push_str("Content-Type: application/json\r\n");
        }
        r.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        s.write_all(r.as_bytes()).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let text = String::from_utf8_lossy(&out).into_owned();
        let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, text.split_once("\r\n\r\n").map(|x| x.1.to_owned()).unwrap_or_default())
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

    fn get_json(&self, sid: &str, path: &str, extra: &[(&str, &str)]) -> Json {
        let mut h = vec![("Cookie", sid)];
        h.extend_from_slice(extra);
        let (st, body) = self.req("GET", path, &h, "");
        assert_eq!(st, 200, "{path}: {body}");
        json::parse(&body).unwrap_or_else(|e| panic!("{e}: {body}"))
    }

    fn helper(&self, sid: &str) -> Json {
        self.get_json(sid, "/pa/search/helper", &[])
    }

    fn pa(&self, sid: &str, path: &str, body: &str, extra: &[(&str, &str)]) -> (u16, String) {
        let origin = format!("http://{}", self.addr);
        let mut h = vec![("Cookie", sid), ("Origin", origin.as_str()), ("X-Requested-With", "PaTorrent")];
        h.extend_from_slice(extra);
        self.req("POST", path, &h, body)
    }

    /// Starts a step; its task id.
    fn start(&self, sid: &str, step: &str, body: &str) -> i64 {
        let (st, text) = self.pa(sid, &format!("/pa/search/helper/{step}"), body, &[]);
        assert_eq!(st, 200, "{step}: {text}");
        json::parse(&text).unwrap().get("id").and_then(Json::as_i64).unwrap()
    }

    /// `GET /pa/search/helper` until task `id` has ended; that task.
    fn wait(&self, sid: &str, id: i64, seen: &mut Vec<(i64, Option<i64>)>) -> Json {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let m = self.helper(sid);
            let t = m.get("task").cloned().unwrap_or(Json::Null);
            assert_eq!(t.get("id").and_then(Json::as_i64), Some(id), "{}", m.to_json());
            seen.push((
                t.get("got").and_then(Json::as_i64).unwrap_or(0),
                t.get("total").and_then(Json::as_i64),
            ));
            if t.get("running").and_then(Json::as_bool) == Some(false) {
                return t;
            }
            assert!(Instant::now() < deadline, "task {id} never ended: {}", m.to_json());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn sources(&self, sid: &str) -> Vec<Json> {
        match self.get_json(sid, "/pa/search", &[]).get("sources") {
            Some(Json::Arr(v)) => v.clone(),
            _ => Vec::new(),
        }
    }
}

fn s(j: &Json, key: &str) -> String {
    j.get(key).and_then(Json::as_str).unwrap_or("").to_owned()
}

fn b(j: &Json, key: &str) -> bool {
    j.get(key).and_then(Json::as_bool).unwrap_or(false)
}

fn start_web(dir: &Path) -> Web {
    let settings = Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        ..Settings::default()
    };
    let session =
        Session::start(SessionParams { settings: settings.clone(), state_dir: None, ..Default::default() })
            .unwrap();
    let mut c = Config::new("127.0.0.1:0".parse().unwrap(), settings, dir.join("dl").to_string_lossy());
    c.password = Some(PASS.into());
    c.state_dir = Some(dir.join("config"));
    c.helper_dir = Some(dir.join("state"));
    c.pbkdf2_iterations = 2;
    let web = WebUi::start(session.handle(), c).unwrap();
    Web { addr: web.local_addr(), _web: web, _session: session }
}

fn the_web_ui_installs_runs_and_removes_the_helper() {
    let dir: PathBuf = std::env::temp_dir().join(format!("nox-search-helper-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let web = start_web(&dir);
    let sid = web.login();

    let m = web.helper(&sid);
    assert!(b(&m, "local"), "a loopback request is local: {}", m.to_json());
    if !helper::available() {
        // the Flatpak, the Store and package-manager builds, FreeBSD: nothing offered
        assert!(!b(&m, "available"), "{}", m.to_json());
        assert_eq!(web.pa(&sid, "/pa/search/helper/check", "{}", &[]).0, 404);
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    assert!(b(&m, "available") && !b(&m, "installed"), "{}", m.to_json());
    assert!(s(&m, "folder").ends_with("search-helper"), "{}", m.to_json());

    let (name, data) = release_archive();
    let gh = fake_github(name, data);
    // SAFETY: set before any thread of ours reads the environment (the Web UI's do not)
    unsafe { std::env::set_var("PATORRENT_HELPER_RELEASES", format!("http://{}/latest", gh.addr)) };

    // Get: the check, then the install behind the password
    let mut seen = Vec::new();
    let id = web.start(&sid, "check", r#"{"update":false}"#);
    let t = web.wait(&sid, id, &mut seen);
    assert_eq!(s(&t, "error"), "", "{}", t.to_json());
    assert_eq!(s(&t, "version"), "v9.9.9");
    assert!(s(&t, "question").contains("GPL-2.0") && s(&t, "question").contains("v9.9.9"), "{}", t.to_json());
    assert!(b(&t, "first"));
    let (st, _) = web.pa(&sid, "/pa/search/helper/install", r#"{"version":"v1"}"#, &[]);
    assert_eq!(st, 409, "only the release the check found");
    let (st, text) = web.pa(&sid, "/pa/search/helper/install", r#"{"version":"v9.9.9"}"#, &[]);
    assert_eq!(st, 400, "the password is needed: {text}");
    let id = web.start(&sid, "install", &format!(r#"{{"version":"v9.9.9","password":"{PASS}"}}"#));
    seen.clear();
    let t = web.wait(&sid, id, &mut seen);
    assert_eq!(s(&t, "error"), "", "{}", t.to_json());
    assert!(s(&t, "text").contains("v9.9.9"), "{}", t.to_json());
    assert!(b(&t, "first"), "the page goes on to the sites");
    let size = seen.iter().filter_map(|x| x.1).max();
    assert!(size.is_some_and(|n| n > 0), "the size is known: {seen:?}");
    let m = web.helper(&sid);
    assert!(b(&m, "installed") && s(&m, "version") == "v9.9.9", "{}", m.to_json());
    assert!(dir.join("state").join("search-helper").join("helper.ini").is_file());
    let sources = web.sources(&sid);
    assert_eq!(sources.len(), 1, "the helper's source is added");
    assert!(s(&sources[0], "url_redacted").contains("127.0.0.1"), "{}", sources[0].to_json());
    assert!(b(&sources[0], "has_key"));

    // the sites: the helper started, the public ones listed, nothing ticked; FlareSolverr marked
    let id = web.start(&sid, "sites", "{}");
    let t = web.wait(&sid, id, &mut seen);
    assert_eq!(s(&t, "error"), "", "{}", t.to_json());
    let Some(Json::Arr(sites)) = t.get("sites") else { panic!("{}", t.to_json()) };
    let ids: Vec<String> = sites.iter().map(|x| s(x, "id")).collect();
    assert_eq!(ids, ["cfsite", "opensite"], "public ones, by name");
    assert!(sites.iter().all(|x| !b(x, "configured")), "nothing ticked for the user");
    assert!(b(&sites[0], "needs_flaresolverr") && !b(&sites[1], "needs_flaresolverr"));
    assert!(web.helper(&sid).get("running").and_then(Json::as_bool) == Some(true));
    // OK with both ticked: the FlareSolverr one is never configured
    let id = web.start(&sid, "apply", r#"{"ticked":["opensite","cfsite"]}"#);
    let t = web.wait(&sid, id, &mut seen);
    assert_eq!(s(&t, "error"), "", "{}", t.to_json());
    assert!(s(&t, "text").contains('1'), "one site ticked: {}", t.to_json());
    let id = web.start(&sid, "sites", "{}");
    let t = web.wait(&sid, id, &mut seen);
    let Some(Json::Arr(sites)) = t.get("sites") else { panic!("{}", t.to_json()) };
    let on: Vec<String> = sites.iter().filter(|x| b(x, "configured")).map(|x| s(x, "id")).collect();
    assert_eq!(on, ["opensite"], "a site ticked before stays ticked");

    // a search of the helper's source goes to the running helper with its key
    let (st, text) = web.pa(&sid, "/pa/search/start", r#"{"q":"fawlty"}"#, &[]);
    assert_eq!(st, 204, "{text}");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let m = web.get_json(&sid, "/pa/search", &[]);
        let total = m.get("total").and_then(Json::as_i64).unwrap_or(0);
        if total > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "no results: {}", m.to_json());
        std::thread::sleep(Duration::from_millis(100));
    }

    // its own page: the address and password for a browser here, refused through a proxy
    let id = web.start(&sid, "page", "{}");
    let t = web.wait(&sid, id, &mut seen);
    assert!(
        s(&t, "url").starts_with("http://127.0.0.1:") && s(&t, "url").ends_with("/UI/Dashboard"),
        "{}",
        t.to_json()
    );
    assert_eq!(s(&t, "password").len(), 24);
    let proxied = [("X-Forwarded-For", "192.0.2.7")];
    let remote = web.get_json(&sid, "/pa/search/helper", &proxied);
    assert!(!b(&remote, "local"), "{}", remote.to_json());
    let rt = remote.get("task").cloned().unwrap_or(Json::Null);
    assert!(
        rt.get("password").is_none() && rt.get("url").is_none(),
        "never to another computer: {}",
        rt.to_json()
    );
    let (st, _) = web.pa(&sid, "/pa/search/helper/page", "{}", &proxied);
    assert_eq!(st, 403);

    // Update: up to date, then a newer release whose slow download is cancelled
    let id = web.start(&sid, "check", r#"{"update":true}"#);
    let t = web.wait(&sid, id, &mut seen);
    assert!(b(&t, "up_to_date") && s(&t, "text").contains("v9.9.9"), "{}", t.to_json());
    *gh.tag.lock().unwrap() = ("v9.9.10".to_owned(), true);
    let id = web.start(&sid, "check", r#"{"update":true}"#);
    let t = web.wait(&sid, id, &mut seen);
    assert!(!b(&t, "up_to_date") && s(&t, "question").contains("v9.9.10"), "{}", t.to_json());
    assert!(!b(&t, "first"));
    let id = web.start(&sid, "install", &format!(r#"{{"version":"v9.9.10","password":"{PASS}"}}"#));
    let deadline = Instant::now() + Duration::from_secs(30);
    while web.helper(&sid).get("task").and_then(|t| t.get("got")).and_then(Json::as_i64).unwrap_or(0) == 0 {
        assert!(Instant::now() < deadline, "the download never began");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (st, _) = web.pa(&sid, "/pa/search/helper/cancel", &format!(r#"{{"id":{id}}}"#), &[]);
    assert_eq!(st, 204);
    let t = web.wait(&sid, id, &mut seen);
    assert!(b(&t, "cancelled") && !s(&t, "error").is_empty(), "{}", t.to_json());
    // the worker ends soon after: then the helper is as it was
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (st, _) = web.pa(&sid, "/pa/search/helper/sites", "{}", &[]);
        if st == 200 {
            break;
        }
        assert_eq!(st, 409, "busy until the cancelled download ends");
        assert!(Instant::now() < deadline, "the cancelled download never ended");
        std::thread::sleep(Duration::from_millis(100));
    }
    let m = web.helper(&sid);
    assert_eq!(s(&m, "version"), "v9.9.9", "the cancelled update changed nothing: {}", m.to_json());
    let id = m.get("task").and_then(|t| t.get("id")).and_then(Json::as_i64).unwrap();
    web.wait(&sid, id, &mut seen);

    // Remove: the helper stopped, its folder and source gone
    let (st, text) = web.pa(&sid, "/pa/search/helper/remove", "{}", &[]);
    assert_eq!(st, 200, "{text}");
    let m = web.helper(&sid);
    assert!(!b(&m, "installed") && !b(&m, "running"), "{}", m.to_json());
    assert!(!dir.join("state").join("search-helper").exists());
    assert!(web.sources(&sid).is_empty(), "its source is taken out");

    helper::shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
