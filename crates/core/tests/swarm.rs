//! Loopback swarm tests for the real engine: a seeding session and a downloading session on
//! 127.0.0.1, connected directly, through an HTTP tracker and through a UDP tracker; plus a
//! restart that resumes without rechecking, and one that keeps partly downloaded pieces; magnet
//! links that fetch their metadata from the seed (BEP 9), a trackerless magnet that finds its
//! seed through a third session's DHT, a leech that learns about another through the seed's
//! peer exchange (BEP 11), and two sessions that find each other through local service
//! discovery (BEP 14); the MSE/PE policy matrix: forced, enabled and disabled encryption on
//! either side; uTP (BEP 29): the default preference for it, uTP-only sessions, the fall
//! back to TCP when the other end has no uTP, and encryption over uTP; a peer taking back a
//! piece with `lt_donthave` (BEP 54); a torrent made by `create_torrent` (with pad files); a
//! peer banned by smart-ban for a bad block while the seed that sent the rest of the piece is
//! kept; and the IP and port filters: blocked peers are never connected, refused when they
//! connect, and dropped when a new filter blocks them, and blocked trackers are not announced
//! to; and a skipped file kept in the part file across a restart and a recheck, then unskipped.

use std::collections::VecDeque;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, BlockReason, EncLevel, EncPolicy, IpFilter, PeerFlags, PeerSource, PortFilter,
    Session, SessionHandle, SessionParams, Settings, TorrentFlags, TorrentId, TorrentSource, TorrentState,
    TorrentStatus,
};

const PIECE: usize = 32 * 1024;

struct Fixture {
    torrent: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    info_hash: [u8; 20],
    piece_len: usize,
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
    fixture_with(announce, padding, PIECE)
}

