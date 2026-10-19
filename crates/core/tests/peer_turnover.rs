//! Loopback tests for peer turnover (libtorrent `peer_turnover`, `peer_turnover_cutoff`,
//! `peer_turnover_interval`): a downloading session at its connection limit, with another
//! known peer waiting, drops the least useful peer every interval (never the one it is
//! interested in) and connects to the waiting one; with no peer waiting it keeps them all.
//! The peers are scripted: they answer the handshake, one of them with a full bitfield, and
//! then stay quiet.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, EncPolicy, Session, SessionHandle, SessionParams, Settings, TorrentFlags,
    TorrentId, TorrentSource,
};

const PIECE: usize = 32 * 1024;
const PIECES: usize = 7;

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d =
        std::env::temp_dir().join(format!("lt-turnover-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-file torrent of `PIECES` pieces (`tag` makes the info-hash differ between tests).
/// Returns the `.torrent` and its info-hash.
fn fixture(tag: &str) -> (Vec<u8>, [u8; 20]) {
    let data: Vec<u8> = (0..PIECE * PIECES).map(|i| (i * 7 % 251) as u8).collect();
    let pieces: Vec<u8> = data.chunks(PIECE).flat_map(|c| Sha1::digest(c).0).collect();
    let mut info = Entry::new_dict();
    *info.key("length") = Entry::Int(data.len() as i64);
    *info.key("name") = Entry::Bytes(b"turnover-test.bin".to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    *info.key("x-tag") = Entry::Bytes(tag.as_bytes().to_vec());
    let info_hash = Sha1::digest(&info.bencode()).0;
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    (t.bencode(), info_hash)
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Connected(usize),
    Closed(usize),
}

/// Peer `index`: accepts one connection, answers the handshake (plain, no extensions), sends a
/// full bitfield if `has_all`, then reads until the connection closes.
fn quiet_peer(index: usize, info_hash: [u8; 20], has_all: bool, events: mpsc::Sender<Event>) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        let Ok((mut s, _)) = l.accept() else { return };
        let mut hs = [0u8; 68];
        if s.read_exact(&mut hs).is_err() || hs[28..48] != info_hash {
            return;
        }
        let mut out = Vec::new();
        out.push(19);
        out.extend_from_slice(b"BitTorrent protocol");
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&info_hash);
        out.extend_from_slice(format!("-XX0000-{index:012}").as_bytes());
        if has_all {
            // bitfield: the seven pieces, the spare bit clear
            out.extend_from_slice(&2u32.to_be_bytes());
            out.extend_from_slice(&[5, 0xfe]);
        }
        if s.write_all(&out).is_err() {
            return;
        }
        let _ = events.send(Event::Connected(index));
        let mut buf = [0u8; 4096];
        while matches!(s.read(&mut buf), Ok(n) if n > 0) {}
        let _ = events.send(Event::Closed(index));
    });
    addr
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
        // the scripted peers speak plain TCP
        enable_outgoing_utp: false,
        enable_incoming_utp: false,
        out_enc_policy: EncPolicy::Disabled,
        in_enc_policy: EncPolicy::Disabled,
        connections_limit: 6,
        // the first turnover comes after an interval this short (libtorrent: after 90 s)
        peer_turnover_interval: 2,
        ..Settings::default()
    }
}

/// Takes alerts until `f` returns `Some` (as in `tests/swarm.rs`).
fn wait<T>(
    h: &SessionHandle,
    backlog: &mut VecDeque<Alert>,
    secs: u64,
    mut f: impl FnMut(&Alert) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut alerts = Vec::new();
    loop {
        while let Some(a) = backlog.pop_front() {
            if let Some(v) = f(&a) {
                return v;
            }
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(30));
        h.pop_alerts(&mut alerts);
        backlog.extend(alerts.drain(..));
    }
}

/// A downloading session with the torrent added; the peers are not connected yet.
fn leech(tag: &str) -> (Session, TorrentId, [u8; 20], PathBuf) {
    let (torrent, info_hash) = fixture(tag);
    let dir = temp_dir(tag);
    let s = Session::start(SessionParams { settings: settings(), state_dir: None }).expect("session starts");
    let h = s.handle();
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(torrent), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    h.add_torrent(p);
    let mut backlog = VecDeque::new();
    let id = wait(&h, &mut backlog, 10, |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    });
    // checked (nothing on disk) and downloading before the peers come
    wait(&h, &mut backlog, 10, |a| matches!(a, Alert::TorrentChecked { id: i } if *i == id).then_some(()));
    (s, id, info_hash, dir)
}

/// Connects the first `n` peers one by one, so they are connected in order.
fn connect(h: &SessionHandle, id: TorrentId, peers: &[SocketAddr], events: &mpsc::Receiver<Event>, n: usize) {
    for (i, &a) in peers.iter().enumerate().take(n) {
        h.torrent(id).connect_peer(a);
        assert_eq!(events.recv_timeout(Duration::from_secs(10)), Ok(Event::Connected(i)));
    }
}

#[test]
fn at_the_connection_limit_the_least_useful_peer_makes_room_for_a_waiting_one() {
    let (s, id, info_hash, dir) = leech("turnover");
    let h = s.handle();
    let (tx, events) = mpsc::channel();
    // peer 0 has every piece: we are interested in it, so it is the last to go
    let peers: Vec<SocketAddr> = (0..7).map(|i| quiet_peer(i, info_hash, i == 0, tx.clone())).collect();
    connect(&h, id, &peers, &events, 6);
    // at the limit: the seventh is known, but not connected
    h.torrent(id).connect_peer(peers[6]);
    assert!(events.recv_timeout(Duration::from_millis(500)).is_err(), "no room for the seventh peer yet");

    // within a turnover interval or two one of the quiet peers goes, and the seventh comes in
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut closed, mut seventh) = (Vec::new(), false);
    while closed.is_empty() || !seventh {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(Event::Closed(i)) => closed.push(i),
            Ok(Event::Connected(6)) => seventh = true,
            Ok(e) => panic!("unexpected {e:?}"),
            Err(_) => panic!("timed out: closed {closed:?}, seventh connected: {seventh}"),
        }
    }
    assert!(!closed.contains(&0), "the peer we want pieces from stays: closed {closed:?}");
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_a_peer_to_connect_instead_nobody_is_dropped() {
    let (s, id, info_hash, dir) = leech("no-turnover");
    let h = s.handle();
    let (tx, events) = mpsc::channel();
    let peers: Vec<SocketAddr> = (0..6).map(|i| quiet_peer(i, info_hash, false, tx.clone())).collect();
    connect(&h, id, &peers, &events, 6);
    // two turnover intervals pass
    match events.recv_timeout(Duration::from_secs(5)) {
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        other => panic!("a peer was dropped: {other:?}"),
    }
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
