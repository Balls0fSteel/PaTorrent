//! Loopback swarm tests for the real engine: a seeding session and a downloading session on
//! 127.0.0.1, connected directly, through an HTTP tracker and through a UDP tracker; plus a
//! restart that resumes without rechecking.

use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use libretorrent_core::bencode::Entry;
use libretorrent_core::crypto::Sha1;
use libretorrent_core::{
    AddTorrentParams, Alert, Session, SessionHandle, SessionParams, Settings, TorrentFlags, TorrentId,
    TorrentSource, TorrentState, TorrentStatus,
};

const PIECE: usize = 32 * 1024;

struct Fixture {
    torrent: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    info_hash: [u8; 20],
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lt-swarm-{name}-{}-{}", std::process::id(), rand_u32()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rand_u32() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    h.finish() as u32
}

fn fixture(announce: Option<&str>) -> Fixture {
    let sizes =
        [("alpha.bin", 300_000usize), ("sub/beta.bin", 1_000_000), ("sub/gamma.txt", 12_345), ("empty", 0)];
    let mut all = Vec::new();
    let mut files = Vec::new();
    for (i, (name, size)) in sizes.iter().enumerate() {
        let mut x = 0x9e3779b9u32.wrapping_mul(i as u32 + 1);
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
    let mut pieces = Vec::new();
    for c in all.chunks(PIECE) {
        pieces.extend_from_slice(&Sha1::digest(c).0);
    }
    let mut info = Entry::new_dict();
    let mut list = Entry::new_list();
    for (name, data) in &files {
        let mut f = Entry::new_dict();
        *f.key("length") = Entry::Int(data.len() as i64);
        *f.key("path") = Entry::List(name.split('/').map(|p| Entry::Bytes(p.as_bytes().to_vec())).collect());
        list.list_mut().push(f);
    }
    *info.key("files") = list;
    *info.key("name") = Entry::Bytes(b"swarm-test".to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let info_hash = Sha1::digest(&info.bencode()).0;
    let mut t = Entry::new_dict();
    if let Some(a) = announce {
        *t.key("announce") = Entry::Bytes(a.as_bytes().to_vec());
    }
    *t.key("info") = info;
    Fixture { torrent: t.bencode(), files, info_hash }
}

fn write_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let p = dir.join("swarm-test").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

fn assert_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let got = std::fs::read(dir.join("swarm-test").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(got == *data, "{name} differs");
    }
}

fn settings() -> Settings {
    Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        tick_interval: 100,
        aio_threads: 2,
        ..Settings::default()
    }
}

fn start(state_dir: Option<PathBuf>) -> Session {
    Session::start(SessionParams { settings: settings(), state_dir }).expect("session starts")
}

/// Pops alerts until `f` returns `Some`, failing after `secs`.
fn wait<T>(h: &SessionHandle, secs: u64, what: &str, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut alerts = Vec::new();
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        h.post_torrent_updates();
        std::thread::sleep(Duration::from_millis(30));
        h.pop_alerts(&mut alerts);
        for a in &alerts {
            if !matches!(a, Alert::StateUpdate(_)) {
                seen.push(a.message());
            }
            if let Some(v) = f(a) {
                return v;
            }
        }
    }
    panic!("timed out waiting for {what}; alerts: {seen:#?}");
}

fn listen_port(h: &SessionHandle) -> u16 {
    wait(h, 5, "listen", |a| match a {
        Alert::ListenSucceeded { address, .. } => Some(address.port()),
        _ => None,
    })
}

fn add(h: &SessionHandle, f: &Fixture, dir: &Path) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    h.add_torrent(p);
    wait(h, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}

fn status_of(a: &Alert, id: TorrentId) -> Option<TorrentStatus> {
    match a {
        Alert::StateUpdate(v) => v.iter().find(|s| s.id == id).cloned(),
        _ => None,
    }
}

fn wait_state(h: &SessionHandle, id: TorrentId, state: TorrentState, secs: u64) -> TorrentStatus {
    wait(h, secs, &format!("{state:?}"), |a| {
        status_of(a, id).filter(|s| s.state == state && s.error.is_empty())
    })
}

fn seeder(f: &Fixture) -> (Session, PathBuf, u16, TorrentId) {
    let dir = temp_dir("seed");
    write_files(f, &dir);
    let s = start(None);
    let port = listen_port(&s.handle());
    let id = add(&s.handle(), f, &dir);
    let st = wait_state(&s.handle(), id, TorrentState::Seeding, 20);
    assert_eq!(st.total_done, st.total);
    (s, dir, port, id)
}

