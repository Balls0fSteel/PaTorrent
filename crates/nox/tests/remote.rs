//! Remote sessions against the Web UI over loopback (`GET /pa/remote`): a client session
//! driving the nox's engine (add, the status updates from the cache, a detail request, pause,
//! remove), a second client getting the whole list, settings taking the Preferences path, the
//! login and ban, and the same over HTTPS with a pinned certificate.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use patorrent_core::remote::{self, ConnectError, Target};
use patorrent_core::tls::Verify;
use patorrent_core::{
    AddTorrentParams, Alert, RemoveFlags, Session, SessionHandle, SessionParams, Settings, TorrentFlags,
    TorrentId, TorrentSource, TorrentStatus,
};
use patorrent_nox::webui::https::Certificate;
use patorrent_nox::webui::{Config, WebUi};

const PASS: &str = "s3cret-pass";
const TIMEOUT: Duration = Duration::from_secs(10);

struct Nox {
    web: Arc<WebUi>,
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
    _session: Session,
    dir: PathBuf,
}

impl Drop for Nox {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(p) = self.pump.take() {
            let _ = p.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn settings() -> Settings {
    Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        tick_interval: 100,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        ..Settings::default()
    }
}

/// A nox: the engine, the Web UI, and the main loop's alert pump.
fn nox(name: &str, https: bool) -> Nox {
    let dir = std::env::temp_dir().join(format!("patorrent-remote-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let session = Session::start(SessionParams { settings: settings(), state_dir: None }).unwrap();
    let h = session.handle();
    let mut c = Config::new("127.0.0.1:0".parse().unwrap(), settings(), dir.join("dl").to_string_lossy());
    c.password = Some(PASS.into());
    c.state_dir = Some(dir.join("state"));
    c.pbkdf2_iterations = 2;
    if https {
        c.https = Some(Certificate::Generated { dir: None, names: vec!["localhost".into()] });
    }
    let web = Arc::new(WebUi::start(h.clone(), c).unwrap());
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
    Nox { web, stop, pump: Some(pump), _session: session, dir }
}

fn target(addr: SocketAddr, tls: Option<Verify>, password: &str) -> Target {
    Target {
        host: "127.0.0.1".into(),
        port: addr.port(),
        tls,
        username: "admin".into(),
        password: password.into(),
        allow_plaintext: false,
    }
}

/// A remote client, and the latest status of every torrent it was sent, kept as a GUI's model
/// keeps it: an update that arrives while a test waits for something else is not lost.
struct Client {
    h: SessionHandle,
    seen: RefCell<HashMap<TorrentId, TorrentStatus>>,
}

impl std::ops::Deref for Client {
    type Target = SessionHandle;
    fn deref(&self) -> &SessionHandle {
        &self.h
    }
}

impl Client {
    fn new(h: SessionHandle) -> Client {
        Client { h, seen: RefCell::new(HashMap::new()) }
    }

    /// Pops the alerts until `f` finds what it wants, asking for status updates as it goes.
    fn wait_for<T>(&self, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut alerts = Vec::new();
        while Instant::now() < deadline {
            self.h.post_torrent_updates();
            std::thread::sleep(Duration::from_millis(30));
            self.h.pop_alerts(&mut alerts);
            for a in &alerts {
                if let Alert::StateUpdate(v) = a {
                    self.seen.borrow_mut().extend(v.iter().map(|s| (s.id, s.clone())));
                }
            }
            if let Some(t) = alerts.iter().find_map(&mut f) {
                return t;
            }
        }
        panic!("timed out");
    }

    /// The torrent's status once `f` holds for it.
    fn status_of(&self, id: TorrentId, f: impl Fn(&TorrentStatus) -> bool) -> TorrentStatus {
        let check = || self.seen.borrow().get(&id).filter(|s| f(s)).cloned();
        if let Some(s) = check() {
            return s;
        }
        self.wait_for(|_| check())
    }
}

fn magnet(byte: u8, name: &str) -> AddTorrentParams {
    let uri = format!("magnet:?xt=urn:btih:{}&dn={name}", format!("{byte:02x}").repeat(20));
    AddTorrentParams::new(TorrentSource::Magnet(uri), "")
}

/// `GET path` with the session cookie of a fresh login; the body.
fn shim_get(addr: SocketAddr, path: &str) -> String {
    let req = |r: String| {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(r.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    };
    let form = format!("username=admin&password={PASS}");
    let login = req(format!(
        "POST /api/v2/auth/login HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{form}",
        form.len()
    ));
    let sid = login.split("SID=").nth(1).unwrap().split(';').next().unwrap().to_owned();
    let r =
        req(format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nCookie: SID={sid}\r\n\r\n"));
    r.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned()
}

#[test]
fn a_remote_session_drives_the_nox() {
    let n = nox("drive", false);
    let addr = n.web.local_addr();
    let (client, hello) = remote::connect(&target(addr, None, PASS), TIMEOUT).unwrap();
    assert_eq!(hello.server_version, patorrent_core::VERSION);
    assert!(hello.save_path.ends_with("dl"), "{}", hello.save_path);
    assert_eq!(hello.settings.listen_interfaces, "127.0.0.1:0");
    let h = Client::new(client.handle());

    // an add goes to the engine; its alert comes back through the fan-out, the status from
    // the cache
    let mut p = magnet(0xa1, "first");
    p.save_path = hello.save_path.clone();
    h.add_torrent(p);
    let id = h.wait_for(|a| match a {
        Alert::TorrentAdded { id, name, .. } if name == "first" => Some(*id),
        _ => None,
    });
    let st = h.status_of(id, |_| true);
    assert_eq!(st.name, "first");
    // a detail request is answered by the engine
    h.torrent(id).post_trackers();
    h.wait_for(|a| matches!(a, Alert::TrackerList { id: x, .. } if *x == id).then_some(()));
    // pause: the next update shows it
    h.torrent(id).unset_flags(TorrentFlags::AUTO_MANAGED);
    h.torrent(id).pause();
    h.status_of(id, |s| s.flags.contains(TorrentFlags::PAUSED));
    // only what changed comes again: an idle update leaves the paused torrent out
    std::thread::sleep(Duration::from_millis(500));
    let mut alerts = Vec::new();
    h.pop_alerts(&mut alerts);
    h.post_torrent_updates();
    let quiet = h.wait_for(|a| match a {
        Alert::StateUpdate(v) => Some(v.len()),
        _ => None,
    });
    assert!(quiet <= 1, "{quiet} torrents in an idle update");

    // a second client gets the whole list at once
    let (other, _) = remote::connect(&target(addr, None, PASS), TIMEOUT).unwrap();
    let o = Client::new(other.handle());
    o.status_of(id, |_| true);

    // settings take the Preferences path: applied and visible to the Web UI
    let mut s = hello.settings.clone();
    s.active_downloads = 7;
    h.apply_settings(s);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let prefs = shim_get(addr, "/api/v2/app/preferences");
        if prefs.contains("\"max_active_downloads\":7") {
            break;
        }
        assert!(Instant::now() < deadline, "{prefs}");
        std::thread::sleep(Duration::from_millis(50));
    }

    // removal reaches both clients
    h.remove_torrent(id, RemoveFlags::empty());
    for c in [&h, &o] {
        c.wait_for(|a| matches!(a, Alert::TorrentRemoved { id: x, .. } if *x == id).then_some(()));
    }
    drop(other);
    client.shutdown();
}

#[test]
fn the_login_is_checked_and_failures_count() {
    let n = nox("login", false);
    let addr = n.web.local_addr();
    let e = remote::connect(&target(addr, None, "wrong"), TIMEOUT).err().unwrap();
    assert!(matches!(e, ConnectError::Refused { status: 401, .. }), "{e:?}");
    // the default limit is five failures
    for _ in 0..4 {
        let _ = remote::connect(&target(addr, None, "wrong"), TIMEOUT);
    }
    let e = remote::connect(&target(addr, None, PASS), TIMEOUT).err().unwrap();
    assert!(matches!(e, ConnectError::Refused { status: 403, .. }), "{e:?}");
}

#[test]
fn the_upgrade_is_checked() {
    let n = nox("upgrade", false);
    let addr = n.web.local_addr();
    let ask = |extra: &str| {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(TIMEOUT)).unwrap();
        let auth = patorrent_core::util::string::base64encode(format!("admin:{PASS}").as_bytes());
        write!(
            s,
            "GET {} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Basic {auth}\r\n{extra}\r\n",
            remote::PATH
        )
        .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out.split(' ').nth(1).unwrap_or_default().to_owned()
    };
    let up = format!("Connection: Upgrade\r\nUpgrade: {}\r\n", remote::UPGRADE);
    assert_eq!(ask(""), "400");
    assert_eq!(ask(&up), "426");
    assert_eq!(ask(&format!("{up}{}: 999\r\n", remote::VERSION_HEADER)), "426");
    // a browser page elsewhere cannot open one
    assert_eq!(
        ask(&format!(
            "{up}{}: {}\r\nOrigin: http://evil.example\r\n",
            remote::VERSION_HEADER,
            remote::PROTOCOL
        )),
        "403"
    );
}

#[test]
fn a_remote_session_over_https() {
    let n = nox("https", true);
    let addr = n.web.local_addr();
    let pin = remote::probe_certificate("127.0.0.1", addr.port(), TIMEOUT).unwrap();
    assert_eq!(pin, n.web.https().unwrap().fingerprint);
    // not trusted without the pin
    let e = remote::connect(&target(addr, Some(Verify::System), PASS), TIMEOUT).err().unwrap();
    assert!(matches!(e, ConnectError::Io(_)), "{e:?}");
    let (client, _) = remote::connect(&target(addr, Some(Verify::Pinned(pin)), PASS), TIMEOUT).unwrap();
    let h = Client::new(client.handle());
    h.add_torrent(magnet(0xb2, "over-tls"));
    let id = h.wait_for(|a| match a {
        Alert::TorrentAdded { id, name, .. } if name == "over-tls" => Some(*id),
        _ => None,
    });
    h.status_of(id, |_| true);
}
