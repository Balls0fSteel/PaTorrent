//! The session's external address on loopback: an HTTP tracker's `external ip`, a peer's
//! `yourip` (BEP 10) and a DHT node's `ip` (BEP 42) are votes that set it, reported with
//! `Alert::ExternalIp`.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, EncPolicy, Session, SessionHandle, SessionParams, Settings, TorrentFlags,
    TorrentId, TorrentSource,
};

static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Every alert the tests saw and what the fake tracker did, timed, for a timeout to print (all
/// tests of the file write here).
fn note(what: String) {
    let t = T0.get_or_init(Instant::now).elapsed().as_secs_f64();
    LOG.lock().unwrap().push(format!("{t:8.3} {what}"));
}

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d =
        std::env::temp_dir().join(format!("lt-extip-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-file torrent; returns the .torrent and its info-hash.
fn torrent(announce: Option<&str>) -> (Vec<u8>, [u8; 20]) {
    let data = vec![7u8; 64 * 1024];
    let mut info = Entry::new_dict();
    *info.key("length") = Entry::Int(data.len() as i64);
    *info.key("name") = Entry::Bytes(b"extip-test.bin".to_vec());
    *info.key("piece length") = Entry::Int(32 * 1024);
    let mut pieces = Vec::new();
    for c in data.chunks(32 * 1024) {
        pieces.extend_from_slice(&Sha1::digest(c).0);
    }
    *info.key("pieces") = Entry::Bytes(pieces);
    let info_hash = Sha1::digest(&info.bencode()).0;
    let mut t = Entry::new_dict();
    if let Some(a) = announce {
        *t.key("announce") = Entry::Bytes(a.as_bytes().to_vec());
    }
    *t.key("info") = info;
    (t.bencode(), info_hash)
}

fn start(f: impl FnOnce(&mut Settings)) -> Session {
    let mut settings = Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        tick_interval: 100,
        aio_threads: 2,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        alert_mask: u32::MAX,
        ..Settings::default()
    };
    f(&mut settings);
    Session::start(SessionParams { settings, state_dir: None }).expect("session starts")
}

thread_local! {
    /// Alerts popped but not yet looked at: a wait that ends mid-batch leaves the rest here for
    /// the next one (each test runs one session on its own thread). Without it, an
    /// `ExternalIp` popped together with `TorrentAdded` was lost on a slow macOS runner.
    static BACKLOG: std::cell::RefCell<std::collections::VecDeque<Alert>> = Default::default();
}

/// Takes alerts until `f` returns `Some`; after `secs`, the messages of those it took.
fn try_wait<T>(
    h: &SessionHandle,
    secs: u64,
    mut f: impl FnMut(&Alert) -> Option<T>,
) -> Result<T, Vec<String>> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut alerts = Vec::new();
    let mut seen = Vec::new();
    loop {
        while let Some(a) = BACKLOG.with_borrow_mut(|b| b.pop_front()) {
            if !matches!(a, Alert::StateUpdate(_)) {
                seen.push(a.message());
                note(a.message());
            }
            if let Some(v) = f(&a) {
                return Ok(v);
            }
        }
        if Instant::now() >= deadline {
            return Err(seen);
        }
        std::thread::sleep(Duration::from_millis(20));
        h.pop_alerts(&mut alerts);
        BACKLOG.with_borrow_mut(|b| b.extend(alerts.drain(..)));
    }
}

fn timed_out(what: &str, seen: &[String]) -> ! {
    let log = LOG.lock().unwrap().join("\n");
    panic!("timed out waiting for {what}; alerts: {seen:#?}\nall:\n{log}");
}

/// Takes alerts until `f` returns `Some`, failing after `secs`.
fn wait<T>(h: &SessionHandle, secs: u64, what: &str, f: impl FnMut(&Alert) -> Option<T>) -> T {
    try_wait(h, secs, f).unwrap_or_else(|seen| timed_out(what, &seen))
}

fn add(h: &SessionHandle, t: &[u8], dir: &std::path::Path) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(t.to_vec()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    h.add_torrent(p);
    wait(h, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}

fn external_ip(a: &Alert) -> Option<IpAddr> {
    match a {
        Alert::ExternalIp { ip } => Some(*ip),
        _ => None,
    }
}

#[test]
fn a_tracker_reply_sets_the_external_address() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/announce", l.local_addr().unwrap());
    note(format!("tracker listening on {url}"));
    let tracker = std::thread::spawn(move || {
        let (mut c, from) = l.accept().unwrap();
        note(format!("tracker accepted {from}"));
        let mut buf = vec![0u8; 4096];
        let mut n = 0;
        while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
            match c.read(&mut buf[n..]) {
                Ok(0) | Err(_) => break,
                Ok(k) => n += k,
            }
        }
        note(format!("tracker read {n} bytes: {:?}", String::from_utf8_lossy(&buf[..n.min(80)])));
        let mut body = b"d11:external ip4:".to_vec();
        body.extend_from_slice(&[203, 0, 113, 7]);
        body.extend_from_slice(b"8:intervali1800e5:peers0:e");
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        c.write_all(head.as_bytes()).unwrap();
        c.write_all(&body).unwrap();
        note("tracker answered".into());
    });
    let s = start(|_| {});
    let h = s.handle();
    let (t, _) = torrent(Some(&url));
    let dir = temp_dir("tracker");
    let id = add(&h, &t, &dir);
    let ip = try_wait(&h, 15, external_ip).unwrap_or_else(|seen| {
        // whether the engine still answers, and what it makes of its tracker
        h.torrent(id).post_trackers();
        h.post_session_stats();
        let _ = try_wait(&h, 2, |a| {
            if let Alert::TrackerList { trackers, .. } = a {
                note(format!("{trackers:?}"));
            }
            None::<()>
        });
        timed_out("external IP", &seen)
    });
    assert_eq!(ip, "203.0.113.7".parse::<IpAddr>().unwrap());
    tracker.join().unwrap();
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_peers_yourip_sets_the_external_address() {
    let s = start(|s| {
        s.out_enc_policy = EncPolicy::Disabled;
        s.in_enc_policy = EncPolicy::Disabled;
    });
    let h = s.handle();
    let port = wait(&h, 5, "listen", |a| match a {
        Alert::ListenSucceeded { address, .. } => Some(address.port()),
        _ => None,
    });
    let (t, info_hash) = torrent(None);
    let dir = temp_dir("peer");
    add(&h, &t, &dir);

    // a plaintext BitTorrent handshake with the extension bit, then our extension handshake
    let mut c = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).unwrap();
    let mut hs = b"\x13BitTorrent protocol".to_vec();
    hs.extend_from_slice(&[0, 0, 0, 0, 0, 0x10, 0, 0]);
    hs.extend_from_slice(&info_hash);
    hs.extend_from_slice(b"-XX0000-abcdefghijkl");
    c.write_all(&hs).unwrap();
    let mut theirs = [0u8; 68];
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    c.read_exact(&mut theirs).unwrap();
    assert_eq!(&theirs[28..48], &info_hash);
    let mut ext = b"d1:md11:ut_metadatai3ee1:v4:fake6:yourip4:".to_vec();
    ext.extend_from_slice(&[198, 51, 100, 9]);
    ext.push(b'e');
    let mut m = ((ext.len() + 2) as u32).to_be_bytes().to_vec();
    m.extend_from_slice(&[20, 0]);
    m.extend_from_slice(&ext);
    c.write_all(&m).unwrap();

    let ip = wait(&h, 10, "external IP", external_ip);
    assert_eq!(ip, "198.51.100.9".parse::<IpAddr>().unwrap());
    drop(c);
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_dht_nodes_ip_sets_the_external_address() {
    let s = start(|s| {
        s.enable_dht = true;
        s.dht_bootstrap_nodes = String::new();
    });
    let h = s.handle();
    let port = wait(&h, 5, "DHT socket", |a| match a {
        Alert::ListenSucceeded { address, protocol } if format!("{protocol:?}") == "Udp" => {
            Some(address.port())
        }
        _ => None,
    });
    // a ping that tells us our address (BEP 42 `ip`); the session answers it
    let u = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut q = b"d1:ad2:id20:aaaaaaaaaaaaaaaaaaaae2:ip6:".to_vec();
    q.extend_from_slice(&[192, 0, 2, 44, 0x1a, 0xe1]);
    q.extend_from_slice(b"1:q4:ping1:t2:aa1:y1:qe");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut alerts = Vec::new();
    let ip = 'found: loop {
        assert!(Instant::now() < deadline, "no ExternalIp alert after DHT pings");
        u.send_to(&q, SocketAddr::from(([127, 0, 0, 1], port))).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        h.pop_alerts(&mut alerts);
        for a in alerts.drain(..) {
            if let Some(ip) = external_ip(&a) {
                break 'found ip;
            }
        }
    };
    assert_eq!(ip, "192.0.2.44".parse::<IpAddr>().unwrap());
    s.shutdown();
}
