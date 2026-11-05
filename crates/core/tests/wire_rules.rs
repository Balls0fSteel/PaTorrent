//! Loopback tests for three wire rules of `engine/wire.rs` and `engine/mod.rs`, with a scripted
//! peer as in `tests/super_seed.rs` (bug hunt of 2026-11-05, findings 7 to 9): a peer kept
//! across a magnet's metadata fetch and file check gets our piece set once the check is done
//! (libtorrent `files_checked`); a request that refills an empty queue after an idle spell is
//! not snubbed on the next tick (the request timeout runs from the request, libtorrent
//! `m_requested`); and a choke keeps and serves the requests accepted under BEP 6 allowed-fast
//! (libtorrent `send_choke` keeps `m_accept_fast`). Finding 18: a block of the wrong length
//! disconnects the peer (libtorrent `verify_piece`).

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::bencode::{Entry, bdecode};
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, PeerFlags, PeerInfo, Session, SessionHandle, SessionParams, Settings,
    TorrentFlags, TorrentId, TorrentSource, TorrentState, TorrentStatus,
};

const PIECE: usize = 32 * 1024;
const BLOCK: u32 = 16 * 1024;

// message ids
const CHOKE: u8 = 0;
const UNCHOKE: u8 = 1;
const INTERESTED: u8 = 2;
const NOT_INTERESTED: u8 = 3;
const HAVE: u8 = 4;
const BITFIELD: u8 = 5;
const REQUEST: u8 = 6;
const PIECE_MSG: u8 = 7;
const CANCEL: u8 = 8;
const HAVE_ALL: u8 = 0x0e;
const HAVE_NONE: u8 = 0x0f;
const REJECT: u8 = 0x10;
const ALLOWED_FAST: u8 = 0x11;
const EXTENDED: u8 = 20;

struct Fixture {
    torrent: Vec<u8>,
    /// The bencoded info dictionary, as ut_metadata hands it out.
    info: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    /// Every file in order: the piece data.
    all: Vec<u8>,
    info_hash: [u8; 20],
    num_pieces: u32,
}

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d = std::env::temp_dir().join(format!("lt-wire-{name}-{}-{}", std::process::id(), h.finish() as u32));
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
    *info.key("name") = Entry::Bytes(b"wire-test".to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let info_bytes = info.bencode();
    let info_hash = Sha1::digest(&info_bytes).0;
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    let num_pieces = all.len().div_ceil(PIECE) as u32;
    Fixture { torrent: t.bencode(), info: info_bytes, files, all, info_hash, num_pieces }
}

fn write_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let p = dir.join("wire-test").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
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
        enable_incoming_utp: false,
        enable_outgoing_utp: false,
        ..Settings::default()
    };
    Session::start(SessionParams { settings, state_dir: None, ..Default::default() }).expect("session starts")
}

/// A session handle plus the alerts popped but not looked at yet (as in `tests/swarm.rs`).
struct Watch {
    h: SessionHandle,
    backlog: VecDeque<Alert>,
}

