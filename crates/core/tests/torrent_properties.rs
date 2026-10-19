//! Loopback tests for the per-torrent settings a Properties dialog changes on a running torrent
//! (libtorrent `torrent_handle::set_max_uploads`, `set_max_connections`, `replace_trackers`,
//! `add_tracker`, `set_flags` with `disable_dht` / `disable_pex`): a seed with three slow
//! leeches unchokes one regular peer plus the optimistic one under `max_uploads = 1` and all
//! of them without a limit, and drops the extra peers when `max_connections` is lowered; a
//! leech whose tracker list is replaced announces to the new tracker, downloads from the peer
//! it hands out, and finds the list and its switches again after a restart.

use std::collections::VecDeque;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, AnnounceEntry, PeerFlags, PeerInfo, Session, SessionHandle, SessionParams,
    Settings, TorrentFlags, TorrentId, TorrentSource, TorrentState, TorrentStatus,
};

const PIECE: usize = 32 * 1024;

struct Fixture {
    torrent: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    info_hash: [u8; 20],
}

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d =
        std::env::temp_dir().join(format!("lt-props-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Files of their own (another seed than the other test files' fixtures), `tag` making the
/// info-hash differ between the tests here.
fn fixture(tag: &str) -> Fixture {
    let sizes = [("one.bin", 600_000usize), ("two/three.bin", 900_000)];
    let mut all = Vec::new();
    let mut files = Vec::new();
    for (i, (name, size)) in sizes.iter().enumerate() {
        let mut x = 0x6c07_8965u32.wrapping_mul(i as u32 + 3);
        let data: Vec<u8> = (0..*size)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        all.extend_from_slice(&data);
        files.push((name.to_string(), data));
    }
    let pieces: Vec<u8> = all.chunks(PIECE).flat_map(|c| Sha1::digest(c).0).collect();
    let mut info = Entry::new_dict();
    let mut list = Entry::new_list();
    for (name, data) in &files {
        let mut f = Entry::new_dict();
        *f.key("length") = Entry::Int(data.len() as i64);
        *f.key("path") = Entry::List(name.split('/').map(|p| Entry::Bytes(p.as_bytes().to_vec())).collect());
        list.list_mut().push(f);
    }
    *info.key("files") = list;
    *info.key("name") = Entry::Bytes(b"props-test".to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    *info.key("x-tag") = Entry::Bytes(tag.as_bytes().to_vec());
    let info_hash = Sha1::digest(&info.bencode()).0;
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    Fixture { torrent: t.bencode(), files, info_hash }
}

fn write_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let p = dir.join("props-test").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
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

fn start(settings: Settings, state_dir: Option<PathBuf>) -> (Session, Watch) {
    let s = Session::start(SessionParams { settings, state_dir }).expect("session starts");
    let w = Watch { h: s.handle(), backlog: VecDeque::new() };
    (s, w)
}

/// A session handle plus the alerts popped but not looked at yet (as in `tests/swarm.rs`).
struct Watch {
    h: SessionHandle,
    backlog: VecDeque<Alert>,
}

fn wait<T>(w: &mut Watch, secs: u64, what: &str, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut alerts = Vec::new();
    loop {
        while let Some(a) = w.backlog.pop_front() {
            if let Some(v) = f(&a) {
                return v;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        w.h.post_torrent_updates();
        std::thread::sleep(Duration::from_millis(30));
        w.h.pop_alerts(&mut alerts);
        w.backlog.extend(alerts.drain(..));
    }
}

fn listen_port(w: &mut Watch) -> u16 {
    wait(w, 5, "listen", |a| match a {
        Alert::ListenSucceeded { address, .. } => Some(address.port()),
        _ => None,
    })
}

fn add(w: &mut Watch, f: &Fixture, dir: &Path, download_limit: u32) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    p.download_limit = download_limit;
    w.h.add_torrent(p);
    wait(w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}

fn status(w: &mut Watch, id: TorrentId, what: &str, f: impl Fn(&TorrentStatus) -> bool) -> TorrentStatus {
    wait(w, 30, what, |a| match a {
        Alert::StateUpdate(v) => v.iter().find(|s| s.id == id && f(s)).cloned(),
        _ => None,
    })
}

/// Connected peers (past the handshake).
fn peers(w: &mut Watch, id: TorrentId) -> Vec<PeerInfo> {
    w.h.torrent(id).post_peer_info();
    let all = wait(w, 5, "peer info", |a| match a {
        Alert::PeerInfo { id: i, peers } if *i == id => Some(peers.clone()),
        _ => None,
    });
    all.into_iter().filter(|p| !p.flags.intersects(PeerFlags::CONNECTING | PeerFlags::HANDSHAKE)).collect()
}

fn unchoked(peers: &[PeerInfo]) -> usize {
    peers.iter().filter(|p| !p.flags.contains(PeerFlags::CHOKED)).count()
}

/// Polls the seed's peers until `f` holds, failing after `secs`.
fn until(w: &mut Watch, id: TorrentId, secs: u64, what: &str, f: impl Fn(&[PeerInfo]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let p = peers(w, id);
        if f(&p) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}; peers: {p:#?}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn upload_slots_and_connection_limits_apply_to_a_running_seed() {
    let f = fixture("slots");
    let seed_dir = temp_dir("seed");
    write_files(&f, &seed_dir);
    // a choker run every second, so the test does not wait 15 s for one
    let (seed, mut sw) = start(Settings { unchoke_interval: 1, ..settings() }, None);
    let port = listen_port(&mut sw);
    let seed_id = add(&mut sw, &f, &seed_dir, 0);
    status(&mut sw, seed_id, "seeding", |s| s.state == TorrentState::Seeding);

    // three leeches at 16 kB/s stay interested for the whole test
    let mut leeches = Vec::new();
    for i in 0..3 {
        let dir = temp_dir(&format!("leech{i}"));
        let (s, mut w) = start(settings(), None);
        let id = add(&mut w, &f, &dir, 16_000);
        status(&mut w, id, "downloading", |s| s.state == TorrentState::Downloading);
        s.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
        leeches.push((s, w, dir));
    }
    until(&mut sw, seed_id, 20, "three interested leeches", |p| {
        p.iter().filter(|p| p.flags.contains(PeerFlags::REMOTE_INTERESTED)).count() == 3
    });
    until(&mut sw, seed_id, 5, "all three unchoked", |p| unchoked(p) == 3);

    // one regular upload slot, plus the optimistic unchoke libtorrent exempts from it
    sw.h.torrent(seed_id).set_max_uploads(1);
    status(&mut sw, seed_id, "uploads_limit", |s| s.uploads_limit == Some(1));
    until(&mut sw, seed_id, 5, "extra peers choked", |p| unchoked(p) <= 2);
    for _ in 0..8 {
        let p = peers(&mut sw, seed_id);
        assert!((1..=2).contains(&unchoked(&p)), "one slot plus the optimistic one: {p:#?}");
        std::thread::sleep(Duration::from_millis(250));
    }

    // no limit again: everyone is unchoked at the next choker run
    sw.h.torrent(seed_id).set_max_uploads(0);
    until(&mut sw, seed_id, 5, "all three unchoked again", |p| unchoked(p) == 3);
    status(&mut sw, seed_id, "no uploads_limit", |s| s.uploads_limit.is_none());

    // a lower connection limit drops the extra peers, and they are not let back in
    sw.h.torrent(seed_id).set_max_connections(2);
    status(&mut sw, seed_id, "connections_limit", |s| s.connections_limit == Some(2));
    until(&mut sw, seed_id, 5, "two peers left", |p| p.len() == 2);
    for (s, w, _) in &mut leeches {
        // the dropped leech tries again at once
        let id = wait(w, 1, "the leech's torrent", |a| match a {
            Alert::StateUpdate(v) => v.first().map(|s| s.id),
            _ => None,
        });
        s.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    std::thread::sleep(Duration::from_secs(1));
    assert!(peers(&mut sw, seed_id).len() <= 2, "the limit holds against incoming connections");

    for (s, _, dir) in leeches {
        s.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }
    seed.shutdown();
    let _ = std::fs::remove_dir_all(seed_dir);
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// An HTTP tracker that answers one announce with the seed's address.
fn http_tracker(info_hash: [u8; 20], seed: SocketAddr) -> (String, std::thread::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/announce", l.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let Some(Ok(mut c)) = l.incoming().next() else { return };
        let mut buf = vec![0u8; 4096];
        let mut n = 0;
        while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
            match c.read(&mut buf[n..]) {
                Ok(0) | Err(_) => break,
                Ok(k) => n += k,
            }
        }
        let req = String::from_utf8_lossy(&buf[..n]).into_owned();
        let q = req.split("info_hash=").nth(1).and_then(|r| r.split('&').next()).unwrap_or_default();
        assert_eq!(percent_decode(q), info_hash, "announce carries the info-hash: {req}");
        let SocketAddr::V4(a) = seed else { unreachable!() };
        let mut body = b"d8:intervali1800e5:peers6:".to_vec();
        body.extend_from_slice(&a.ip().octets());
        body.extend_from_slice(&a.port().to_be_bytes());
        body.push(b'e');
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = c.write_all(head.as_bytes());
        let _ = c.write_all(&body);
    });
    (url, h)
}

fn trackers(w: &mut Watch, id: TorrentId) -> Vec<(String, u32)> {
    w.h.torrent(id).post_trackers();
    wait(w, 5, "tracker list", |a| match a {
        Alert::TrackerList { id: i, trackers } if *i == id => {
            Some(trackers.iter().map(|t| (t.url.clone(), t.tier)).collect())
        }
        _ => None,
    })
}

#[test]
fn replaced_trackers_are_announced_to_and_saved_with_the_switches() {
    let f = fixture("trackers");
    let seed_dir = temp_dir("tseed");
    write_files(&f, &seed_dir);
    let (seed, mut sw) = start(settings(), None);
    let port = listen_port(&mut sw);
    let seed_id = add(&mut sw, &f, &seed_dir, 0);
    status(&mut sw, seed_id, "seeding", |s| s.state == TorrentState::Seeding);

    let state = temp_dir("tstate");
    let dir = temp_dir("tleech");
    let (leech, mut w) = start(settings(), Some(state.clone()));
    let id = add(&mut w, &f, &dir, 0);
    assert!(trackers(&mut w, id).is_empty());
    let (url, tracker) = http_tracker(f.info_hash, SocketAddr::from(([127, 0, 0, 1], port)));
    // a dead tracker in the second tier, blanks and a repeat that are dropped
    let dead = "udp://127.0.0.1:9/announce".to_owned();
    w.h.torrent(id).replace_trackers(vec![
        AnnounceEntry::new(dead.clone(), 1),
        AnnounceEntry::new(url.clone(), 0),
        AnnounceEntry::new(" ", 0),
        AnnounceEntry::new(url.clone(), 2),
    ]);
    w.h.torrent(id).add_tracker(AnnounceEntry::new("http://127.0.0.1:9/b", 1));
    w.h.torrent(id).add_tracker(AnnounceEntry::new(dead.clone(), 0));
    let want = vec![(url.clone(), 0), (dead.clone(), 1), ("http://127.0.0.1:9/b".to_owned(), 1)];
    assert_eq!(trackers(&mut w, id), want);
    wait(&mut w, 10, "the new tracker's reply", |a| match a {
        Alert::TrackerReply { url: u, .. } if *u == url => Some(()),
        _ => None,
    });
    tracker.join().unwrap();
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));

    w.h.torrent(id).set_flags(TorrentFlags::DISABLE_DHT | TorrentFlags::DISABLE_PEX);
    w.h.torrent(id).set_max_uploads(3);
    status(&mut w, id, "the switches", |s| {
        s.flags.contains(TorrentFlags::DISABLE_DHT | TorrentFlags::DISABLE_PEX) && s.uploads_limit == Some(3)
    });
    leech.shutdown();

    let (leech, mut w) = start(settings(), Some(state.clone()));
    let id = wait(&mut w, 5, "the restored torrent", |a| match a {
        Alert::StateUpdate(v) => v.first().map(|s| s.id),
        _ => None,
    });
    assert_eq!(trackers(&mut w, id), want, "the list and its tiers are saved");
    let st = status(&mut w, id, "restored", |_| true);
    assert!(st.flags.contains(TorrentFlags::DISABLE_DHT | TorrentFlags::DISABLE_PEX), "{:?}", st.flags);
    assert!(!st.flags.contains(TorrentFlags::DISABLE_LSD));
    assert_eq!(st.uploads_limit, Some(3));
    w.h.torrent(id).replace_trackers(Vec::new());
    assert!(trackers(&mut w, id).is_empty());

    leech.shutdown();
    seed.shutdown();
    for d in [seed_dir, state, dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}
