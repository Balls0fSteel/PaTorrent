//! Loopback swarm tests for the real engine: a seeding session and a downloading session on
//! 127.0.0.1, connected directly, through an HTTP tracker and through a UDP tracker; plus a
//! restart that resumes without rechecking, and one that keeps partly downloaded pieces; magnet
//! links that fetch their metadata from the seed (BEP 9), a trackerless magnet that finds its
//! seed through a third session's DHT, and a leech that learns about another through the
//! seed's peer exchange (BEP 11).

use std::collections::VecDeque;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, PeerFlags, PeerSource, Session, SessionHandle, SessionParams, Settings,
    TorrentFlags, TorrentId, TorrentSource, TorrentState, TorrentStatus,
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
    fixture_padded(announce, 0)
}

/// `padding` bytes of an unknown key make the info dictionary that much bigger (unknown keys
/// are kept and hashed), so the metadata spans several 16 KiB ut_metadata blocks.
fn fixture_padded(announce: Option<&str>, padding: usize) -> Fixture {
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
    if padding > 0 {
        *info.key("x-padding") = Entry::Bytes((0..padding).map(|i| (i % 251) as u8).collect());
    }
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
        // loopback tests stay off the internet
        enable_dht: false,
        ..Settings::default()
    }
}

fn start(state_dir: Option<PathBuf>) -> Session {
    Session::start(SessionParams { settings: settings(), state_dir }).expect("session starts")
}

/// A session handle plus the alerts popped but not looked at yet. `pop_alerts` hands over
/// everything queued, so a wait that stops at one alert must keep the rest for the next wait:
/// on a slow test thread (a loaded CI runner) `TorrentFinished` can arrive in the same batch as
/// `TrackerReply`, and dropping it made the HTTP tracker test time out after a finished download.
struct Watch {
    h: SessionHandle,
    backlog: VecDeque<Alert>,
}

impl Watch {
    fn new(h: SessionHandle) -> Watch {
        Watch { h, backlog: VecDeque::new() }
    }
}