#[test]
fn download_from_a_directly_connected_seed() {
    let f = fixture(None);
    let (seed, seed_dir, port, _) = seeder(&f);
    let dir = temp_dir("leech");
    let leech = start(None);
    let id = add(&leech.handle(), &f, &dir);
    wait_state(&leech.handle(), id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    wait(&leech.handle(), 60, "finished", |a| {
        matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(())
    });
    let st = wait_state(&leech.handle(), id, TorrentState::Seeding, 10);
    assert_eq!(st.total_wanted_done, st.total_wanted);
    assert_eq!(st.total_failed_bytes, 0);
    leech.shutdown();
    seed.shutdown();
    assert_files(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
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

/// A tiny HTTP tracker that hands every announcer the seed's address.
fn http_tracker(info_hash: [u8; 20], seed: SocketAddr) -> (String, std::thread::JoinHandle<u32>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/announce", l.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let mut announces = 0;
        l.set_nonblocking(false).unwrap();
        for conn in l.incoming().take(3) {
            let Ok(mut c) = conn else { continue };
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
            announces += 1;
            let SocketAddr::V4(a) = seed else { unreachable!() };
            let mut peers = a.ip().octets().to_vec();
            peers.extend_from_slice(&a.port().to_be_bytes());
            let mut body = b"d8:completei1e10:incompletei1e8:intervali1800e5:peers6:".to_vec();
            body.extend_from_slice(&peers);
            body.push(b'e');
            let head =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = c.write_all(head.as_bytes());
            let _ = c.write_all(&body);
            if announces >= 1 {
                break;
            }
        }
        announces
    });
    (url, h)
}

#[test]
fn download_through_an_http_tracker() {
    let plain = fixture(None);
    let (seed, seed_dir, port, _) = seeder(&plain);
    let (url, tracker) = http_tracker(plain.info_hash, SocketAddr::from(([127, 0, 0, 1], port)));
    let dir = temp_dir("leech-http");
    let leech = start(None);
    let mut p =
        AddTorrentParams::new(TorrentSource::TorrentFile(plain.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    p.trackers = vec![url];
    leech.handle().add_torrent(p);
    let id = wait(&leech.handle(), 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    wait(&leech.handle(), 10, "tracker reply", |a| match a {
        Alert::TrackerReply { num_peers, .. } => Some(*num_peers),
        Alert::TrackerError { message, .. } => panic!("tracker error: {message}"),
        _ => None,
    });
    wait(&leech.handle(), 60, "finished", |a| {
        matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(())
    });
    assert_eq!(tracker.join().unwrap(), 1);
    leech.shutdown();
    seed.shutdown();
    assert_files(&plain, &dir);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

/// A BEP 15 tracker answering connect and announce with the seed's address.
fn udp_tracker(seed: SocketAddr) -> (String, std::thread::JoinHandle<u32>) {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let url = format!("udp://{}/announce", s.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        let mut announces = 0;
        while announces == 0 {
            let Ok((n, from)) = s.recv_from(&mut buf) else { break };
            let action = u32::from_be_bytes(buf[8..12].try_into().unwrap());
            let txn = &buf[12..16];
            let mut out = Vec::new();
            if n >= 16 && action == 0 {
                assert_eq!(&buf[..8], &0x41727101980u64.to_be_bytes());
                out.extend_from_slice(&0u32.to_be_bytes());
                out.extend_from_slice(txn);
                out.extend_from_slice(&0x1234_5678_9abc_def0u64.to_be_bytes());
            } else if n >= 98 && action == 1 {
                assert_eq!(&buf[..8], &0x1234_5678_9abc_def0u64.to_be_bytes(), "uses the connection id");
                announces += 1;
                out.extend_from_slice(&1u32.to_be_bytes());
                out.extend_from_slice(txn);
                out.extend_from_slice(&1800u32.to_be_bytes());
                out.extend_from_slice(&1u32.to_be_bytes());
                out.extend_from_slice(&1u32.to_be_bytes());
                let SocketAddr::V4(a) = seed else { unreachable!() };
                out.extend_from_slice(&a.ip().octets());
                out.extend_from_slice(&a.port().to_be_bytes());
            } else {
                continue;
            }
            s.send_to(&out, from).unwrap();
        }
        announces
    });
    (url, h)
}

#[test]
fn download_through_a_udp_tracker_then_resume_without_recheck() {
    let plain = fixture(None);
    let (seed, seed_dir, port, _) = seeder(&plain);
    let (url, tracker) = udp_tracker(SocketAddr::from(([127, 0, 0, 1], port)));
    let dir = temp_dir("leech-udp");
    let state = temp_dir("state");
    {
        let leech = start(Some(state.clone()));
        let mut p =
            AddTorrentParams::new(TorrentSource::TorrentFile(plain.torrent.clone()), dir.to_string_lossy());
        p.flags = TorrentFlags::AUTO_MANAGED;
        p.trackers = vec![url];
        leech.handle().add_torrent(p);
        let id = wait(&leech.handle(), 5, "add", |a| match a {
            Alert::TorrentAdded { id, .. } => Some(*id),
            _ => None,
        });
        wait(&leech.handle(), 60, "finished", |a| {
            matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(())
        });
        assert_eq!(tracker.join().unwrap(), 1);
        leech.shutdown();
    }
    seed.shutdown();
    assert_files(&plain, &dir);

    // Restart from the store: the torrent comes back as a seed after resume verification,
    // without a full recheck.
    let leech = start(Some(state.clone()));
    let h = leech.handle();
    let id = wait(&h, 5, "restore", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    let mut rechecked = false;
    let st = wait(&h, 10, "seeding after restart", |a| {
        if let Alert::Log { message, .. } = a {
            rechecked |= message.contains("rechecked");
        }
        if let Alert::StateChanged { state: TorrentState::CheckingFiles, .. } = a {
            rechecked = true;
        }
        status_of(a, id).filter(|s| s.state == TorrentState::Seeding)
    });
    assert!(!rechecked, "resumed without a full recheck");
    assert_eq!(st.total_done, st.total);
    assert!(st.all_time_download >= st.total, "all-time totals persist");
    leech.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(state);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn magnets_and_garbage_are_rejected_cleanly() {
    let s = start(None);
    let h = s.handle();
    h.add_torrent(AddTorrentParams::new(TorrentSource::TorrentFile(b"garbage".to_vec()), "."));
    h.add_torrent(AddTorrentParams::new(
        TorrentSource::Magnet("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x".into()),
        ".",
    ));
    let mut failures = 0;
    wait(&h, 5, "two failures", |a| {
        if matches!(a, Alert::AddTorrentFailed { .. }) {
            failures += 1;
        }
        (failures == 2).then_some(())
    });
    s.shutdown();
}
