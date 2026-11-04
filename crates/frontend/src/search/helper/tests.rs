//! The helper against a fake: one local HTTP server that answers as GitHub's release API and
//! download would, and as a running Jackett would (sign-in, the indexer listing, the configure
//! and remove calls, a Torznab search of `all`). No third-party code runs.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use patorrent_core::crypto::Sha256;

use super::archive::tests::{TempDir, gzip, tar, zip};
use super::*;
use crate::search::worker::{Query, fetch_results};

const PASSWORD: &str = "pa ss&word=1";
const KEY: &str = "abcdefghijklmnopqrstuvwxyz012345";
const RESULTS: &str = include_str!("../fixtures/prowlarr-results.xml");

struct Fake {
    addr: SocketAddr,
    /// Every request: `METHOD target` and its body.
    log: Arc<Mutex<Vec<(String, String)>>>,
}

impl Fake {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().iter().map(|(r, _)| r.clone()).collect()
    }
}

/// This platform's archive, as the release has it: the program and two definitions.
fn release_archive() -> (String, Vec<u8>) {
    let name = release::platform_asset().unwrap_or("Jackett.Binaries.Windows.zip").to_owned();
    let exe = if cfg!(windows) { "Jackett/JackettConsole.exe" } else { "Jackett/jackett" };
    let cf = b"settings:\n  - name: info_flaresolverr\n    type: info_flaresolverr\n";
    let plain = b"settings: []\n";
    let data = if name.ends_with(".zip") {
        zip(&[
            ("Jackett/", b"", false, 0o755),
            (exe, b"stand-in program", true, 0o755),
            ("Jackett/Definitions/cfsite.yml", cf, true, 0o644),
            ("Jackett/Definitions/opensite.yml", plain, true, 0o644),
        ])
    } else {
        gzip(&tar(&[
            ("Jackett/", b"", b'5', 0o755),
            (exe, b"stand-in program", b'0', 0o755),
            ("Jackett/Definitions/cfsite.yml", cf, b'0', 0o644),
            ("Jackett/Definitions/opensite.yml", plain, b'0', 0o644),
        ]))
    };
    (name, data)
}

/// A request line, its headers (names lowercased) and its body.
type Request = (String, Vec<(String, String)>, String);

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
    Some((line, headers, String::from_utf8_lossy(&body).into_owned()))
}

fn respond(s: &mut TcpStream, status: &str, extra: &str, body: &[u8], chunked: bool) {
    let head = if chunked {
        format!("HTTP/1.1 {status}\r\n{extra}Transfer-Encoding: chunked\r\n\r\n")
    } else {
        format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\n\r\n", body.len())
    };
    let _ = s.write_all(head.as_bytes());
    if chunked {
        for part in body.chunks(1000) {
            let _ = s.write_all(format!("{:x}\r\n", part.len()).as_bytes());
            let _ = s.write_all(part);
            let _ = s.write_all(b"\r\n");
        }
        let _ = s.write_all(b"0\r\n\r\n");
    } else {
        let _ = s.write_all(body);
    }
}