/// Takes alerts in order until `f` returns `Some`, failing after `secs`. The alerts behind the
/// matching one stay queued.
fn wait<T>(w: &mut Watch, secs: u64, what: &str, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut alerts = Vec::new();
    let mut seen = Vec::new();
    loop {
        while let Some(a) = w.backlog.pop_front() {
            if !matches!(a, Alert::StateUpdate(_)) {
                seen.push(a.message());
            }
            if let Some(v) = f(&a) {
                return v;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        w.h.post_torrent_updates();
        std::thread::sleep(Duration::from_millis(30));
        w.h.pop_alerts(&mut alerts);
        w.backlog.extend(alerts.drain(..));
    }
    panic!("timed out waiting for {what}; alerts: {seen:#?}");
}

fn listen_port(h: &mut Watch) -> u16 {
    wait(h, 5, "listen", |a| match a {
        Alert::ListenSucceeded { address, .. } => Some(address.port()),
        _ => None,
    })
}

fn add(h: &mut Watch, f: &Fixture, dir: &Path) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    h.h.add_torrent(p);
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

fn wait_state(h: &mut Watch, id: TorrentId, state: TorrentState, secs: u64) -> TorrentStatus {
    wait(h, secs, &format!("{state:?}"), |a| {
        status_of(a, id).filter(|s| s.state == state && s.error.is_empty())
    })
}

fn seeder(f: &Fixture) -> (Session, PathBuf, u16, TorrentId) {
    let dir = temp_dir("seed");
    write_files(f, &dir);
    let s = start(None);
    let mut w = Watch::new(s.handle());
    let port = listen_port(&mut w);
    let id = add(&mut w, f, &dir);
    let st = wait_state(&mut w, id, TorrentState::Seeding, 20);
    assert_eq!(st.total_done, st.total);
    (s, dir, port, id)
}

#[test]
fn download_from_a_directly_connected_seed() {
    let f = fixture(None);
    let (seed, seed_dir, port, _) = seeder(&f);
    let dir = temp_dir("leech");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
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
    let mut w = Watch::new(leech.handle());
    let mut p =
        AddTorrentParams::new(TorrentSource::TorrentFile(plain.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    p.trackers = vec![url];
    leech.handle().add_torrent(p);
    let id = wait(&mut w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    wait(&mut w, 10, "tracker reply", |a| match a {
        Alert::TrackerReply { num_peers, .. } => Some(*num_peers),
        Alert::TrackerError { message, .. } => panic!("tracker error: {message}"),
        _ => None,
    });
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
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
        let mut w = Watch::new(leech.handle());
        let mut p =
            AddTorrentParams::new(TorrentSource::TorrentFile(plain.torrent.clone()), dir.to_string_lossy());
        p.flags = TorrentFlags::AUTO_MANAGED;
        p.trackers = vec![url];
        leech.handle().add_torrent(p);
        let id = wait(&mut w, 5, "add", |a| match a {
            Alert::TorrentAdded { id, .. } => Some(*id),
            _ => None,
        });
        wait(&mut w, 60, "finished", |a| {
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
    let mut w = Watch::new(leech.handle());
    let id = wait(&mut w, 5, "restore", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    let mut rechecked = false;
    let st = wait(&mut w, 10, "seeding after restart", |a| {
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

/// A scripted peer that has everything but answers only some requests: every block of the first
/// piece asked for, then block 0 of the next `partial` pieces. Returns the bytes it sent once the
/// connection closes; `quota` turns true when it has sent all it will send. `requests` logs every
/// (piece, offset) asked for, for the failure message.
fn stingy_peer(
    f: &Fixture,
    partial: usize,
    quota: std::sync::Arc<std::sync::atomic::AtomicBool>,
    requests: std::sync::Arc<std::sync::Mutex<Vec<(u32, u32)>>>,
) -> (u16, std::thread::JoinHandle<u64>) {
    use std::io::{Read, Write};
    use std::sync::atomic::Ordering;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let all: Vec<u8> = f.files.iter().flat_map(|(_, d)| d.iter().copied()).collect();
    let info_hash = f.info_hash;
    let h = std::thread::spawn(move || {
        let (mut c, _) = l.accept().unwrap();
        c.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let mut hs = [0u8; 68];
        c.read_exact(&mut hs).unwrap();
        assert_eq!(&hs[28..48], &info_hash);
        let mut out = vec![19u8];
        out.extend_from_slice(b"BitTorrent protocol");
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&info_hash);
        out.extend_from_slice(b"-XX0001-stingypeer00");
        let num_pieces = all.len().div_ceil(PIECE);
        let mut bits = vec![0u8; num_pieces.div_ceil(8)];
        for i in 0..num_pieces {
            bits[i / 8] |= 0x80 >> (i % 8);
        }
        out.extend_from_slice(&(bits.len() as u32 + 1).to_be_bytes());
        out.push(5);
        out.extend_from_slice(&bits);
        out.extend_from_slice(&[0, 0, 0, 1, 1]); // unchoke
        c.write_all(&out).unwrap();

        let (mut full, mut partial_sent, mut sent) = (None, 0, 0u64);
        loop {
            let mut len = [0u8; 4];
            if c.read_exact(&mut len).is_err() {
                return sent;
            }
            let mut msg = vec![0u8; u32::from_be_bytes(len) as usize];
            if c.read_exact(&mut msg).is_err() {
                return sent;
            }
            if msg.first() != Some(&6) {
                continue;
            }
            let field = |i: usize| u32::from_be_bytes(msg[1 + 4 * i..5 + 4 * i].try_into().unwrap());
            let (piece, begin, length) = (field(0), field(1), field(2));
            requests.lock().unwrap().push((piece, begin));
            let full_piece = *full.get_or_insert(piece);
            let serve = if piece == full_piece {
                true
            } else if begin == 0 && partial_sent < partial {
                partial_sent += 1;
                true
            } else {
                false
            };
            if serve {
                let start = piece as usize * PIECE + begin as usize;
                let mut out = (9 + length).to_be_bytes().to_vec();
                out.push(7);
                out.extend_from_slice(&piece.to_be_bytes());
                out.extend_from_slice(&begin.to_be_bytes());
                out.extend_from_slice(&all[start..start + length as usize]);
                c.write_all(&out).unwrap();
                sent += length as u64;
            }
            let full_done = sent - partial_sent as u64 * 16384
                >= PIECE.min(all.len() - full_piece as usize * PIECE) as u64;
            if partial_sent == partial && full_done {
                quota.store(true, Ordering::SeqCst);
            }
        }
    });
    (port, h)
}

#[test]
fn partial_pieces_survive_a_restart() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = fixture(None);
    let dir = temp_dir("leech-partial");
    let state = temp_dir("state-partial");
    let quota = Arc::new(AtomicBool::new(false));
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    // Two partial pieces, not more: requests the peer never answers fill the leech's pipeline
    // (4 deep at first, 1 once snubbed), and with four partials a slow CI runner sat on them
    // past the deadline (CI run 38761101805).
    let (peer_port, peer) = stingy_peer(&f, 2, quota.clone(), requests.clone());
    let done_before = {
        let leech = start(Some(state.clone()));
        let mut w = Watch::new(leech.handle());
        let id = add(&mut w, &f, &dir);
        wait_state(&mut w, id, TorrentState::Downloading, 10);
        leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], peer_port)));
        let deadline = Instant::now() + Duration::from_secs(30);
        while !quota.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "the leech did not ask for enough blocks; it asked for (piece, offset): {:?}",
                requests.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // Wait until every block sent is on disk (counted in total_done), then stop.
        let st = wait(&mut w, 10, "blocks on disk", |a| {
            status_of(a, id).filter(|s| s.total_done > 0 && s.total_done == s.total_payload_download)
        });
        assert_eq!(st.num_pieces, 1, "one whole piece, the rest partial");
        leech.shutdown();
        st.total_done
    };
    let sent = peer.join().unwrap();
    assert_eq!(done_before, sent);

    // Restart: the partial blocks come back without a recheck and are not downloaded again.
    let (seed, seed_dir, seed_port, _) = seeder(&f);
    let leech = start(Some(state.clone()));
    let mut w = Watch::new(leech.handle());
    let id = wait(&mut w, 5, "restore", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    let st = wait_state(&mut w, id, TorrentState::Downloading, 10);
    assert_eq!(st.total_done, done_before, "restored blocks count as done");
    assert_eq!(st.num_pieces, 1);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], seed_port)));
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert_eq!(st.total_failed_bytes, 0, "restored blocks hashed correctly");
    assert_eq!(st.total_payload_download + st.total_redundant_bytes, st.total - done_before);
    leech.shutdown();
    seed.shutdown();
    assert_files(&f, &dir);
    for d in [dir, state, seed_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn add_magnet(w: &mut Watch, uri: String, dir: &Path) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::Magnet(uri), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    w.h.add_torrent(p);
    wait(w, 5, "add magnet", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}

#[test]
fn magnet_fetches_metadata_from_the_seed_then_downloads() {
    // About 40 KB of metadata: three ut_metadata blocks.
    let f = fixture_padded(None, 40_000);
    let (seed, seed_dir, port, _) = seeder(&f);
    let dir = temp_dir("leech-magnet");
    let state = temp_dir("state-magnet");
    {
        let leech = start(Some(state.clone()));
        let mut w = Watch::new(leech.handle());
        let uri = format!("magnet:?xt=urn:btih:{}&dn=from+the+link&x.pe=127.0.0.1:{port}", hex(&f.info_hash));
        let id = add_magnet(&mut w, uri, &dir);
        let st = wait(&mut w, 10, "metadata state", |a| status_of(a, id));
        assert_eq!(st.state, TorrentState::DownloadingMetadata);
        assert_eq!(st.name, "from the link");
        assert!(!st.has_metadata);
        wait(&mut w, 30, "metadata", |a| {
            matches!(a, Alert::MetadataReceived { id: i } if *i == id).then_some(())
        });
        wait(&mut w, 60, "finished", |a| {
            matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(())
        });
        let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
        assert_eq!(st.name, "swarm-test", "the real name replaces the link's display name");
        assert_eq!(st.total_done, st.total);
        assert_eq!(st.total_failed_bytes, 0);
        leech.shutdown();
    }
    assert_files(&f, &dir);

    // The fetched metadata was stored: after a restart the torrent is a seed at once, with the
    // seed gone.
    seed.shutdown();
    let leech = start(Some(state.clone()));
    let mut w = Watch::new(leech.handle());
    let id = wait(&mut w, 5, "restore", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert!(st.has_metadata);
    assert_eq!(st.name, "swarm-test");
    leech.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(state);
    let _ = std::fs::remove_dir_all(seed_dir);
}

/// A session whose DHT bootstraps from `router` (no router: a DHT of its own).
fn start_dht(router: Option<u16>) -> (Session, Watch, u16) {
    let settings = Settings {
        enable_dht: true,
        dht_bootstrap_nodes: router.map_or(String::new(), |p| format!("127.0.0.1:{p}")),
        // re-announce every 2 s, so a lookup that ran before the seed announced comes back soon
        dht_announce_interval: 2,
        ..settings()
    };
    let s = Session::start(SessionParams { settings, state_dir: None }).expect("session starts");
    let mut w = Watch::new(s.handle());
    let port = listen_port(&mut w);
    // the DHT's UDP socket shares the TCP listen port
    wait(&mut w, 5, "DHT socket", |a| match a {
        Alert::ListenSucceeded { address, protocol } if format!("{protocol:?}") == "Udp" => {
            assert_eq!(address.port(), port);
            Some(())
        }
        _ => None,
    });
    (s, w, port)
}

#[test]
fn trackerless_magnet_finds_its_seed_through_the_dht() {
    // a bootstrap node, a seed announcing to it, and a magnet with nothing but the info-hash
    let f = fixture(None);
    let (router, _rw, router_port) = start_dht(None);
    let seed_dir = temp_dir("dht-seed");
    write_files(&f, &seed_dir);
    let (seed, mut sw, _) = start_dht(Some(router_port));
    let sid = add(&mut sw, &f, &seed_dir);
    wait_state(&mut sw, sid, TorrentState::Seeding, 20);

    let dir = temp_dir("dht-leech");
    let (leech, mut w, _) = start_dht(Some(router_port));
    let id = add_magnet(&mut w, format!("magnet:?xt=urn:btih:{}", hex(&f.info_hash)), &dir);
    let n = wait(&mut w, 30, "DHT peers", |a| match a {
        Alert::DhtReply { id: i, num_peers } if *i == id => Some(*num_peers),
        _ => None,
    });
    assert!(n >= 1);
    wait(&mut w, 30, "metadata", |a| {
        matches!(a, Alert::MetadataReceived { id: i } if *i == id).then_some(())
    });
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
    leech.handle().post_session_stats();
    let st = wait(&mut w, 5, "stats", |a| match a {
        Alert::SessionStats(s) => Some(*s),
        _ => None,
    });
    assert!(st.dht_nodes >= 1, "the router answered, so it is in the routing table");
    leech.shutdown();
    assert_files(&f, &dir);
    seed.shutdown();
    router.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn magnet_without_peers_survives_a_restart_then_downloads() {
    let f = fixture(None);
    let dir = temp_dir("leech-magnet2");
    let state = temp_dir("state-magnet2");
    let uri = format!("magnet:?xt=urn:btih:{}", hex(&f.info_hash));
    {
        let leech = start(Some(state.clone()));
        let mut w = Watch::new(leech.handle());
        let id = add_magnet(&mut w, uri, &dir);
        let st = wait_state(&mut w, id, TorrentState::DownloadingMetadata, 10);
        assert_eq!(st.name, hex(&f.info_hash), "without dn= the info-hash is the name");
        leech.shutdown();
    }
    let (seed, seed_dir, port, _) = seeder(&f);
    let leech = start(Some(state.clone()));
    let mut w = Watch::new(leech.handle());
    let id = wait(&mut w, 5, "restore", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    wait_state(&mut w, id, TorrentState::DownloadingMetadata, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert_eq!(st.name, "swarm-test");
    leech.shutdown();
    seed.shutdown();
    assert_files(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(state);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn bad_magnets_and_garbage_are_rejected_cleanly() {
    let s = start(None);
    let mut w = Watch::new(s.handle());
    let h = s.handle();
    h.add_torrent(AddTorrentParams::new(TorrentSource::TorrentFile(b"garbage".to_vec()), "."));
    h.add_torrent(AddTorrentParams::new(
        TorrentSource::Magnet("magnet:?xt=urn:btih:not-a-hash&dn=x".into()),
        ".",
    ));
    let mut failures = 0;
    wait(&mut w, 5, "two failures", |a| {
        if matches!(a, Alert::AddTorrentFailed { .. }) {
            failures += 1;
        }
        (failures == 2).then_some(())
    });
    s.shutdown();
}

/// A full alert queue drops the `StateUpdate`; its torrents must show up in a later update
/// (they only appear in updates while marked changed). Seen first in the GUI with 10,000 saved
/// torrents: the restore alerts filled the queue and the list stayed empty.
#[test]
fn dropped_state_updates_are_reported_again() {
    const N: usize = 20;
    let settings = Settings { alert_queue_size: 2, ..settings() };
    let s = Session::start(SessionParams { settings, state_dir: None }).expect("session starts");
    let h = s.handle();
    let dir = temp_dir("dropped-updates");
    for i in 0..N {
        let mut info = Entry::new_dict();
        *info.key("length") = Entry::Int(1000);
        *info.key("name") = Entry::Bytes(format!("t{i}").into_bytes());
        *info.key("piece length") = Entry::Int(PIECE as i64);
        *info.key("pieces") = Entry::Bytes(Sha1::digest(&[0u8; 1000]).0.to_vec());
        let mut t = Entry::new_dict();
        *t.key("info") = info;
        let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(t.bencode()), dir.to_string_lossy());
        p.flags = TorrentFlags::PAUSED;
        h.add_torrent(p);
    }
    // Nobody pops alerts: the add alerts fill the queue, so this update is dropped.
    std::thread::sleep(Duration::from_millis(500));
    h.post_torrent_updates();
    std::thread::sleep(Duration::from_millis(500));
    let mut alerts = Vec::new();
    h.pop_alerts(&mut alerts);
    assert!(matches!(alerts.first(), Some(Alert::AlertsDropped { .. })), "the queue should have overflowed");

    let mut seen = std::collections::HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while seen.len() < N && Instant::now() < deadline {
        h.post_torrent_updates();
        std::thread::sleep(Duration::from_millis(100));
        h.pop_alerts(&mut alerts);
        for a in &alerts {
            if let Alert::StateUpdate(v) = a {
                seen.extend(v.iter().map(|st| st.id));
            }
        }
    }
    s.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(seen.len(), N, "torrents from the dropped update never came back");
}

#[test]
fn a_leech_finds_another_through_the_seeds_peer_exchange() {
    // A and B want nothing, so neither finishes a download and drops its peers mid-test. A
    // connects to the seed; B connects only to the seed and must learn A's listen address from
    // the seed's ut_pex message.
    let f = fixture(None);
    let (seed, seed_dir, seed_port, _) = seeder(&f);
    let seed_addr = SocketAddr::from(([127, 0, 0, 1], seed_port));

    let a_dir = temp_dir("pex-a");
    let a = start(None);
    let mut aw = Watch::new(a.handle());
    let a_port = listen_port(&mut aw);
    let a_id = add_wanting_nothing(&mut aw, &f, &a_dir);
    a.handle().torrent(a_id).connect_peer(seed_addr);

    let b_dir = temp_dir("pex-b");
    let b = start(None);
    let mut bw = Watch::new(b.handle());
    let b_id = add_wanting_nothing(&mut bw, &f, &b_dir);
    // Give the seed a moment to finish A's handshake, so B's first PEX list includes it.
    std::thread::sleep(Duration::from_millis(500));
    b.handle().torrent(b_id).connect_peer(seed_addr);

    let a_addr = SocketAddr::from(([127, 0, 0, 1], a_port));
    let deadline = Instant::now() + Duration::from_secs(30);
    let found = loop {
        b.handle().torrent(b_id).post_peer_info();
        let peers = wait(&mut bw, 5, "peer info", |al| match al {
            Alert::PeerInfo { id, peers } if *id == b_id => Some(peers.clone()),
            _ => None,
        });
        if let Some(p) = peers.iter().find(|p| p.ip == a_addr) {
            break p.clone();
        }
        assert!(Instant::now() < deadline, "B never connected to A; peers: {peers:#?}");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(found.source.contains(PeerSource::PEX), "learned through PEX: {:?}", found.source);
    b.shutdown();
    a.shutdown();
    seed.shutdown();
    for d in [a_dir, b_dir, seed_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn a_new_peer_gets_download_quota_while_another_saturates_the_limit() {
    // The leech downloads from seed A at a 20 kB/s limit, so A's socket always has more data
    // than a tick's quota. Seed B, connected later, must still get quota to read its handshake
    // (it used to sit in the handshake until it timed out, as the real-swarm DHT and PEX peers
    // did, because A was served first and took everything).
    let f = fixture(None);
    let (seed_a, a_dir, a_port, _) = seeder(&f);
    let (seed_b, b_dir, b_port, _) = seeder(&f);
    let dir = temp_dir("limited");
    let leech = Session::start(SessionParams {
        settings: Settings { download_rate_limit: 20_000, ..settings() },
        state_dir: None,
    })
    .expect("session starts");
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], a_port)));
    wait(&mut w, 10, "payload from A", |a| {
        status_of(a, id).filter(|s| s.total_payload_download > 64 * 1024).map(|_| ())
    });

    let b_addr = SocketAddr::from(([127, 0, 0, 1], b_port));
    leech.handle().torrent(id).connect_peer(b_addr);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        leech.handle().torrent(id).post_peer_info();
        let peers = wait(&mut w, 5, "peer info", |al| match al {
            Alert::PeerInfo { id: i, peers } if *i == id => Some(peers.clone()),
            _ => None,
        });
        let b = peers.iter().find(|p| p.ip == b_addr);
        if b.is_some_and(|p| {
            !p.flags.contains(PeerFlags::CONNECTING) && !p.flags.contains(PeerFlags::HANDSHAKE)
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "B never got past the handshake; peers: {peers:#?}");
        std::thread::sleep(Duration::from_millis(200));
    }
    leech.shutdown();
    seed_a.shutdown();
    seed_b.shutdown();
    for d in [dir, a_dir, b_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

fn add_wanting_nothing(w: &mut Watch, f: &Fixture, dir: &Path) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::DEFAULT_DONT_DOWNLOAD;
    w.h.add_torrent(p);
    wait(w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}