fn fixture_with(announce: Option<&str>, padding: usize, piece_len: usize) -> Fixture {
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
    for c in all.chunks(piece_len) {
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
    *info.key("piece length") = Entry::Int(piece_len as i64);
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
    Fixture { torrent: t.bencode(), files, info_hash, piece_len }
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
        // loopback tests stay off the internet, and off the LAN: with LSD, sessions of tests
        // running in parallel on one fixture would find each other
        enable_dht: false,
        enable_lsd: false,
        // and off the router: loopback sessions have no ports to forward
        enable_upnp: false,
        enable_natpmp: false,
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

/// The status of the torrent a restarted session restored (these tests restore one). Restored
/// torrents post no `TorrentAdded`; the first status update names them, and it is put back for
/// the waits that follow (a torrent whose state does not change is in no later update).
fn first_restored(w: &mut Watch) -> TorrentStatus {
    let (st, update) = wait(w, 5, "restore", |a| match a {
        Alert::StateUpdate(v) => v.first().map(|s| (s.clone(), a.clone())),
        _ => None,
    });
    w.backlog.push_front(update);
    st
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

#[test]
fn upload_mode_downloads_nothing_until_it_is_switched_off() {
    // The scheduler's "Seeding only": a connected leech in upload mode stays not interested.
    let f = fixture(None);
    let (seed, seed_dir, port, _) = seeder(&f);
    let dir = temp_dir("leech");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let id = add_with_flags(&mut w, &f, &dir, TorrentFlags::UPLOAD_MODE);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    wait(&mut w, 20, "a connected seed", |a| status_of(a, id).filter(|s| s.num_seeds > 0));
    std::thread::sleep(Duration::from_secs(2));
    w.backlog.clear();
    let st = wait(&mut w, 5, "status", |a| status_of(a, id));
    assert_eq!(st.total_done, 0);
    assert!(st.flags.contains(TorrentFlags::UPLOAD_MODE));
    leech.handle().torrent(id).unset_flags(TorrentFlags::UPLOAD_MODE);
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
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
    // without a full recheck. The session says how many came back, once.
    let leech = start(Some(state.clone()));
    let mut w = Watch::new(leech.handle());
    let count = wait(&mut w, 5, "the restore's count", |a| match a {
        Alert::TorrentsRestored { count, .. } => Some(*count),
        Alert::TorrentAdded { .. } => panic!("a restored torrent posted TorrentAdded"),
        _ => None,
    });
    assert_eq!(count, 1);
    let id = first_restored(&mut w).id;
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

        let num_pieces = all.len().div_ceil(PIECE);
        let (mut full, mut partial_sent, mut partial_bytes, mut sent) = (None, 0, 0u64, 0u64);
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
            // A partial piece gets its first block only. The last piece of the fixture is a
            // single short block, so serving it would complete it: it is never a partial (CI
            // run 38784078071 served it, and the 16 KiB-per-partial arithmetic never added up).
            let is_full = piece == full_piece;
            let is_partial = !is_full
                && begin == 0
                && partial_sent < partial
                && piece as usize + 1 < num_pieces
                && length as usize == 16384;
            if is_full || is_partial {
                let start = piece as usize * PIECE + begin as usize;
                let mut out = (9 + length).to_be_bytes().to_vec();
                out.push(7);
                out.extend_from_slice(&piece.to_be_bytes());
                out.extend_from_slice(&begin.to_be_bytes());
                out.extend_from_slice(&all[start..start + length as usize]);
                c.write_all(&out).unwrap();
                sent += length as u64;
                if is_partial {
                    partial_sent += 1;
                    partial_bytes += length as u64;
                }
            }
            let full_done = sent - partial_bytes >= PIECE.min(all.len() - full_piece as usize * PIECE) as u64;
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
    let id = first_restored(&mut w).id;
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
    let id = first_restored(&mut w).id;
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert!(st.has_metadata);
    assert_eq!(st.name, "swarm-test");
    leech.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(state);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn a_magnet_in_upload_mode_hands_out_its_torrent_file_and_waits() {
    // The Add New Torrent dialog for a magnet: metadata fetched without writing anything, the
    // `.torrent` handed to the GUI, the download started once the user has chosen.
    let f = fixture(None);
    let (seed, seed_dir, port, _) = seeder(&f);
    let dir = temp_dir("leech-magnet-wait");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let uri = format!("magnet:?xt=urn:btih:{}&tr=http%3A%2F%2F127.0.0.1%3A9%2Fannounce", hex(&f.info_hash));
    let mut p = AddTorrentParams::new(TorrentSource::Magnet(uri), dir.to_string_lossy());
    p.flags = TorrentFlags::UPLOAD_MODE;
    leech.handle().add_torrent(p);
    let id = wait(&mut w, 5, "add magnet", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    let h = leech.handle().torrent(id);
    h.post_torrent_file();
    let none = wait(&mut w, 5, "no file yet", |a| match a {
        Alert::TorrentFile { id: i, data } if *i == id => Some(data.clone()),
        _ => None,
    });
    assert!(none.is_none(), "no metadata, no .torrent");
    h.connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    wait(&mut w, 30, "metadata", |a| {
        matches!(a, Alert::MetadataReceived { id: i } if *i == id).then_some(())
    });
    h.post_torrent_file();
    let data = wait(&mut w, 5, "torrent file", |a| match a {
        Alert::TorrentFile { id: i, data } if *i == id => data.clone(),
        _ => None,
    });
    let ti = patorrent_core::metainfo::TorrentInfo::from_bytes(&data).unwrap();
    assert_eq!(ti.info_hash().as_bytes(), &f.info_hash);
    assert_eq!(ti.trackers().len(), 1, "the magnet's tracker is in the file");
    assert_eq!(ti.trackers()[0].url, "http://127.0.0.1:9/announce");
    std::thread::sleep(Duration::from_secs(1));
    w.backlog.clear();
    let st = wait(&mut w, 5, "status", |a| status_of(a, id));
    assert_eq!(st.total_done, 0, "upload mode wrote nothing");
    h.unset_flags(TorrentFlags::UPLOAD_MODE);
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
    leech.shutdown();
    seed.shutdown();
    assert_files(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn stop_when_ready_stops_a_torrent_once_it_is_checked() {
    let f = fixture(None);
    let (seed, seed_dir, port, _) = seeder(&f);
    let dir = temp_dir("leech-swr");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    // a .torrent: queued, checked, then stopped and out of the queue
    let id = add_with_flags(&mut w, &f, &dir, TorrentFlags::AUTO_MANAGED | TorrentFlags::STOP_WHEN_READY);
    wait(&mut w, 10, "paused", |a| matches!(a, Alert::TorrentPaused { id: i } if *i == id).then_some(()));
    let st = wait(&mut w, 5, "status", |a| status_of(a, id));
    assert!(st.flags.contains(TorrentFlags::PAUSED));
    assert!(
        !st.flags.intersects(TorrentFlags::AUTO_MANAGED | TorrentFlags::STOP_WHEN_READY),
        "{:?}",
        st.flags
    );
    assert_eq!(st.state, TorrentState::Downloading);
    leech.handle().remove_torrent(id, patorrent_core::RemoveFlags::empty());
    wait(&mut w, 5, "removed", |a| {
        matches!(a, Alert::TorrentRemoved { id: i, .. } if *i == id).then_some(())
    });

    // a magnet: its metadata first, then the check, then the stop; nothing downloaded
    let uri = format!("magnet:?xt=urn:btih:{}&x.pe=127.0.0.1:{port}", hex(&f.info_hash));
    let mut p = AddTorrentParams::new(TorrentSource::Magnet(uri), dir.to_string_lossy());
    p.flags = TorrentFlags::STOP_WHEN_READY;
    leech.handle().add_torrent(p);
    let id = wait(&mut w, 5, "add magnet", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    wait(&mut w, 30, "metadata", |a| {
        matches!(a, Alert::MetadataReceived { id: i } if *i == id).then_some(())
    });
    wait(&mut w, 10, "paused", |a| matches!(a, Alert::TorrentPaused { id: i } if *i == id).then_some(()));
    let st = wait(&mut w, 5, "status", |a| status_of(a, id));
    assert!(st.flags.contains(TorrentFlags::PAUSED) && st.has_metadata);
    assert!(st.total_done < f.piece_len as u64 * 2, "stopped before downloading much: {}", st.total_done);
    // setting the flag on a torrent that is ready stops it at once
    let h = leech.handle().torrent(id);
    h.resume();
    wait(&mut w, 10, "resumed", |a| matches!(a, Alert::TorrentResumed { id: i } if *i == id).then_some(()));
    h.set_flags(TorrentFlags::STOP_WHEN_READY);
    wait(&mut w, 10, "paused again", |a| {
        matches!(a, Alert::TorrentPaused { id: i } if *i == id).then_some(())
    });
    leech.shutdown();
    seed.shutdown();
    let _ = std::fs::remove_dir_all(dir);
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
fn a_torrent_files_dht_nodes_join_the_routing_table() {
    // libtorrent `add_torrent_impl`: a `.torrent`'s `nodes` go to the DHT, host names resolved
    let f = fixture(None);
    let (router, _rw, router_port) = start_dht(None);
    let (leech, mut w, _) = start_dht(None);
    let d = patorrent_core::bencode::bdecode(&f.torrent[..]).unwrap();
    let mut e = Entry::from_node(d.root().unwrap());
    *e.key("nodes") =
        Entry::List(vec![Entry::List(vec!["localhost".into(), Entry::Int(router_port as i64)])]);
    let dir = temp_dir("dht-nodes");
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(e.bencode()), dir.to_string_lossy());
    p.flags = TorrentFlags::PAUSED;
    leech.handle().add_torrent(p);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        leech.handle().post_session_stats();
        let n = wait(&mut w, 5, "stats", |a| match a {
            Alert::SessionStats(s) => Some(s.dht_nodes),
            _ => None,
        });
        if n >= 1 {
            break;
        }
        assert!(Instant::now() < deadline, "the torrent's node never joined the routing table");
        std::thread::sleep(Duration::from_millis(200));
    }
    leech.shutdown();
    router.shutdown();
    let _ = std::fs::remove_dir_all(dir);
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
    let id = first_restored(&mut w).id;
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
    // the seed's ut_pex message. A has PEX off: were it to learn B's address too, the two would
    // race to connect each other and the duplicate-connection rule could keep A's connection,
    // which B sees as incoming (a coin flip on the peer ids).
    let f = fixture(None);
    let (seed, seed_dir, seed_port, _) = seeder(&f);
    let seed_addr = SocketAddr::from(([127, 0, 0, 1], seed_port));

    let a_dir = temp_dir("pex-a");
    let a = start(None);
    let mut aw = Watch::new(a.handle());
    let a_port = listen_port(&mut aw);
    let a_id =
        add_with_flags(&mut aw, &f, &a_dir, TorrentFlags::DEFAULT_DONT_DOWNLOAD | TorrentFlags::DISABLE_PEX);
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
    add_with_flags(w, f, dir, TorrentFlags::DEFAULT_DONT_DOWNLOAD)
}

fn add_with_flags(w: &mut Watch, f: &Fixture, dir: &Path, flags: TorrentFlags) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = flags;
    w.h.add_torrent(p);
    wait(w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}

#[test]
fn a_leech_finds_the_seed_through_local_service_discovery() {
    // No tracker, no DHT and no connect_peer: the two sessions must find each other through
    // their BEP 14 multicast announces on the loopback interface.
    let lsd = || {
        Session::start(SessionParams {
            settings: Settings { enable_lsd: true, ..settings() },
            state_dir: None,
        })
        .expect("session starts")
    };
    // an info-hash of its own, so no other test's session takes part
    let f = fixture_padded(None, 77);
    let seed_dir = temp_dir("lsd-seed");
    write_files(&f, &seed_dir);
    let seed = lsd();
    let mut sw = Watch::new(seed.handle());
    let seed_id = add(&mut sw, &f, &seed_dir);
    wait_state(&mut sw, seed_id, TorrentState::Seeding, 20);

    let dir = temp_dir("lsd-leech");
    let leech = lsd();
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, &f, &dir);
    let mut leech_heard_seed = false;
    wait(&mut w, 60, "finished", |a| match a {
        Alert::LsdPeer { id: i, .. } if *i == id => {
            leech_heard_seed = true;
            None
        }
        Alert::TorrentFinished { id: i } if *i == id => Some(()),
        _ => None,
    });
    if !leech_heard_seed {
        // the seed heard the leech's announce and connected to it
        wait(&mut sw, 5, "LSD peer on the seed", |a| {
            matches!(a, Alert::LsdPeer { id, .. } if *id == seed_id).then_some(())
        });
    }
    leech.shutdown();
    seed.shutdown();
    assert_files(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn a_second_connection_to_the_same_peer_is_closed() {
    // The seed listens on every IPv4 address; the leech reaches it at two loopback addresses.
    // Both handshakes carry the seed's peer-id, so one connection must go (libtorrent
    // `duplicate_peer_id`), and both ends must agree on which.
    let f = fixture(None);
    let seed_dir = temp_dir("dup-seed");
    write_files(&f, &seed_dir);
    let seed = Session::start(SessionParams {
        settings: Settings { listen_interfaces: "0.0.0.0:0".into(), ..settings() },
        state_dir: None,
    })
    .expect("session starts");
    let mut sw = Watch::new(seed.handle());
    let port = listen_port(&mut sw);
    let seed_id = add_wanting_nothing(&mut sw, &f, &seed_dir);
    wait_state(&mut sw, seed_id, TorrentState::Seeding, 20);
    let dir = temp_dir("dup-leech");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let id = add_wanting_nothing(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Finished, 10);
    // (FreeBSD answers only on 127.0.0.1; the test then sees a single connection anyway)
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 2], port)));
    std::thread::sleep(Duration::from_secs(2));
    let connected = |w: &mut Watch, s: &Session, id: TorrentId| {
        s.handle().torrent(id).post_peer_info();
        let peers = wait(w, 5, "peer info", |a| match a {
            Alert::PeerInfo { id: i, peers } if *i == id => Some(peers.clone()),
            _ => None,
        });
        peers
            .into_iter()
            .filter(|p| !p.flags.intersects(PeerFlags::CONNECTING | PeerFlags::HANDSHAKE))
            .count()
    };
    assert_eq!(connected(&mut w, &leech, id), 1, "leech side");
    assert_eq!(connected(&mut sw, &seed, seed_id), 1, "seed side");
    leech.shutdown();
    seed.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn disconnects_are_posted_with_their_reason_when_the_mask_asks() {
    // libtorrent `peer_disconnected_alert`, in the connect category: off by default, so the seed
    // posts none; the other seed asks for it and sees the two seeds part, then a refused connect
    let f = fixture(None);
    let (seed, seed_dir, port, _) = seeder(&f);
    let sw = Watch::new(seed.handle());
    let dir = temp_dir("disconnect-seed");
    write_files(&f, &dir);
    let (other, mut w, _) = start_with(|s| {
        s.alert_mask |= patorrent_core::AlertCategory::CONNECT.bits();
        // a refused TCP connect is reported at once; a uTP one would wait for its timeout
        s.enable_outgoing_utp = false;
    });
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Seeding, 20);
    let seed_addr = SocketAddr::from(([127, 0, 0, 1], port));
    other.handle().torrent(id).connect_peer(seed_addr);
    let (reason, error, utp) = wait(&mut w, 15, "the seeds parting", |a| match a {
        Alert::PeerDisconnected { id: Some(i), addr, reason, error, utp, .. }
            if *i == id && *addr == seed_addr =>
        {
            Some((reason.clone(), *error, *utp))
        }
        _ => None,
    });
    // whichever side reads the other's bitfield first closes; the other may see it close
    assert!(reason == "both sides are seeds" || reason == "connection closed by peer", "{reason}");
    assert!(!error && !utp, "{reason}");

    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    other.handle().torrent(id).connect_peer(closed);
    let (reason, error) = wait(&mut w, 15, "a refused connect", |a| match a {
        Alert::PeerDisconnected { addr, reason, error, .. } if *addr == closed => {
            Some((reason.clone(), *error))
        }
        _ => None,
    });
    assert!(error && reason.starts_with("connect failed"), "{reason}");

    let mut alerts = Vec::new();
    sw.h.pop_alerts(&mut alerts);
    assert!(
        !alerts.iter().any(|a| matches!(a, Alert::PeerDisconnected { .. })),
        "the default mask posts none"
    );
    other.shutdown();
    seed.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn the_announce_limits_leave_the_last_running_torrents_out() {
    // libtorrent `auto_manage_torrents`: of the running auto-managed torrents the first
    // `active_tracker_limit` announce to their trackers, the first `active_lsd_limit` on LSD,
    // the first `active_dht_limit` on the DHT; a forced torrent announces everywhere
    let dir = temp_dir("announce-limits");
    // three torrents of the same files (the padding gives each its own info-hash)
    let fixtures: Vec<Fixture> = (1..=3).map(|n| fixture_padded(None, n)).collect();
    write_files(&fixtures[0], &dir);
    let (s, mut w, _) = start_with(|s| {
        s.active_tracker_limit = 1;
        s.active_lsd_limit = 2;
        s.active_dht_limit = -1;
        s.auto_manage_interval = 1;
    });
    let ids: Vec<TorrentId> =
        fixtures.iter().map(|f| add_with_flags(&mut w, f, &dir, TorrentFlags::AUTO_MANAGED)).collect();
    // the latest status of each, until the counts are as expected
    let mut latest: std::collections::HashMap<TorrentId, TorrentStatus> = std::collections::HashMap::new();
    let mut counts = |w: &mut Watch, want: (usize, usize, usize), what: &str| {
        wait(w, 20, what, |a| {
            for id in &ids {
                if let Some(st) = status_of(a, *id) {
                    latest.insert(*id, st);
                }
            }
            let seeding =
                latest.len() == ids.len() && latest.values().all(|st| st.state == TorrentState::Seeding);
            let count = |f: fn(&TorrentStatus) -> bool| latest.values().filter(|st| f(st)).count();
            let got = (
                count(|st| st.announcing_to_trackers),
                count(|st| st.announcing_to_lsd),
                count(|st| st.announcing_to_dht),
            );
            (seeding && got == want).then_some(())
        });
        latest.clone()
    };
    let st = counts(&mut w, (1, 2, 3), "one tracker, two LSD and three DHT announcers");
    // forcing one that was left out of the trackers makes it announce there too, and frees the
    // auto-managed slot it held for the first of the others
    let left_out = *st.iter().find(|(_, st)| !st.announcing_to_trackers).expect("one is left out").0;
    s.handle().torrent(left_out).unset_flags(TorrentFlags::AUTO_MANAGED);
    let st = counts(&mut w, (2, 3, 3), "the forced torrent announcing too");
    assert!(st[&left_out].announcing_to_trackers && st[&left_out].announcing_to_lsd);
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

// --- MSE/PE protocol encryption ------------------------------------------------------------------

fn start_with(f: impl FnOnce(&mut Settings)) -> (Session, Watch, u16) {
    let mut settings = settings();
    f(&mut settings);
    let s = Session::start(SessionParams { settings, state_dir: None }).expect("session starts");
    let mut w = Watch::new(s.handle());
    let port = listen_port(&mut w);
    (s, w, port)
}

fn forced_rc4(s: &mut Settings) {
    s.out_enc_policy = EncPolicy::Forced;
    s.in_enc_policy = EncPolicy::Forced;
    s.allowed_enc_level = EncLevel::Rc4;
}

fn forced_any(s: &mut Settings) {
    s.out_enc_policy = EncPolicy::Forced;
    s.in_enc_policy = EncPolicy::Forced;
}

fn disabled(s: &mut Settings) {
    s.out_enc_policy = EncPolicy::Disabled;
    s.in_enc_policy = EncPolicy::Disabled;
}

/// The flags of the torrent's connected (past the handshake) peers.
fn connected_peer_flags(w: &mut Watch, id: TorrentId) -> Vec<PeerFlags> {
    w.h.torrent(id).post_peer_info();
    let peers = wait(w, 5, "peer info", |a| match a {
        Alert::PeerInfo { id: i, peers } if *i == id => Some(peers.clone()),
        _ => None,
    });
    peers
        .into_iter()
        .filter(|p| !p.flags.intersects(PeerFlags::CONNECTING | PeerFlags::HANDSHAKE))
        .map(|p| p.flags)
        .collect()
}

/// A seed and a leech with the given settings; the leech connects to the seed and must finish.
/// Returns the leech's view of the seed connection and the seed's view of the leech.
fn encrypted_download(
    seed_settings: fn(&mut Settings),
    leech_settings: fn(&mut Settings),
) -> (PeerFlags, PeerFlags) {
    let f = fixture(None);
    let seed_dir = temp_dir("enc-seed");
    write_files(&f, &seed_dir);
    let (seed, mut sw, port) = start_with(seed_settings);
    let seed_id = add(&mut sw, &f, &seed_dir);
    wait_state(&mut sw, seed_id, TorrentState::Seeding, 20);
    let dir = temp_dir("enc-leech");
    // Slow enough that the connection can be looked at while it lasts.
    let (leech, mut w, _) = start_with(|s| {
        leech_settings(s);
        s.download_rate_limit = 300_000;
    });
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    // Both views of the connection while it lasts: once the leech is a seed too, the
    // connection is redundant and closed.
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut leech_view, mut seed_view) = (Vec::new(), Vec::new());
    while Instant::now() < deadline && (leech_view.is_empty() || seed_view.is_empty()) {
        if leech_view.is_empty() {
            leech_view = connected_peer_flags(&mut w, id);
        }
        if seed_view.is_empty() {
            seed_view = connected_peer_flags(&mut sw, seed_id);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(leech_view.len(), 1, "leech side: {leech_view:?}");
    assert_eq!(seed_view.len(), 1, "seed side: {seed_view:?}");
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert_eq!(st.total_failed_bytes, 0);
    leech.shutdown();
    seed.shutdown();
    assert_files(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
    (leech_view[0], seed_view[0])
}

#[test]
fn forced_encryption_on_both_sides_downloads_over_rc4() {
    let (leech_view, seed_view) = encrypted_download(forced_rc4, forced_rc4);
    assert!(leech_view.contains(PeerFlags::RC4_ENCRYPTED), "{leech_view:?}");
    assert!(seed_view.contains(PeerFlags::RC4_ENCRYPTED), "{seed_view:?}");
}

#[test]
fn forced_encryption_with_both_levels_allowed_agrees_on_a_plaintext_stream() {
    // Both offer plaintext and RC4; without `prefer_rc4` the responder keeps the lower bit, so
    // the handshake is encrypted and the payload is not (libtorrent `plaintext_encrypted`).
    let (leech_view, seed_view) = encrypted_download(forced_any, forced_any);
    assert!(leech_view.contains(PeerFlags::PLAINTEXT_ENCRYPTED), "{leech_view:?}");
    assert!(!leech_view.contains(PeerFlags::RC4_ENCRYPTED), "{leech_view:?}");
    assert!(seed_view.contains(PeerFlags::PLAINTEXT_ENCRYPTED), "{seed_view:?}");
}

#[test]
fn an_enabled_leech_retries_with_encryption_when_a_forced_seed_refuses_plaintext() {
    // `Enabled` (the default) tries plaintext first with a peer it knows nothing about; the
    // seed closes that connection ("no incoming plaintext connections allowed"), and the leech
    // reconnects at once with the encrypted handshake (libtorrent's `pe_support` toggle and
    // `fast_reconnect`).
    let (leech_view, seed_view) = encrypted_download(forced_rc4, |_| {});
    assert!(leech_view.contains(PeerFlags::RC4_ENCRYPTED), "{leech_view:?}");
    assert!(seed_view.contains(PeerFlags::RC4_ENCRYPTED), "{seed_view:?}");
}

#[test]
fn enabled_peers_talk_plaintext_with_a_disabled_peer_in_either_direction() {
    let plain =
        |flags: PeerFlags| !flags.intersects(PeerFlags::RC4_ENCRYPTED | PeerFlags::PLAINTEXT_ENCRYPTED);
    let (leech_view, seed_view) = encrypted_download(disabled, |_| {});
    assert!(plain(leech_view) && plain(seed_view), "{leech_view:?} {seed_view:?}");
    let (leech_view, seed_view) = encrypted_download(|_| {}, disabled);
    assert!(plain(leech_view) && plain(seed_view), "{leech_view:?} {seed_view:?}");
}

#[test]
fn forced_and_disabled_peers_never_connect() {
    for (seed_settings, leech_settings, what) in [
        (disabled as fn(&mut Settings), forced_rc4 as fn(&mut Settings), "forced leech, disabled seed"),
        (forced_rc4, disabled, "disabled leech, forced seed"),
    ] {
        let f = fixture(None);
        let seed_dir = temp_dir("noenc-seed");
        write_files(&f, &seed_dir);
        let (seed, mut sw, port) = start_with(seed_settings);
        let seed_id = add(&mut sw, &f, &seed_dir);
        wait_state(&mut sw, seed_id, TorrentState::Seeding, 20);
        let dir = temp_dir("noenc-leech");
        let (leech, mut w, _) = start_with(leech_settings);
        let id = add(&mut w, &f, &dir);
        wait_state(&mut w, id, TorrentState::Downloading, 10);
        leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
        std::thread::sleep(Duration::from_secs(3));
        assert!(connected_peer_flags(&mut w, id).is_empty(), "{what}: leech connected");
        assert!(connected_peer_flags(&mut sw, seed_id).is_empty(), "{what}: seed connected");
        let st = wait_state(&mut w, id, TorrentState::Downloading, 5);
        assert_eq!(st.total_done, 0, "{what}");
        leech.shutdown();
        seed.shutdown();
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(seed_dir);
    }
}

// --- uTP (BEP 29) ----------------------------------------------------------------------------

fn tcp_only(s: &mut Settings) {
    s.enable_incoming_utp = false;
    s.enable_outgoing_utp = false;
}

fn utp_only(s: &mut Settings) {
    s.enable_incoming_tcp = false;
    s.enable_outgoing_tcp = false;
}

fn forced_rc4_over_utp(s: &mut Settings) {
    forced_rc4(s);
    utp_only(s);
}

/// A leech that gives up on a uTP connect after a second instead of three.
fn quick_utp_connect(s: &mut Settings) {
    s.utp_connect_timeout = 1000;
}

#[test]
fn the_default_settings_connect_over_utp() {
    // libtorrent assumes every peer supports uTP until a uTP connect to it fails, so with both
    // transports on the first attempt is uTP (`torrent_peer::supports_utp` starts true).
    let (leech_view, seed_view) = encrypted_download(|_| {}, |_| {});
    assert!(leech_view.contains(PeerFlags::UTP_SOCKET), "{leech_view:?}");
    assert!(seed_view.contains(PeerFlags::UTP_SOCKET), "{seed_view:?}");
}

#[test]
fn utp_only_sessions_download_over_utp() {
    // libtorrent test_utp.cpp: TCP off on both sides, in and out
    let (leech_view, seed_view) = encrypted_download(utp_only, utp_only);
    assert!(leech_view.contains(PeerFlags::UTP_SOCKET), "{leech_view:?}");
    assert!(seed_view.contains(PeerFlags::UTP_SOCKET), "{seed_view:?}");
}

#[test]
fn a_leech_falls_back_to_tcp_when_the_seed_has_no_utp() {
    // The seed has no UDP socket at all (no DHT, no uTP): the leech's SYN gets no answer, the
    // uTP connect times out after `utp_connect_timeout`, the peer is marked as not supporting
    // uTP and reconnected over TCP at once (libtorrent `on_connection_failed`).
    let (leech_view, seed_view) = encrypted_download(tcp_only, quick_utp_connect);
    assert!(!leech_view.contains(PeerFlags::UTP_SOCKET), "{leech_view:?}");
    assert!(!seed_view.contains(PeerFlags::UTP_SOCKET), "{seed_view:?}");
}

#[test]
fn a_tcp_only_leech_reaches_a_utp_seed_over_tcp() {
    let (leech_view, seed_view) = encrypted_download(|_| {}, tcp_only);
    assert!(!leech_view.contains(PeerFlags::UTP_SOCKET), "{leech_view:?}");
    assert!(!seed_view.contains(PeerFlags::UTP_SOCKET), "{seed_view:?}");
}

#[test]
fn encryption_works_over_utp() {
    let (leech_view, seed_view) = encrypted_download(forced_rc4_over_utp, forced_rc4_over_utp);
    assert!(leech_view.contains(PeerFlags::UTP_SOCKET | PeerFlags::RC4_ENCRYPTED), "{leech_view:?}");
    assert!(seed_view.contains(PeerFlags::UTP_SOCKET | PeerFlags::RC4_ENCRYPTED), "{seed_view:?}");
}

#[test]
fn utp_only_and_tcp_only_sessions_never_connect() {
    let f = fixture(None);
    let seed_dir = temp_dir("noutp-seed");
    write_files(&f, &seed_dir);
    let (seed, mut sw, port) = start_with(tcp_only);
    let seed_id = add(&mut sw, &f, &seed_dir);
    wait_state(&mut sw, seed_id, TorrentState::Seeding, 20);
    let dir = temp_dir("noutp-leech");
    let (leech, mut w, _) = start_with(|s| {
        utp_only(s);
        quick_utp_connect(s);
    });
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    std::thread::sleep(Duration::from_secs(3));
    assert!(connected_peer_flags(&mut w, id).is_empty(), "leech connected");
    assert!(connected_peer_flags(&mut sw, seed_id).is_empty(), "seed connected");
    let st = wait_state(&mut w, id, TorrentState::Downloading, 5);
    assert_eq!(st.total_done, 0);
    leech.shutdown();
    seed.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

/// The reply to a BitTorrent handshake from a scripted peer: `reserved` bits, our info-hash
/// and a made-up peer id.
fn scripted_handshake(info_hash: &[u8; 20], reserved: [u8; 8], peer_id: &[u8; 20]) -> Vec<u8> {
    let mut out = vec![19u8];
    out.extend_from_slice(b"BitTorrent protocol");
    out.extend_from_slice(&reserved);
    out.extend_from_slice(info_hash);
    out.extend_from_slice(peer_id);
    out
}

/// One length-prefixed message, or `None` once the connection is gone.
fn read_message(c: &mut std::net::TcpStream) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut len = [0u8; 4];
    c.read_exact(&mut len).ok()?;
    let mut msg = vec![0u8; u32::from_be_bytes(len) as usize];
    c.read_exact(&mut msg).ok()?;
    Some(msg)
}

/// libtorrent test_fast_extension.cpp `dont_have`: a scripted peer claims everything with
/// HAVE_ALL, reads our extension handshake for the `lt_donthave` id, and takes piece 3 back
/// once `go` fires. It stays connected until the leech closes.
fn dont_have_peer(f: &Fixture, go: std::sync::mpsc::Receiver<()>) -> (u16, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let info_hash = f.info_hash;
    let h = std::thread::spawn(move || {
        let (mut c, _) = l.accept().unwrap();
        c.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let mut hs = [0u8; 68];
        c.read_exact(&mut hs).unwrap();
        assert_eq!(&hs[28..48], &info_hash);
        assert!(hs[25] & 0x10 != 0, "the leech offers extensions");
        let mut reserved = [0u8; 8];
        reserved[5] |= 0x10; // BEP 10
        reserved[7] |= 0x04; // BEP 6
        let mut out = scripted_handshake(&info_hash, reserved, b"-XX0001-donthavepeer");
        out.extend_from_slice(&[0, 0, 0, 1, 0x0e]); // have_all
        c.write_all(&out).unwrap();
        // Our extension handshake names lt_donthave.
        let id = loop {
            let msg = read_message(&mut c).expect("extension handshake before the close");
            if msg.len() > 2 && msg[0] == 20 && msg[1] == 0 {
                let doc = patorrent_core::bencode::bdecode(&msg[2..]).unwrap();
                let m = doc.root().unwrap().dict_find_dict("m").expect("m");
                break m.dict_find_int_value("lt_donthave").expect("lt_donthave") as u8;
            }
        };
        assert_ne!(id, 0);
        go.recv().unwrap();
        c.write_all(&[0, 0, 0, 6, 20, id, 0, 0, 0, 3]).unwrap();
        while read_message(&mut c).is_some() {}
    });
    (port, h)
}

fn peer_info(w: &mut Watch, id: TorrentId) -> Vec<patorrent_core::PeerInfo> {
    w.h.torrent(id).post_peer_info();
    wait(w, 5, "peer info", |a| match a {
        Alert::PeerInfo { id: i, peers } if *i == id => Some(peers.clone()),
        _ => None,
    })
}

#[test]
fn a_peer_takes_a_piece_back_with_dont_have() {
    let f = fixture(None);
    let num_pieces = f.files.iter().map(|(_, d)| d.len()).sum::<usize>().div_ceil(PIECE) as u32;
    let dir = temp_dir("donthave");
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    let (port, peer) = dont_have_peer(&f, go_rx);
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    // at this point, the peer should be considered a seed
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let peers = peer_info(&mut w, id);
        if peers.iter().any(|p| p.flags.contains(PeerFlags::SEED)) {
            break;
        }
        assert!(Instant::now() < deadline, "the peer never became a seed: {peers:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    go_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let expected = (num_pieces as u64 - 1) * 1_000_000 / num_pieces as u64;
    let p = loop {
        let peers = peer_info(&mut w, id);
        assert_eq!(peers.len(), 1, "{peers:?}");
        if !peers[0].flags.contains(PeerFlags::SEED) {
            break peers.into_iter().next().unwrap();
        }
        assert!(Instant::now() < deadline, "the peer is still a seed: {peers:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(p.progress_ppm as u64, expected, "all pieces but one");
    assert!(p.flags.contains(PeerFlags::INTERESTING), "still worth talking to");
    // The picker's availability follows: piece 3 is gone, the rest is still there.
    w.h.torrent(id).post_piece_info();
    let availability = wait(&mut w, 5, "piece info", |a| match a {
        Alert::PieceInfo { id: i, availability, .. } if *i == id => Some(availability.clone()),
        _ => None,
    });
    assert_eq!(availability.len(), num_pieces as usize);
    assert_eq!(availability[3], 0);
    assert!(availability.iter().enumerate().all(|(i, &a)| i == 3 || a == 1), "{availability:?}");
    leech.shutdown();
    peer.join().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

/// A scripted peer that has every piece but the last and answers its first `bad_blocks`
/// requests with corrupted data, then chokes and answers nothing more; `served` turns true once
/// the bad blocks are out. Returns the number of blocks it sent once the connection closes.
/// The last piece of the fixture is a single short block: claiming it would make this peer the
/// only sender of a whole piece one time in six (the picker's random tie-break), which is the
/// sole-sender ban of `penalize_peers`, not smart_ban's.
fn corrupting_peer(
    f: &Fixture,
    bad_blocks: usize,
    served: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> (u16, std::thread::JoinHandle<usize>) {
    use std::io::{Read, Write};
    use std::sync::atomic::Ordering;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let all: Vec<u8> = f.files.iter().flat_map(|(_, d)| d.iter().copied()).collect();
    let (info_hash, piece_len) = (f.info_hash, f.piece_len);
    let h = std::thread::spawn(move || {
        let (mut c, _) = l.accept().unwrap();
        c.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let mut hs = [0u8; 68];
        c.read_exact(&mut hs).unwrap();
        assert_eq!(&hs[28..48], &info_hash);
        let mut out = scripted_handshake(&info_hash, [0; 8], b"-XX0001-corruptpeer0");
        let num_pieces = all.len().div_ceil(piece_len);
        let mut bits = vec![0u8; num_pieces.div_ceil(8)];
        for i in 0..num_pieces - 1 {
            bits[i / 8] |= 0x80 >> (i % 8);
        }
        out.extend_from_slice(&(bits.len() as u32 + 1).to_be_bytes());
        out.push(5);
        out.extend_from_slice(&bits);
        out.extend_from_slice(&[0, 0, 0, 1, 1]); // unchoke
        c.write_all(&out).unwrap();
        let mut sent = 0;
        while let Some(msg) = read_message(&mut c) {
            if msg.first() != Some(&6) || sent >= bad_blocks {
                continue;
            }
            let field = |i: usize| u32::from_be_bytes(msg[1 + 4 * i..5 + 4 * i].try_into().unwrap());
            let (piece, begin, length) = (field(0), field(1), field(2));
            let start = piece as usize * piece_len + begin as usize;
            let mut out = (9 + length).to_be_bytes().to_vec();
            out.push(7);
            out.extend_from_slice(&piece.to_be_bytes());
            out.extend_from_slice(&begin.to_be_bytes());
            out.extend(all[start..start + length as usize].iter().map(|b| !b));
            sent += 1;
            if sent == bad_blocks {
                out.extend_from_slice(&[0, 0, 0, 1, 0]); // choke: nothing more from here
            }
            c.write_all(&out).unwrap();
            if sent == bad_blocks {
                served.store(true, Ordering::SeqCst);
            }
        }
        sent
    });
    (port, h)
}

/// libtorrent's smart_ban has no unit test; this is its scenario on loopback. A bad peer sends
/// two corrupted blocks of a 16-block piece and chokes; the seed sends the other fourteen, the
/// piece fails, and both are only suspects (trust points, parole). The seed then downloads the
/// piece again by itself, it passes, and the blocks recorded at the failure are compared: the
/// bad peer's differ, so it is banned; the seed's match, so it is kept and finishes the job.
#[test]
fn smart_ban_bans_the_sender_of_a_bad_block_and_keeps_the_seed() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = fixture_with(None, 0, 256 * 1024);
    let dir = temp_dir("smartban-leech");
    let served = Arc::new(AtomicBool::new(false));
    let (bad_port, bad) = corrupting_peer(&f, 2, served.clone());
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], bad_port)));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !served.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the bad peer was never asked for two blocks");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (seed, seed_dir, seed_port, _) = seeder(&f);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], seed_port)));
    let bad_addr = SocketAddr::from(([127, 0, 0, 1], bad_port));
    let mut banned = Vec::new();
    let mut finished = false;
    wait(&mut w, 60, "finished with the bad peer banned", |a| {
        match a {
            Alert::PeerBanned { id: i, addr, peer_id } if *i == id => banned.push((*addr, *peer_id)),
            Alert::TorrentFinished { id: i } if *i == id => finished = true,
            _ => {}
        }
        (finished && !banned.is_empty()).then_some(())
    });
    assert_eq!(banned.len(), 1, "{banned:?}");
    assert_eq!(banned[0].0, bad_addr);
    assert_eq!(&banned[0].1[..8], b"-XX0001-", "banned while connected");
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert_eq!(st.total_failed_bytes, f.piece_len as u64, "the one piece with the bad blocks");
    assert_eq!(bad.join().unwrap(), 2);
    leech.shutdown();
    seed.shutdown();
    assert_files(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

// --- IP filter -------------------------------------------------------------------------------

fn loopback_blocked() -> IpFilter {
    let mut f = IpFilter::new();
    f.add_rule([127, 0, 0, 0].into(), [127, 255, 255, 255].into(), IpFilter::BLOCKED);
    f
}

fn blocked_alert(w: &mut Watch, what: &str, want: Option<TorrentId>, reason: BlockReason) -> SocketAddr {
    wait(w, 10, what, |a| match a {
        Alert::PeerBlocked { id, addr, reason: r } if *id == want && *r == reason => Some(*addr),
        _ => None,
    })
}

/// Every peer the session lists for the torrent, connecting or connected.
fn all_peers(w: &mut Watch, id: TorrentId) -> usize {
    w.h.torrent(id).post_peer_info();
    wait(w, 5, "peer info", |a| match a {
        Alert::PeerInfo { id: i, peers } if *i == id => Some(peers.len()),
        _ => None,
    })
}

fn add_with_tracker(w: &mut Watch, f: &Fixture, dir: &Path, url: String) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::APPLY_IP_FILTER;
    p.trackers = vec![url];
    w.h.add_torrent(p);
    wait(w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    })
}

#[test]
fn a_seed_in_a_blocked_range_is_never_connected() {
    let f = fixture(None);
    let (seed, seed_dir, port, seed_id) = seeder(&f);
    let mut sw = Watch::new(seed.handle());
    let dir = temp_dir("ipf-leech");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    leech.handle().set_ip_filter(loopback_blocked());
    let id = add_with_flags(&mut w, &f, &dir, TorrentFlags::APPLY_IP_FILTER);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    let seed_addr = SocketAddr::from(([127, 0, 0, 1], port));
    leech.handle().torrent(id).connect_peer(seed_addr);
    assert_eq!(blocked_alert(&mut w, "blocked seed", Some(id), BlockReason::IpFilter), seed_addr);
    // a second attempt is refused the same way
    leech.handle().torrent(id).connect_peer(seed_addr);
    blocked_alert(&mut w, "blocked again", Some(id), BlockReason::IpFilter);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(all_peers(&mut w, id), 0, "no connection was even attempted");
    assert_eq!(all_peers(&mut sw, seed_id), 0, "the seed never saw a connection");
    leech.shutdown();
    seed.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn a_peer_on_a_blocked_port_is_never_connected() {
    let f = fixture(None);
    let (seed, seed_dir, port, seed_id) = seeder(&f);
    let mut sw = Watch::new(seed.handle());
    let dir = temp_dir("pf-leech");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let mut pf = PortFilter::new();
    pf.add_rule(port, port, PortFilter::BLOCKED);
    leech.handle().set_port_filter(pf);
    // the port filter applies whether or not the torrent applies the IP filter
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    blocked_alert(&mut w, "blocked port", Some(id), BlockReason::PortFilter);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(all_peers(&mut w, id), 0);
    assert_eq!(all_peers(&mut sw, seed_id), 0);
    leech.shutdown();
    seed.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn an_incoming_connection_from_a_blocked_address_is_refused() {
    let f = fixture(None);
    let seed_dir = temp_dir("ipf-in-seed");
    write_files(&f, &seed_dir);
    let (seed, mut sw, port) = start_with(|_| {});
    seed.handle().set_ip_filter(loopback_blocked());
    let seed_id = add_with_flags(&mut sw, &f, &seed_dir, TorrentFlags::APPLY_IP_FILTER);
    wait_state(&mut sw, seed_id, TorrentState::Seeding, 20);

    let dir = temp_dir("ipf-in-leech");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    // every torrent applies the filter, so the seed refuses before the handshake
    let from = blocked_alert(&mut sw, "refused incoming", None, BlockReason::IpFilter);
    assert!(from.ip().is_loopback());
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(all_peers(&mut sw, seed_id), 0);
    leech.handle().post_torrent_updates();
    let st = wait(&mut w, 5, "status", |a| status_of(a, id));
    assert_eq!(st.total_done, 0, "nothing was downloaded");
    leech.shutdown();
    seed.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn an_incoming_peer_is_refused_once_it_names_a_filtered_torrent() {
    // The seed has a second torrent that ignores the filter, so a blocked address is let in
    // until its handshake names the torrent that applies it (libtorrent `attach_peer`).
    let f = fixture(None);
    let other = fixture_padded(None, 3);
    let seed_dir = temp_dir("ipf-attach-seed");
    write_files(&f, &seed_dir);
    let (seed, mut sw, port) = start_with(|_| {});
    seed.handle().set_ip_filter(loopback_blocked());
    let seed_id = add_with_flags(&mut sw, &f, &seed_dir, TorrentFlags::APPLY_IP_FILTER);
    add_with_flags(&mut sw, &other, &seed_dir, TorrentFlags::empty());
    wait_state(&mut sw, seed_id, TorrentState::Seeding, 20);

    let dir = temp_dir("ipf-attach-leech");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    blocked_alert(&mut sw, "refused on attach", Some(seed_id), BlockReason::IpFilter);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(all_peers(&mut sw, seed_id), 0);
    leech.handle().post_torrent_updates();
    let st = wait(&mut w, 5, "status", |a| status_of(a, id));
    assert_eq!(st.total_done, 0, "nothing was downloaded");
    leech.shutdown();
    seed.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn a_new_ip_filter_closes_existing_connections() {
    // Two finished sessions that want nothing from each other stay connected (see
    // `a_second_connection_to_the_same_peer_is_closed`) until the leech blocks the seed.
    let f = fixture(None);
    let seed_dir = temp_dir("ipf-close-seed");
    write_files(&f, &seed_dir);
    let (seed, mut sw, port) = start_with(|_| {});
    let seed_id = add_wanting_nothing(&mut sw, &f, &seed_dir);
    wait_state(&mut sw, seed_id, TorrentState::Seeding, 20);
    let dir = temp_dir("ipf-close-leech");
    let (leech, mut w, _) = start_with(|_| {});
    let flags = TorrentFlags::DEFAULT_DONT_DOWNLOAD | TorrentFlags::APPLY_IP_FILTER;
    let id = add_with_flags(&mut w, &f, &dir, flags);
    wait_state(&mut w, id, TorrentState::Finished, 10);
    let seed_addr = SocketAddr::from(([127, 0, 0, 1], port));
    leech.handle().torrent(id).connect_peer(seed_addr);
    let deadline = Instant::now() + Duration::from_secs(10);
    while connected_peer_flags(&mut w, id).is_empty() {
        assert!(Instant::now() < deadline, "the leech never connected");
        std::thread::sleep(Duration::from_millis(100));
    }
    leech.handle().set_ip_filter(loopback_blocked());
    assert_eq!(blocked_alert(&mut w, "closed connection", Some(id), BlockReason::IpFilter), seed_addr);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(all_peers(&mut w, id), 0);
    // and it is not connected again
    leech.handle().torrent(id).connect_peer(seed_addr);
    blocked_alert(&mut w, "reconnect refused", Some(id), BlockReason::IpFilter);
    leech.shutdown();
    seed.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn trackers_in_a_blocked_range_are_skipped_unless_exempt() {
    let f = fixture(None);
    // a tracker that must never be contacted: nothing accepts on it
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    silent.set_nonblocking(true).unwrap();
    let dir = temp_dir("ipf-tracker");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    leech.handle().set_ip_filter(loopback_blocked());
    add_with_tracker(&mut w, &f, &dir, format!("http://{}/announce", silent.local_addr().unwrap()));
    let message = wait(&mut w, 10, "tracker error", |a| match a {
        Alert::TrackerError { message, .. } => Some(message.clone()),
        Alert::TrackerReply { .. } => panic!("a blocked tracker was announced to"),
        _ => None,
    });
    assert!(message.contains("IP filter"), "{message}");
    assert!(silent.accept().is_err(), "the tracker was contacted");
    leech.shutdown();
    let _ = std::fs::remove_dir_all(dir);

    // With `apply_ip_filter_to_trackers` off the tracker is asked, but the peer it hands out
    // is still blocked.
    let peer = SocketAddr::from(([127, 0, 0, 1], 7));
    let (url, tracker) = http_tracker(f.info_hash, peer);
    let dir = temp_dir("ipf-tracker-exempt");
    let (leech, mut w, _) = start_with(|s| s.apply_ip_filter_to_trackers = false);
    leech.handle().set_ip_filter(loopback_blocked());
    let id = add_with_tracker(&mut w, &f, &dir, url);
    assert_eq!(blocked_alert(&mut w, "tracker peer blocked", Some(id), BlockReason::IpFilter), peer);
    assert_eq!(tracker.join().unwrap(), 1);
    leech.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A torrent made by `create_torrent` from files on disk (pad files included) is seeded from
/// those files and downloads byte for byte.
#[test]
fn a_created_torrent_seeds_and_downloads() {
    use patorrent_core::metainfo::{
        CreateFlags, CreateTorrent, FileStorage, TorrentInfo, add_files, set_piece_hashes,
    };

    let mut f = fixture(None);
    let seed_dir = temp_dir("create-seed");
    write_files(&f, &seed_dir);
    let mut fs = FileStorage::new();
    add_files(&mut fs, &seed_dir.join("swarm-test"), &mut |_| true, CreateFlags::empty()).unwrap();
    let mut t = CreateTorrent::new(fs, 0, CreateFlags::V1_ONLY | CreateFlags::CANONICAL_FILES).unwrap();
    t.set_creator("swarm test");
    set_piece_hashes(&mut t, &seed_dir, &mut |_| true).unwrap();
    f.torrent = t.generate_buf().unwrap();
    let ti = TorrentInfo::from_bytes(&f.torrent).unwrap();
    assert!((0..ti.num_files()).any(|i| ti.files().pad_file_at(i)));
    assert_eq!(
        ti.num_files() - (0..ti.num_files()).filter(|&i| ti.files().pad_file_at(i)).count(),
        f.files.len()
    );
    f.info_hash = ti.info_hash().0;
    f.piece_len = ti.piece_length() as usize;

    let seed = start(None);
    let mut sw = Watch::new(seed.handle());
    let port = listen_port(&mut sw);
    let sid = add(&mut sw, &f, &seed_dir);
    let st = wait_state(&mut sw, sid, TorrentState::Seeding, 20);
    assert_eq!(st.total_done, st.total);

    let dir = temp_dir("create-leech");
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

#[test]
fn a_skipped_file_stays_in_the_part_file_until_it_is_unskipped() {
    use patorrent_core::{DownloadPriority, RemoveFlags};
    let f = fixture(None);
    let (seed, seed_dir, port, _) = seeder(&f);
    let dir = temp_dir("leech-skip");
    let state = temp_dir("state-skip");
    let beta = dir.join("swarm-test/sub/beta.bin");
    let part = dir.join(format!(".{}.parts", hex(&f.info_hash)));
    let seed_addr = SocketAddr::from(([127, 0, 0, 1], port));
    {
        let leech = start(Some(state.clone()));
        let mut w = Watch::new(leech.handle());
        let mut p =
            AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
        p.flags = TorrentFlags::empty();
        // Skip the middle file: its first and last pieces are shared with alpha and gamma.
        p.file_priorities = vec![DownloadPriority::NORMAL, DownloadPriority::DONT_DOWNLOAD];
        leech.handle().add_torrent(p);
        let id = wait(&mut w, 5, "add", |a| match a {
            Alert::TorrentAdded { id, .. } => Some(*id),
            _ => None,
        });
        wait_state(&mut w, id, TorrentState::Downloading, 10);
        leech.handle().torrent(id).connect_peer(seed_addr);
        let st = wait_state(&mut w, id, TorrentState::Finished, 60);
        assert_eq!(st.total_wanted_done, st.total_wanted);
        assert_eq!(st.total_failed_bytes, 0, "the boundary pieces pass their hash checks");
        assert!(!beta.exists(), "the skipped file is not created");
        assert!(part.exists(), "the boundary pieces' beta blocks are in the part file");
        for (i, (name, data)) in f.files.iter().enumerate() {
            if i != 1 {
                assert!(std::fs::read(dir.join("swarm-test").join(name)).unwrap() == *data, "{name}");
            }
        }
        leech.shutdown();
    }

    // Restart: still finished, nothing created; a recheck finds the boundary pieces again.
    let leech = start(Some(state.clone()));
    let mut w = Watch::new(leech.handle());
    let id = first_restored(&mut w).id;
    let st = wait_state(&mut w, id, TorrentState::Finished, 10);
    let wanted = st.total_wanted;
    assert_eq!(st.total_wanted_done, wanted);
    leech.handle().torrent(id).force_recheck();
    wait(&mut w, 20, "checked", |a| matches!(a, Alert::TorrentChecked { id: i } if *i == id).then_some(()));
    let st = wait_state(&mut w, id, TorrentState::Finished, 20);
    assert_eq!(st.total_wanted_done, wanted, "the recheck reads the part file");
    assert!(!beta.exists());

    // Unskip: the boundary blocks move into beta and the rest is downloaded. The restarted
    // leech closed its connection to the first seed as redundant (finished, and the other end
    // is a seed) and waits min_reconnect_time before trying it again, so use a second seed.
    leech.handle().torrent(id).set_file_priority(1, DownloadPriority::NORMAL);
    let (seed2, seed2_dir, port2, _) = seeder(&f);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port2)));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 60);
    assert_eq!(st.total_failed_bytes, 0);
    assert_files(&f, &dir);
    assert!(!part.exists(), "the part file is deleted once nothing is in it");

    // Skip gamma again, which exists: it keeps its data, and removing with DELETE_PARTFILE
    // leaves the downloaded files alone.
    leech.handle().torrent(id).set_file_priority(2, DownloadPriority::DONT_DOWNLOAD);
    wait_state(&mut w, id, TorrentState::Seeding, 10);
    leech.handle().remove_torrent(id, RemoveFlags::DELETE_PARTFILE);
    wait(&mut w, 10, "deleted", |a| {
        matches!(a, Alert::TorrentDeleted { id: i, .. } if *i == id).then_some(())
    });
    leech.shutdown();
    seed.shutdown();
    seed2.shutdown();
    assert_files(&f, &dir);
    for d in [dir, state, seed_dir, seed2_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}
