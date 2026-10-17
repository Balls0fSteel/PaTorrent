//! Loopback tests for BEP 16 super seeding (µTorrent "initial seeding"), after libtorrent's
//! `test_super_seeding.cpp` (a super seed and leeches that all finish) plus the wire rules of
//! `peer_connection.cpp`: a scripted peer gets HAVE_NONE and two offered pieces instead of a
//! bitfield, its requests for other pieces are rejected, a HAVE for an offered piece brings the
//! next offer, and switching super seeding off sends HAVE_ALL. A real leech keeps its connection
//! through that switch and finishes.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, PeerFlags, PeerInfo, Session, SessionHandle, SessionParams, Settings,
    TorrentFlags, TorrentId, TorrentSource, TorrentState, TorrentStatus,
};

const PIECE: usize = 32 * 1024;

struct Fixture {
    torrent: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    info_hash: [u8; 20],
    total: u64,
    num_pieces: u32,
}

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d = std::env::temp_dir().join(format!(
        "lt-superseed-{name}-{}-{}",
        std::process::id(),
        h.finish() as u32
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fixture() -> Fixture {
    let sizes = [("alpha.bin", 700_000usize), ("sub/beta.bin", 900_000), ("sub/gamma.txt", 12_345)];
    let mut all = Vec::new();
    let mut files = Vec::new();
    for (i, (name, size)) in sizes.iter().enumerate() {
        let mut x = 0x2545_f491u32.wrapping_mul(i as u32 + 7);
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
    *info.key("name") = Entry::Bytes(b"superseed-test".to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let info_hash = Sha1::digest(&info.bencode()).0;
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    Fixture {
        torrent: t.bencode(),
        files,
        info_hash,
        total: all.len() as u64,
        num_pieces: all.len().div_ceil(PIECE) as u32,
    }
}

fn write_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let p = dir.join("superseed-test").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

fn assert_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let got =
            std::fs::read(dir.join("superseed-test").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(got == *data, "{name} differs");
    }
}

fn start() -> Session {
    let settings = Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        tick_interval: 100,
        aio_threads: 2,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        ..Settings::default()
    };
    Session::start(SessionParams { settings, state_dir: None }).expect("session starts")
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

fn add(w: &mut Watch, f: &Fixture, dir: &Path, flags: TorrentFlags, download_limit: u32) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = flags;
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

fn peer_info(w: &mut Watch, id: TorrentId) -> Vec<PeerInfo> {
    w.h.torrent(id).post_peer_info();
    wait(w, 5, "peer info", |a| match a {
        Alert::PeerInfo { id: i, peers } if *i == id => Some(peers.clone()),
        _ => None,
    })
}

struct Seed {
    session: Session,
    w: Watch,
    dir: PathBuf,
    port: u16,
    id: TorrentId,
}

fn super_seed(f: &Fixture) -> Seed {
    let dir = temp_dir("seed");
    write_files(f, &dir);
    let session = start();
    let mut w = Watch { h: session.handle(), backlog: VecDeque::new() };
    let port = listen_port(&mut w);
    let id = add(&mut w, f, &dir, TorrentFlags::SUPER_SEEDING, 0);
    let st = status(&mut w, id, "super seeding", |s| s.state == TorrentState::Seeding);
    assert!(st.super_seeding, "a checked seed super seeds");
    assert!(st.flags.contains(TorrentFlags::SUPER_SEEDING));
    Seed { session, w, dir, port, id }
}

struct Leech {
    session: Session,
    w: Watch,
    dir: PathBuf,
    port: u16,
    id: TorrentId,
    /// The first status after the check.
    checked: TorrentStatus,
}

fn leech(f: &Fixture, name: &str, flags: TorrentFlags, download_limit: u32) -> Leech {
    let dir = temp_dir(name);
    let session = start();
    let mut w = Watch { h: session.handle(), backlog: VecDeque::new() };
    let port = listen_port(&mut w);
    let id = add(&mut w, f, &dir, flags, download_limit);
    let checked = status(&mut w, id, "downloading", |s| s.state == TorrentState::Downloading);
    Leech { session, w, dir, port, id, checked }
}

fn finished(l: &mut Leech, secs: u64) {
    let id = l.id;
    wait(&mut l.w, secs, "finished", |a| {
        matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(())
    });
}

fn local(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[test]
fn two_leeches_finish_while_the_super_seed_uploads_about_one_copy() {
    let f = fixture();
    let mut seed = super_seed(&f);
    // super seeding is for seeds only: on a torrent that turns out incomplete the flag goes
    let mut a = leech(&f, "leech-a", TorrentFlags::SUPER_SEEDING, 0);
    let st = &a.checked;
    assert!(!st.super_seeding && !st.flags.contains(TorrentFlags::SUPER_SEEDING), "{:?}", st.flags);
    let mut b = leech(&f, "leech-b", TorrentFlags::empty(), 0);
    // and setting it on a downloading torrent does nothing
    b.w.h.torrent(b.id).set_flags(TorrentFlags::SUPER_SEEDING);
    a.w.h.torrent(a.id).connect_peer(local(seed.port));
    b.w.h.torrent(b.id).connect_peer(local(seed.port));
    a.w.h.torrent(a.id).connect_peer(local(b.port));
    finished(&mut a, 90);
    finished(&mut b, 90);
    assert_files(&f, &a.dir);
    assert_files(&f, &b.dir);
    let st = status(&mut b.w, b.id, "b seeding", |s| s.state == TorrentState::Seeding);
    assert!(!st.flags.contains(TorrentFlags::SUPER_SEEDING), "dropped on the downloading torrent");

    let st = status(&mut seed.w, seed.id, "seed status", |_| true);
    assert!(st.super_seeding);
    let copies = st.total_payload_upload as f64 / f.total as f64;
    // Every piece leaves the seed once, plus the end of the download, where both leeches
    // lack only pieces the other is still getting and the seed offers those too. A plain
    // seed hands each leech what it asks for (up to two copies here).
    assert!(copies >= 0.99, "the seed uploaded {copies:.2} copies: the leeches cannot have finished");
    assert!(copies <= 1.5, "the seed uploaded {copies:.2} copies");
    eprintln!("super seed uploaded {copies:.2} copies");
    for s in [seed.session, a.session, b.session] {
        s.shutdown();
    }
    for d in [seed.dir, a.dir, b.dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// A scripted peer connected to the seed: it sends its handshake (fast extension and BEP 10)
/// and reads the seed's.
fn scripted(f: &Fixture, port: u16) -> TcpStream {
    let mut c = TcpStream::connect(local(port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut hs = vec![19u8];
    hs.extend_from_slice(b"BitTorrent protocol");
    hs.extend_from_slice(&[0, 0, 0, 0, 0, 0x10, 0, 0x04]);
    hs.extend_from_slice(&f.info_hash);
    hs.extend_from_slice(b"-XX0001-superseedtst");
    c.write_all(&hs).unwrap();
    let mut reply = [0u8; 68];
    c.read_exact(&mut reply).unwrap();
    assert_eq!(&reply[28..48], &f.info_hash);
    assert!(reply[27] & 0x04 != 0, "the seed supports the fast extension");
    c
}

/// The next message other than a keep-alive.
fn next(c: &mut TcpStream) -> Vec<u8> {
    loop {
        let mut len = [0u8; 4];
        c.read_exact(&mut len).expect("connection open");
        let mut msg = vec![0u8; u32::from_be_bytes(len) as usize];
        c.read_exact(&mut msg).expect("connection open");
        if !msg.is_empty() {
            return msg;
        }
    }
}

fn field(msg: &[u8], i: usize) -> u32 {
    u32::from_be_bytes(msg[1 + 4 * i..5 + 4 * i].try_into().unwrap())
}

fn send(c: &mut TcpStream, id: u8, fields: &[u32]) {
    let mut out = (1 + 4 * fields.len() as u32).to_be_bytes().to_vec();
    out.push(id);
    for v in fields {
        out.extend_from_slice(&v.to_be_bytes());
    }
    c.write_all(&out).unwrap();
}

/// Reads up to the two offered HAVEs and returns them, checking what the super seed sends
/// instead of a bitfield.
fn offers(c: &mut TcpStream) -> [u32; 2] {
    let mut have_none = false;
    let mut have = Vec::new();
    let mut ext = false;
    while have.len() < 2 || !ext {
        let msg = next(c);
        match msg[0] {
            0x0f => have_none = true,
            4 => {
                assert!(have_none, "HAVE_NONE comes first");
                have.push(field(&msg, 0));
            }
            20 if msg[1] == 0 => {
                let doc = patorrent_core::bencode::bdecode(&msg[2..]).unwrap();
                let root = doc.root().unwrap();
                assert_eq!(root.dict_find_int_value("upload_only"), None, "a super seed is not upload-only");
                ext = true;
            }
            5 | 0x0e => panic!("a super seed sent its piece set ({})", msg[0]),
            0x11 => panic!("no allowed-fast set while super seeding"),
            _ => {}
        }
    }
    assert_ne!(have[0], have[1], "two different pieces");
    [have[0], have[1]]
}

/// Interested, then waits for the unchoke.
fn unchoked(c: &mut TcpStream) {
    send(c, 2, &[]);
    while next(c)[0] != 1 {}
}

/// Requests the first block of `piece` and returns the answer's message id (7 piece, 16 reject).
fn request(c: &mut TcpStream, f: &Fixture, piece: u32) -> u8 {
    // the last piece is shorter than a block
    let len = (f.total - piece as u64 * PIECE as u64).min(16 * 1024) as u32;
    send(c, 6, &[piece, 0, len]);
    loop {
        let msg = next(c);
        if matches!(msg[0], 7 | 16) {
            assert_eq!((field(&msg, 0), field(&msg, 1)), (piece, 0), "the answer is for our request");
            return msg[0];
        }
    }
}

#[test]
fn requests_for_pieces_not_offered_are_rejected_and_a_have_brings_the_next_offer() {
    let f = fixture();
    let mut seed = super_seed(&f);
    let mut c = scripted(&f, seed.port);
    let offered = offers(&mut c);
    unchoked(&mut c);
    let other = (0..f.num_pieces).find(|p| !offered.contains(p)).unwrap();
    assert_eq!(request(&mut c, &f, other), 16, "piece {other} was not offered");
    assert_eq!(request(&mut c, &f, offered[0]), 7);
    assert_eq!(request(&mut c, &f, offered[1]), 7);
    // Reporting an offered piece brings a new offer in its place.
    send(&mut c, 4, &[offered[0]]);
    let new = loop {
        let msg = next(&mut c);
        if msg[0] == 4 {
            break field(&msg, 0);
        }
    };
    assert!(!offered.contains(&new), "a new piece, not {offered:?}");
    assert_eq!(request(&mut c, &f, new), 7);
    assert_eq!(request(&mut c, &f, offered[1]), 7, "the older offer still stands");
    assert_eq!(request(&mut c, &f, offered[0]), 16, "the replaced offer is gone");
    let peers = peer_info(&mut seed.w, seed.id);
    assert_eq!(peers.len(), 1, "{peers:?}");
    seed.session.shutdown();
    let _ = std::fs::remove_dir_all(seed.dir);
}

#[test]
fn switching_super_seeding_off_sends_have_all() {
    let f = fixture();
    let mut seed = super_seed(&f);
    let mut c = scripted(&f, seed.port);
    let offered = offers(&mut c);
    unchoked(&mut c);
    let other = (0..f.num_pieces).find(|p| !offered.contains(p)).unwrap();
    assert_eq!(request(&mut c, &f, other), 16);
    seed.w.h.torrent(seed.id).unset_flags(TorrentFlags::SUPER_SEEDING);
    while next(&mut c)[0] != 0x0e {}
    assert_eq!(request(&mut c, &f, other), 7, "every piece can be requested now");
    let st = status(&mut seed.w, seed.id, "super seeding off", |s| !s.super_seeding);
    assert!(!st.flags.contains(TorrentFlags::SUPER_SEEDING));
    // A peer connecting now gets the full set.
    let mut d = scripted(&f, seed.port);
    loop {
        let msg = next(&mut d);
        assert!(!matches!(msg[0], 4 | 0x0f), "no offers once super seeding is off");
        if msg[0] == 0x0e {
            break;
        }
    }
    seed.session.shutdown();
    let _ = std::fs::remove_dir_all(seed.dir);
}

#[test]
fn a_leech_keeps_its_connection_when_super_seeding_stops_mid_download() {
    let f = fixture();
    let seed = super_seed(&f);
    // slow enough to still be downloading when super seeding stops
    let mut l = leech(&f, "leech", TorrentFlags::empty(), 256 * 1024);
    l.w.h.torrent(l.id).connect_peer(local(seed.port));
    let st = status(&mut l.w, l.id, "some progress", |s| s.total_done >= 4 * PIECE as u64);
    assert!(st.total_done < f.total, "finished before super seeding was switched off");
    let before = peer_info(&mut l.w, l.id);
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(!before[0].flags.contains(PeerFlags::SEED), "a super seed looks like a peer");
    seed.w.h.torrent(seed.id).unset_flags(TorrentFlags::SUPER_SEEDING);
    let deadline = Instant::now() + Duration::from_secs(10);
    let after = loop {
        let peers = peer_info(&mut l.w, l.id);
        if peers.first().is_some_and(|p| p.flags.contains(PeerFlags::SEED)) {
            break peers;
        }
        assert!(Instant::now() < deadline, "the seed never sent its piece set: {peers:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(
        after[0].total_download >= before[0].total_download,
        "the same connection: {} then {}",
        before[0].total_download,
        after[0].total_download
    );
    l.w.h.torrent(l.id).set_download_limit(0);
    finished(&mut l, 60);
    assert_files(&f, &l.dir);
    seed.session.shutdown();
    l.session.shutdown();
    let _ = std::fs::remove_dir_all(seed.dir);
    let _ = std::fs::remove_dir_all(l.dir);
}