fn watch(session: &Session) -> Watch {
    Watch { h: session.handle(), backlog: VecDeque::new() }
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

fn added(w: &mut Watch) -> TorrentId {
    wait(w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}

fn add_file(w: &mut Watch, f: &Fixture, dir: &Path) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    w.h.add_torrent(p);
    added(w)
}

fn add_magnet(w: &mut Watch, f: &Fixture, dir: &Path) -> TorrentId {
    let hex: String = f.info_hash.iter().map(|x| format!("{x:02x}")).collect();
    let uri = format!("magnet:?xt=urn:btih:{hex}");
    let mut p = AddTorrentParams::new(TorrentSource::Magnet(uri), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    w.h.add_torrent(p);
    added(w)
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

fn local(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// Connects a scripted peer with the extension protocol and fast extension bits, and completes
/// the handshake.
fn connect(f: &Fixture, port: u16) -> TcpStream {
    let mut c = TcpStream::connect(local(port)).unwrap();
    let mut hs = vec![19u8];
    hs.extend_from_slice(b"BitTorrent protocol");
    hs.extend_from_slice(&[0, 0, 0, 0, 0, 0x10, 0, 0x04]);
    hs.extend_from_slice(&f.info_hash);
    hs.extend_from_slice(b"-XX0002-wire-rules-t");
    c.write_all(&hs).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut reply = [0u8; 68];
    c.read_exact(&mut reply).unwrap();
    assert_eq!(&reply[28..48], &f.info_hash);
    assert!(reply[27] & 0x04 != 0, "the engine supports the fast extension");
    c
}

/// The next message other than a keep-alive, or `None` once `deadline` has passed.
fn next_until(c: &mut TcpStream, deadline: Instant) -> Option<Vec<u8>> {
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        c.set_read_timeout(Some(left.max(Duration::from_millis(10)))).unwrap();
        let mut len = [0u8; 4];
        match c.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return None,
            Err(e) => panic!("connection closed: {e}"),
        }
        let mut msg = vec![0u8; u32::from_be_bytes(len) as usize];
        c.read_exact(&mut msg).expect("connection open");
        if !msg.is_empty() {
            return Some(msg);
        }
    }
}

/// The next message other than a keep-alive, within 20 s.
fn next(c: &mut TcpStream) -> Vec<u8> {
    next_until(c, Instant::now() + Duration::from_secs(20)).expect("a message within 20 s")
}

fn field(msg: &[u8], i: usize) -> u32 {
    u32::from_be_bytes(msg[1 + 4 * i..5 + 4 * i].try_into().unwrap())
}

fn message(id: u8, fields: &[u32]) -> Vec<u8> {
    let mut out = (1 + 4 * fields.len() as u32).to_be_bytes().to_vec();
    out.push(id);
    for v in fields {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out
}

fn send(c: &mut TcpStream, id: u8, fields: &[u32]) {
    c.write_all(&message(id, fields)).unwrap();
}

fn send_raw(c: &mut TcpStream, id: u8, payload: &[u8]) {
    let mut out = (1 + payload.len() as u32).to_be_bytes().to_vec();
    out.push(id);
    out.extend_from_slice(payload);
    c.write_all(&out).unwrap();
}

/// The data of `len` bytes at `start` of `piece`.
fn block(f: &Fixture, piece: u32, start: u32, len: u32) -> &[u8] {
    let at = piece as usize * PIECE + start as usize;
    &f.all[at..at + len as usize]
}

/// Bug hunt finding 7: a peer that handshook while the metadata was fetched (or while the files
/// were checked) never learnt our piece set, could not become interested and was never served.
/// libtorrent's `files_checked` re-initialises every connection: once the check is done the
/// peer gets HAVE_ALL (a complete torrent, a peer with the fast extension) and is served.
#[test]
fn a_peer_kept_across_a_magnets_check_gets_our_piece_set() {
    let f = fixture();
    let dir = temp_dir("magnet-check");
    // the files are complete: the check after the metadata finds every piece
    write_files(&f, &dir);
    let session = start();
    let mut w = watch(&session);
    let port = listen_port(&mut w);
    let id = add_magnet(&mut w, &f, &dir);
    status(&mut w, id, "fetching metadata", |s| s.state == TorrentState::DownloadingMetadata);

    let mut c = connect(&f, port);
    // Our extension handshake: ut_metadata under id 3, and the size of the info dictionary.
    let mut m = Entry::new_dict();
    *m.key("ut_metadata") = Entry::Int(3);
    let mut hs = Entry::new_dict();
    *hs.key("m") = m;
    *hs.key("metadata_size") = Entry::Int(f.info.len() as i64);
    send_raw(&mut c, EXTENDED, &[&[0u8][..], &hs.bencode()].concat());
    // The engine asks for the info dictionary under the id its own handshake gave ut_metadata;
    // the block is served. Then the check runs over the complete files, and our piece set has
    // to follow.
    let mut their_id = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    let piece_set = loop {
        let msg = next_until(&mut c, deadline).expect("our piece set after the check");
        match msg[0] {
            EXTENDED if msg[1] == 0 => {
                let doc = bdecode(&msg[2..]).unwrap();
                let root = doc.root().unwrap();
                let id = root.dict_find_dict("m").and_then(|m| m.dict_find_int_value("ut_metadata"));
                their_id = Some(id.expect("the engine offers ut_metadata") as u8);
            }
            EXTENDED if msg[1] == 3 => {
                let doc = bdecode(&msg[2..]).unwrap();
                let root = doc.root().unwrap();
                assert_eq!(root.dict_find_int_value("msg_type"), Some(0), "a metadata request");
                let piece = root.dict_find_int_value("piece").unwrap() as usize;
                let start = piece * 16 * 1024;
                let end = (start + 16 * 1024).min(f.info.len());
                let mut d = Entry::new_dict();
                *d.key("msg_type") = Entry::Int(1);
                *d.key("piece") = Entry::Int(piece as i64);
                *d.key("total_size") = Entry::Int(f.info.len() as i64);
                let mut payload = vec![their_id.expect("the engine's handshake comes first")];
                payload.extend_from_slice(&d.bencode());
                payload.extend_from_slice(&f.info[start..end]);
                send_raw(&mut c, EXTENDED, &payload);
            }
            HAVE_ALL | BITFIELD => break msg[0],
            // libtorrent sends no piece set before the check; an early HAVE_NONE would be true
            // but is not what this test is about
            HAVE_NONE => {}
            _ => {}
        }
    };
    assert_eq!(piece_set, HAVE_ALL, "a complete torrent sends HAVE_ALL to a peer with the fast extension");
    status(&mut w, id, "seeding", |s| s.state == TorrentState::Seeding);
    // Interested now, the peer is unchoked and served.
    send(&mut c, INTERESTED, &[]);
    while next(&mut c)[0] != UNCHOKE {}
    send(&mut c, REQUEST, &[0, 0, BLOCK]);
    loop {
        let msg = next(&mut c);
        assert_ne!(msg[0], REJECT, "the request is served");
        if msg[0] == PIECE_MSG {
            assert_eq!((field(&msg, 0), field(&msg, 1)), (0, 0));
            assert_eq!(&msg[9..], block(&f, 0, 0, BLOCK));
            break;
        }
    }
    drop(c);
    session.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// Bug hunt finding 8: the request timeout counted from the last block received, so a request
/// that refilled a queue empty for over 20 s was snubbed on the next tick (CANCELs, a one-block
/// window, "snubbed" in the status). It counts from that request now (libtorrent
/// `m_requested`).
#[test]
fn a_request_after_an_idle_spell_is_not_snubbed_at_once() {
    let f = fixture();
    let dir = temp_dir("idle-request");
    let session = start();
    let mut w = watch(&session);
    let port = listen_port(&mut w);
    let id = add_file(&mut w, &f, &dir);
    status(&mut w, id, "downloading", |s| s.state == TorrentState::Downloading);

    let mut c = connect(&f, port);
    // We have piece 0 only: the engine asks for its two blocks, gets them, and has nothing more
    // to ask us for.
    let mut bits = vec![0u8; (f.num_pieces as usize).div_ceil(8)];
    bits[0] = 0x80;
    send_raw(&mut c, BITFIELD, &bits);
    while next(&mut c)[0] != INTERESTED {}
    send(&mut c, UNCHOKE, &[]);
    loop {
        let msg = next(&mut c);
        match msg[0] {
            REQUEST => {
                let (piece, start, len) = (field(&msg, 0), field(&msg, 1), field(&msg, 2));
                assert_eq!(piece, 0, "only piece 0 is on offer");
                let mut payload = piece.to_be_bytes().to_vec();
                payload.extend_from_slice(&start.to_be_bytes());
                payload.extend_from_slice(block(&f, piece, start, len));
                send_raw(&mut c, PIECE_MSG, &payload);
            }
            NOT_INTERESTED => break,
            _ => {}
        }
    }
    // Idle past the 20 s piece timeout, then one more piece to offer: the engine asks for its
    // blocks and must not cancel them on the next tick.
    std::thread::sleep(Duration::from_secs(22));
    send(&mut c, HAVE, &[1]);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut requests = 0;
    while let Some(msg) = next_until(&mut c, deadline) {
        match msg[0] {
            REQUEST => {
                assert_eq!(field(&msg, 0), 1);
                requests += 1;
            }
            CANCEL => panic!("the fresh requests were cancelled: snubbed at once"),
            _ => {}
        }
    }
    assert_eq!(requests, 2, "both blocks of piece 1 were requested");
    let peers = peer_info(&mut w, id);
    assert_eq!(peers.len(), 1);
    assert!(!peers[0].flags.contains(PeerFlags::SNUBBED), "not snubbed");
    drop(c);
    session.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// Bug hunt finding 9: choking a peer dropped the requests accepted under BEP 6 allowed-fast
/// without a REJECT, so the peer waited for blocks that never came. libtorrent's `send_choke`
/// keeps `m_accept_fast` requests: the one for an allowed-fast piece is served through the
/// choke, the other one rejected.
#[test]
fn a_choke_keeps_the_allowed_fast_requests_it_accepted() {
    let f = fixture();
    let dir = temp_dir("choke-fast");
    write_files(&f, &dir);
    let session = start();
    let mut w = watch(&session);
    let port = listen_port(&mut w);
    let id = add_file(&mut w, &f, &dir);
    status(&mut w, id, "seeding", |s| s.state == TorrentState::Seeding);

    let mut c = connect(&f, port);
    // HAVE_ALL, the allowed-fast set, then the extension handshake.
    let mut fast = Vec::new();
    loop {
        let msg = next(&mut c);
        match msg[0] {
            ALLOWED_FAST => fast.push(field(&msg, 0)),
            EXTENDED => break,
            _ => {}
        }
    }
    // whole blocks: not the short last piece
    let last = f.num_pieces - 1;
    let fast_piece = fast.iter().copied().find(|&p| p != last).expect("an allowed-fast piece");
    let other = (0..last).find(|p| !fast.contains(p)).expect("a piece outside the set");
    send(&mut c, INTERESTED, &[]);
    while next(&mut c)[0] != UNCHOKE {}
    // Both requests and the NOT_INTERESTED that chokes us at once arrive in one read.
    let mut batch = message(REQUEST, &[fast_piece, 0, BLOCK]);
    batch.extend(message(REQUEST, &[other, 0, BLOCK]));
    batch.extend(message(NOT_INTERESTED, &[]));
    c.write_all(&batch).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut choked, mut served, mut rejected) = (false, false, false);
    while !(choked && served && rejected) {
        let msg = next_until(&mut c, deadline).unwrap_or_else(|| {
            panic!("choked {choked}, allowed-fast piece served {served}, the other rejected {rejected}")
        });
        match msg[0] {
            CHOKE => choked = true,
            PIECE_MSG => {
                assert_eq!((field(&msg, 0), field(&msg, 1)), (fast_piece, 0), "the allowed-fast request");
                assert_eq!(&msg[9..], block(&f, fast_piece, 0, BLOCK));
                served = true;
            }
            REJECT => {
                assert_eq!(field(&msg, 0), other, "the allowed-fast request is not rejected");
                rejected = true;
            }
            _ => {}
        }
    }
    drop(c);
    session.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// Bug hunt finding 18: a PIECE of the wrong length was counted redundant and dropped, but its
/// request stayed outstanding until the snub. libtorrent's `incoming_piece` disconnects on an
/// invalid piece size (`verify_piece`).
#[test]
fn a_block_of_the_wrong_length_disconnects_the_peer() {
    let f = fixture();
    let dir = temp_dir("bad-length");
    let session = start();
    let mut w = watch(&session);
    let port = listen_port(&mut w);
    let id = add_file(&mut w, &f, &dir);
    status(&mut w, id, "downloading", |s| s.state == TorrentState::Downloading);

    let mut c = connect(&f, port);
    let mut bits = vec![0u8; (f.num_pieces as usize).div_ceil(8)];
    bits[0] = 0x80;
    send_raw(&mut c, BITFIELD, &bits);
    while next(&mut c)[0] != INTERESTED {}
    send(&mut c, UNCHOKE, &[]);
    let req = loop {
        let msg = next(&mut c);
        if msg[0] == REQUEST {
            break msg;
        }
    };
    let (piece, start, len) = (field(&req, 0), field(&req, 1), field(&req, 2));
    // one byte short
    let mut payload = piece.to_be_bytes().to_vec();
    payload.extend_from_slice(&start.to_be_bytes());
    payload.extend_from_slice(block(&f, piece, start, len - 1));
    send_raw(&mut c, PIECE_MSG, &payload);
    // The engine closes the connection: reads end in EOF or a reset, not a timeout.
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = [0u8; 4096];
    let closed = loop {
        match c.read(&mut buf) {
            Ok(0) => break true,
            Ok(_) => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => break false,
            Err(_) => break true,
        }
    };
    assert!(closed, "a block of the wrong length disconnects the peer");
    drop(c);
    session.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