/// The fake GitHub and Jackett; `digest` is what the release lists for the archive.
fn fake(archive: (String, Vec<u8>), digest: String) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let (log2, (name, data)) = (log.clone(), archive);
    let configured = Arc::new(Mutex::new(vec!["oldsite".to_owned()]));
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let Some((line, headers, body)) = read_request(&mut s) else { continue };
            let target = line.split(' ').take(2).collect::<Vec<_>>().join(" ");
            log2.lock().unwrap().push((target.clone(), body.clone()));
            let signed_in =
                headers.iter().any(|(k, v)| k == "cookie" && v.split("; ").any(|c| c == "Jackett=session1"));
            let (method, path) = target.split_once(' ').unwrap();
            match (method, path) {
                ("GET", "/repos/Jackett/Jackett/releases/latest") => {
                    let json = format!(
                        r#"{{"tag_name":"v9.9.9","assets":[{{"name":"{name}","size":{},"browser_download_url":"http://{addr}/download/{name}","digest":"{digest}"}}]}}"#,
                        data.len()
                    );
                    respond(&mut s, "200 OK", "Content-Type: application/json\r\n", json.as_bytes(), false);
                }
                ("GET", p) if p == format!("/download/{name}") => respond(&mut s, "200 OK", "", &data, false),
                ("GET", "/health") => respond(&mut s, "200 OK", "", b"{\"status\":\"OK\"}", false),
                ("POST", "/UI/Dashboard") => {
                    // the form field, as Jackett's [FromForm] reads it
                    let good = body == "password=pa+ss%26word%3D1";
                    let cookie = if good { "Set-Cookie: Jackett=session1; path=/; httponly\r\n" } else { "" };
                    respond(&mut s, "302 Found", &format!("{cookie}Location: /UI/Dashboard\r\n"), b"", false);
                }
                (_, p) if p.starts_with("/api/v2.0/indexers/all/results/torznab") => {
                    if p.contains(&format!("apikey={KEY}")) {
                        respond(&mut s, "200 OK", "", RESULTS.as_bytes(), false);
                    } else {
                        let e = r#"<?xml version="1.0"?><error code="100" description="Invalid API Key" />"#;
                        respond(&mut s, "200 OK", "", e.as_bytes(), false);
                    }
                }
                (_, p) if p.starts_with("/api/") && !signed_in => {
                    respond(&mut s, "302 Found", "Location: /UI/Login\r\n", b"", false)
                }
                ("GET", "/api/v2.0/indexers?configured=false") => {
                    let on = configured.lock().unwrap().clone();
                    let item = |id: &str, name: &str, kind: &str| {
                        format!(
                            r#"{{"id":"{id}","name":"{name}","description":"","type":"{kind}","configured":{},"site_link":"https://{id}.example/","language":"en-US","last_error":"","potatoenabled":false,"caps":[]}}"#,
                            on.iter().any(|c| c == id)
                        )
                    };
                    let json = format!(
                        "[{},{},{},{}]",
                        item("opensite", "Open Site", "public"),
                        item("cfsite", "CF Site", "public"),
                        item("oldsite", "Old Site", "public"),
                        item("privsite", "Private Site", "private")
                    );
                    // Kestrel answers JSON chunked
                    respond(&mut s, "200 OK", "Content-Type: application/json\r\n", json.as_bytes(), true);
                }
                ("GET", p) if p.ends_with("/Config") => {
                    let json = r#"[{"id":"sitelink","type":"inputstring","name":"Site Link","value":"https://x.example/"},{"id":"tags","type":"inputtags","name":"Tags","value":""}]"#;
                    respond(&mut s, "200 OK", "", json.as_bytes(), true);
                }
                ("POST", p) if p.ends_with("/Config") => {
                    let id = p.trim_start_matches("/api/v2.0/indexers/").trim_end_matches("/Config");
                    configured.lock().unwrap().push(id.to_owned());
                    respond(&mut s, "204 No Content", "", b"", false);
                }
                ("DELETE", p) => {
                    let id = p.trim_start_matches("/api/v2.0/indexers/");
                    configured.lock().unwrap().retain(|c| c != id);
                    respond(&mut s, "200 OK", "", b"", false);
                }
                _ => respond(&mut s, "404 Not Found", "", b"", false),
            }
        }
    });
    Fake { addr, log }
}

fn good_fake() -> Fake {
    let archive = release_archive();
    let digest = format!("sha256:{}", Sha256::digest(&archive.1).to_hex());
    fake(archive, digest)
}

/// Jackett publishes no build for some systems (FreeBSD among them); there `latest_from`
/// rightly refuses, so the download and install tests have nothing to fetch.
fn no_jackett_build() -> bool {
    release::platform_asset().is_none()
}

#[test]
fn downloads_checks_and_installs() {
    if no_jackett_build() {
        return;
    }
    let f = good_fake();
    let r = release::latest_from(&f.url("/repos/Jackett/Jackett/releases/latest"), None).unwrap();
    assert_eq!(r.version, "v9.9.9");
    let mut seen = Vec::new();
    let data = release::download(&r, None, &mut |got, total| {
        seen.push((got, total));
        true
    })
    .unwrap();
    assert_eq!(seen.last().map(|s| s.0), Some(data.len() as u64));
    let t = TempDir::new("install");
    let layout = Layout::new(&t.0);
    let state = install_archive(&layout, &r.asset.name, &data, &r.version).unwrap();
    assert!(layout.installed());
    assert_eq!(state.version, "v9.9.9");
    assert_ne!(state.port, 0);
    assert!(layout.definitions().join("cfsite.yml").is_file());

    // the launch: our flags, its folder, and ServerConfig.json written for the port
    let proxy = HelperProxy {
        kind: 0,
        host: "proxy.example".into(),
        port: 3128,
        username: String::new(),
        password: String::new(),
    };
    let l = launch(&layout, 50999, Some(&proxy)).unwrap();
    assert_eq!(l.program, layout.exe());
    assert_eq!(l.cwd.as_deref(), layout.exe().parent());
    let args: Vec<String> = l.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args[..3], ["--NoUpdates", "--NoRestart", "--ListenPrivate"]);
    assert_eq!(args[3], "--DataFolder");
    assert_eq!(args[4], layout.data().to_string_lossy());
    assert_eq!(l.api_key, state.api_key);
    let cfg =
        json::parse(&std::fs::read_to_string(layout.data().join("ServerConfig.json")).unwrap()).unwrap();
    assert_eq!(cfg.get("Port").and_then(json::Json::as_u64), Some(50999));
    assert_eq!(cfg.str("ProxyUrl"), Some("proxy.example"));
    assert_eq!(State::load(&layout).unwrap().port, 50999, "the port is remembered");

    // an update keeps the key and password, replaces the program
    let again = install_archive(&layout, &r.asset.name, &data, "v10.0.0").unwrap();
    assert_eq!(
        (again.api_key, again.password, again.version.as_str()),
        (state.api_key, state.password, "v10.0.0")
    );
    assert!(!layout.root.join("app.new").exists());
}

#[test]
fn a_wrong_checksum_or_a_cancel_installs_nothing() {
    if no_jackett_build() {
        assert!(release::latest_from("http://127.0.0.1:1/", None).unwrap_err().contains("no build"));
        return;
    }
    let f = fake(release_archive(), format!("sha256:{}", "0".repeat(64)));
    let r = release::latest_from(&f.url("/repos/Jackett/Jackett/releases/latest"), None).unwrap();
    let e = release::download(&r, None, &mut |_, _| true).unwrap_err();
    assert!(e.contains("SHA-256"), "{e}");
    let f = good_fake();
    let r = release::latest_from(&f.url("/repos/Jackett/Jackett/releases/latest"), None).unwrap();
    let e = release::download(&r, None, &mut |_, _| false).unwrap_err();
    assert!(e.contains("cancelled"), "{e}");
    // a damaged archive leaves no program behind
    let t = TempDir::new("bad-archive");
    let layout = Layout::new(&t.0);
    assert!(install_archive(&layout, &r.asset.name, b"not an archive", "v1").is_err());
    assert!(!layout.installed());
}

#[test]
fn sites_are_listed_and_configured() {
    let f = good_fake();
    let port = f.addr.port();
    assert!(api::healthy(port, std::time::Duration::from_secs(2)));
    assert!(api::Admin::login(port, "wrong").is_err());
    let admin = api::Admin::login(port, PASSWORD).unwrap();
    let mut list = admin.indexers().unwrap();
    assert_eq!(list.len(), 4);
    list.retain(Indexer::is_public);
    // the release's definitions say which ones need FlareSolverr
    let (name, data) = release_archive();
    let t = TempDir::new("defs");
    let layout = Layout::new(&t.0);
    install_archive(&layout, &name, &data, "v1").unwrap();
    mark_flaresolverr(&mut list, Some(&layout.definitions()));
    let cf = list.iter().find(|i| i.id == "cfsite").unwrap();
    assert!(cf.needs_flaresolverr);
    assert!(!list.iter().find(|i| i.id == "opensite").unwrap().needs_flaresolverr);
    // nothing ticked but the open one (and the FlareSolverr one, which is never configured);
    // the old one, no longer ticked, is removed
    apply_sites_with(&admin, &list, &["opensite".to_owned(), "cfsite".to_owned()]).unwrap();
    let calls = f.calls();
    assert!(calls.contains(&"GET /api/v2.0/indexers/opensite/Config".to_owned()), "{calls:?}");
    assert!(calls.contains(&"POST /api/v2.0/indexers/opensite/Config".to_owned()), "{calls:?}");
    assert!(calls.contains(&"DELETE /api/v2.0/indexers/oldsite".to_owned()), "{calls:?}");
    assert!(!calls.iter().any(|c| c.contains("cfsite")), "{calls:?}");
    // the configure call posted the items back as JSON
    let posted = f
        .log
        .lock()
        .unwrap()
        .iter()
        .find(|(r, _)| r.starts_with("POST /api/v2.0/indexers/opensite"))
        .unwrap()
        .1
        .clone();
    assert!(json::parse(&posted).unwrap().as_array().len() == 2, "{posted}");
    let now = admin.indexers().unwrap();
    assert!(now.iter().find(|i| i.id == "opensite").unwrap().configured);
    assert!(!now.iter().find(|i| i.id == "oldsite").unwrap().configured);
    // signed out: refused, not mistaken for an empty list
    let other = api::Admin::login(port, PASSWORD).unwrap();
    drop(other);
}

#[test]
fn the_source_searches_through_the_helper() {
    let f = good_fake();
    let state =
        State { version: "v1".into(), port: f.addr.port(), api_key: KEY.into(), password: PASSWORD.into() };
    let src = helper_source(&state);
    assert_eq!(src.url, format!("http://127.0.0.1:{}/api/v2.0/indexers/all/results/torznab/", f.addr.port()));
    assert!(is_helper_source(&src, &state));
    let found = fetch_results(&src, &Query { text: "fawlty".into(), categories: vec![] }, None).unwrap();
    assert!(!found.is_empty());
    let bad = Source { api_key: "wrong".into(), ..src.clone() };
    assert!(!is_helper_source(&bad, &state));
    assert!(fetch_results(&bad, &Query::default(), None).is_err());

    // added once, put back when edited, taken out on removal
    let other = Source {
        name: "Prowlarr".into(),
        url: "http://192.168.1.2:9696/1/api".into(),
        api_key: "p".into(),
        enabled: true,
    };
    let list = with_helper_source(std::slice::from_ref(&other), &state);
    assert_eq!(list.len(), 2);
    let mut edited = list.clone();
    edited[1].enabled = false;
    edited[1].url = "http://127.0.0.1:1/api/v2.0/indexers/all/results/torznab/".into();
    let again = with_helper_source(&edited, &state);
    assert_eq!(again.len(), 2);
    assert!(again[1].enabled && again[1].url == src.url);
    assert_eq!(without_helper_source(&again, &state), vec![other]);
}
