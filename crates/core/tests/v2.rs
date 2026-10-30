//! Loopback tests for BitTorrent v2 (BEP 52): v2-only and hybrid torrents made by
//! `create_torrent` are checked against their piece layers, seeded and downloaded (a v2-only
//! torrent's block requests ending at the end of each file's data); a v2 torrent
//! keeps its piece layers in the session store across a restart and passes a recheck there, and
//! a corrupted piece fails it; a hybrid whose v1 and v2 hashes describe different data stops with
//! libtorrent's "inconsistent hashes" error; a v2-only torrent without piece layers is refused.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::bencode::{Entry, bdecode};
use patorrent_core::metainfo::{
    CreateFlags, CreateTorrent, FileStorage, TorrentInfo, add_files, set_piece_hashes,
};
use patorrent_core::{
    AddTorrentParams, Alert, Session, SessionHandle, SessionParams, Settings, TorrentFlags, TorrentId,
    TorrentSource, TorrentState, TorrentStatus,
};

const PIECE: u32 = 32 * 1024;

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d = std::env::temp_dir().join(format!("lt-v2-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Files of several sizes: a multi-piece file, one that ends mid-block, one smaller than a
/// block and an empty one.
fn files(seed: u32) -> Vec<(String, Vec<u8>)> {
    let sizes =
        [("alpha.bin", 300_000usize), ("sub/beta.bin", 1_000_000), ("sub/gamma.txt", 12_345), ("empty", 0)];
    sizes
        .iter()
        .enumerate()
        .map(|(i, (name, size))| {
            let mut x = 0x9e37_79b9u32.wrapping_mul(i as u32 + seed);
            let data = (0..*size)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            (name.to_string(), data)
        })
        .collect()
}

fn write_files(files: &[(String, Vec<u8>)], dir: &Path) {
    for (name, data) in files {
        let p = dir.join("v2-test").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

fn assert_files(files: &[(String, Vec<u8>)], dir: &Path) {
    for (name, data) in files {
        let got = std::fs::read(dir.join("v2-test").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(got == *data, "{name} differs");
    }
}

/// A torrent for the files in `dir/v2-test`, made the way `patorrent-nox create` does.
fn make_torrent(dir: &Path, flags: CreateFlags) -> Vec<u8> {
    let mut fs = FileStorage::new();
    add_files(&mut fs, &dir.join("v2-test"), &mut |_| true, CreateFlags::empty()).unwrap();
    let mut t = CreateTorrent::new(fs, PIECE, flags).unwrap();
    set_piece_hashes(&mut t, dir, &mut |_| true).unwrap();
    t.generate_buf().unwrap()
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

fn start(state_dir: Option<PathBuf>) -> Session {
    Session::start(SessionParams { settings: settings(), state_dir }).expect("session starts")
}

struct Watch {
    h: SessionHandle,
    backlog: VecDeque<Alert>,
}

impl Watch {
    fn new(h: SessionHandle) -> Watch {
        Watch { h, backlog: VecDeque::new() }
    }
}

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

fn listen_port(w: &mut Watch) -> u16 {
    wait(w, 5, "listen", |a| match a {
        Alert::ListenSucceeded { address, .. } => Some(address.port()),
        _ => None,
    })
}

fn add(w: &mut Watch, torrent: &[u8], dir: &Path) -> Result<TorrentId, String> {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(torrent.to_vec()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    w.h.add_torrent(p);
    wait(w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(Ok(*id)),
        Alert::AddTorrentFailed { error, .. } => Some(Err(error.to_string())),
        _ => None,
    })
}

fn status_of(a: &Alert, id: TorrentId) -> Option<TorrentStatus> {
    match a {
        Alert::StateUpdate(v) => v.iter().find(|s| s.id == id).cloned(),
        _ => None,
    }
}

fn wait_state(w: &mut Watch, id: TorrentId, state: TorrentState, secs: u64) -> TorrentStatus {
    wait(w, secs, &format!("{state:?}"), |a| {
        status_of(a, id).filter(|s| s.state == state && s.error.is_empty())
    })
}

/// Seeds `torrent` from `seed_dir` (checking the files against it first) and downloads it into
/// a new directory over loopback.
fn seed_and_download(name: &str, torrent: &[u8], files: &[(String, Vec<u8>)], seed_dir: &Path) {
    let seed = start(None);
    let mut sw = Watch::new(seed.handle());
    let port = listen_port(&mut sw);
    let sid = add(&mut sw, torrent, seed_dir).unwrap();
    let st = wait_state(&mut sw, sid, TorrentState::Seeding, 20);
    assert_eq!(st.total_done, st.total, "{name}: the seed's check passes every piece");

    let dir = temp_dir(&format!("{name}-leech"));
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, torrent, &dir).unwrap();
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert_eq!(st.total_wanted_done, st.total_wanted, "{name}");
    assert_eq!(st.total_failed_bytes, 0, "{name}: no piece failed its check");
    // blocks wholly in pad files are not downloaded (libtorrent's pad blocks), and a v2-only
    // torrent's requests end at the end of each file (libtorrent 2.0.13 `piece_size_for_req`):
    // no pad byte is downloaded at all
    let ti = TorrentInfo::from_bytes(torrent).unwrap();
    let pads = ti.files().pad_bytes_per_piece();
    let pad_blocks: u64 = pads.iter().map(|&(_, b)| (b / 0x4000 * 0x4000) as u64).sum();
    assert!(pad_blocks > 0, "{name}: the layout has pad blocks");
    let not_downloaded = if ti.v1() { pad_blocks } else { pads.iter().map(|&(_, b)| b as u64).sum() };
    assert_eq!(st.total_payload_download, st.total - not_downloaded, "{name}: payload without the padding");
    leech.shutdown();
    seed.shutdown();
    assert_files(files, &dir);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn v2_only_and_hybrid_torrents_seed_and_download() {
    let files = files(1);
    let seed_dir = temp_dir("seed");
    write_files(&files, &seed_dir);
    for (name, flags) in [("v2", CreateFlags::V2_ONLY), ("hybrid", CreateFlags::empty())] {
        let torrent = make_torrent(&seed_dir, flags);
        let ti = TorrentInfo::from_bytes(&torrent).unwrap();
        assert!(ti.v2());
        assert_eq!(ti.v1(), flags.is_empty(), "{name}");
        seed_and_download(name, &torrent, &files, &seed_dir);
    }
    let _ = std::fs::remove_dir_all(seed_dir);
}

/// Forwards one connection to `port` and records the `request` messages (piece, start, length)
/// the connecting side sends. Plaintext only.
type Requests = std::sync::Arc<std::sync::Mutex<Vec<(u32, u32, u32)>>>;

fn request_recorder(port: u16) -> (u16, Requests) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let relay_port = listener.local_addr().unwrap().port();
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = requests.clone();
    std::thread::spawn(move || {
        let Ok((mut down, _)) = listener.accept() else { return };
        let mut up = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (mut down_r, mut up_w) = (down.try_clone().unwrap(), up.try_clone().unwrap());
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut up, &mut down);
        });
        let mut hs = [0u8; 68];
        if down_r.read_exact(&mut hs).is_err() || up_w.write_all(&hs).is_err() {
            return;
        }
        assert_eq!(hs[0], 19, "a plaintext handshake");
        loop {
            let mut len = [0u8; 4];
            if down_r.read_exact(&mut len).is_err() {
                return;
            }
            let mut m = vec![0u8; u32::from_be_bytes(len) as usize];
            if down_r.read_exact(&mut m).is_err() {
                return;
            }
            if m.len() == 13 && m[0] == 6 {
                let int = |i: usize| u32::from_be_bytes(m[i..i + 4].try_into().unwrap());
                log.lock().unwrap().push((int(1), int(5), int(9)));
            }
            if up_w.write_all(&len).and_then(|_| up_w.write_all(&m)).is_err() {
                return;
            }
        }
    });
    (relay_port, requests)
}

/// A v2-only torrent asks for the data of each file and not the padding after it (libtorrent
/// 2.0.13 `piece_size_for_req`): the block at a file's end is requested exactly as long as the
/// data left in it, and a hybrid asks for whole blocks (the SHA-1 piece hash covers the pads).
#[test]
fn file_tail_requests_are_sized_by_the_v2_piece_size() {
    let files = files(7);
    let seed_dir = temp_dir("tail-seed");
    write_files(&files, &seed_dir);
    for (name, flags) in [("v2", CreateFlags::V2_ONLY), ("hybrid", CreateFlags::empty())] {
        let torrent = make_torrent(&seed_dir, flags);
        let ti = TorrentInfo::from_bytes(&torrent).unwrap();
        let seed = start(None);
        let mut sw = Watch::new(seed.handle());
        let port = listen_port(&mut sw);
        let sid = add(&mut sw, &torrent, &seed_dir).unwrap();
        wait_state(&mut sw, sid, TorrentState::Seeding, 20);

        let (relay, requests) = request_recorder(port);
        let dir = temp_dir(&format!("tail-{name}-leech"));
        let leech = Session::start(SessionParams {
            settings: Settings { out_enc_policy: patorrent_core::EncPolicy::Disabled, ..settings() },
            state_dir: None,
        })
        .unwrap();
        let mut w = Watch::new(leech.handle());
        let id = add(&mut w, &torrent, &dir).unwrap();
        wait_state(&mut w, id, TorrentState::Downloading, 10);
        leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], relay)));
        wait(&mut w, 60, "finished", |a| {
            matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(())
        });
        leech.shutdown();
        seed.shutdown();
        assert_files(&files, &dir);
        let _ = std::fs::remove_dir_all(dir);

        let fs = ti.files();
        let requests = requests.lock().unwrap();
        assert!(!requests.is_empty(), "{name}");
        let mut file_tails = 0;
        for &(piece, start, length) in requests.iter() {
            let size = if ti.v1() { fs.piece_size(piece) } else { fs.piece_size2(piece) };
            assert!(start < size, "{name}: ({piece}, {start}, {length}) past the data");
            assert_eq!(length, (size - start).min(0x4000), "{name}: ({piece}, {start}, {length})");
            if length < 0x4000 && piece + 1 < fs.num_pieces() {
                file_tails += 1;
            }
        }
        // alpha.bin, beta.bin and gamma.txt end mid-block; the last of them ends the torrent
        assert_eq!(file_tails, if ti.v1() { 0 } else { 2 }, "{name}: {requests:?}");
    }
    let _ = std::fs::remove_dir_all(seed_dir);
}

/// The piece layers are not part of the info dictionary: the store keeps them, so a restored
/// v2 torrent still verifies its pieces. A changed byte fails its piece.
#[test]
fn a_v2_torrent_keeps_its_piece_layers_across_a_restart() {
    let files = files(2);
    let dir = temp_dir("restart");
    let state = temp_dir("restart-state");
    write_files(&files, &dir);
    let torrent = make_torrent(&dir, CreateFlags::V2_ONLY);
    let ti = TorrentInfo::from_bytes(&torrent).unwrap();
    let id = {
        let s = start(Some(state.clone()));
        let mut w = Watch::new(s.handle());
        let id = add(&mut w, &torrent, &dir).unwrap();
        wait_state(&mut w, id, TorrentState::Seeding, 20);
        s.shutdown();
        id
    };

    // flip one byte in the middle of beta.bin
    let beta = dir.join("v2-test/sub/beta.bin");
    let mut data = std::fs::read(&beta).unwrap();
    data[500_000] ^= 0xff;
    std::fs::write(&beta, &data).unwrap();

    let s = start(Some(state.clone()));
    let mut w = Watch::new(s.handle());
    let restored = first_restored(&mut w).id;
    let _ = id;
    wait_state(&mut w, restored, TorrentState::Seeding, 20);
    s.handle().torrent(restored).force_recheck();
    let st = wait(&mut w, 30, "recheck", |a| {
        status_of(a, restored).filter(|s| s.state == TorrentState::Downloading && s.error.is_empty())
    });
    // exactly the piece holding the changed byte is missing (without the piece layers from
    // the store, no v2 piece would pass)
    assert_eq!(st.num_pieces, ti.num_pieces() - 1);
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(state);
}

/// A hybrid whose v1 piece hashes describe the files but whose v2 hashes describe other data:
/// the check stops with libtorrent's "inconsistent hashes" error.
#[test]
fn a_hybrid_with_inconsistent_hashes_stops_with_an_error() {
    let ours = files(3);
    let theirs = files(4);
    let dir = temp_dir("inconsistent");
    let other = temp_dir("inconsistent-other");
    write_files(&ours, &dir);
    write_files(&theirs, &other);
    let v1 = TorrentInfo::from_bytes(&make_torrent(&dir, CreateFlags::empty())).unwrap();
    let other_torrent = make_torrent(&other, CreateFlags::empty());
    let v2 = TorrentInfo::from_bytes(&other_torrent).unwrap();
    // the other files' file tree and piece layers with our v1 piece hashes
    let torrent = with_info(&other_torrent, mixed_info(&v2, &v1));
    let ti = TorrentInfo::from_bytes(&torrent).unwrap();
    assert!(ti.v1() && ti.v2_piece_layers_complete());

    let s = start(None);
    let mut w = Watch::new(s.handle());
    let id = add(&mut w, &torrent, &dir).unwrap();
    let error = wait(&mut w, 20, "inconsistent hashes", |a| match a {
        Alert::TorrentError { id: i, error } if *i == id => Some(error.to_string()),
        _ => None,
    });
    assert_eq!(error, "v1 and v2 hashes do not describe the same data");
    let st = wait(&mut w, 5, "error status", |a| status_of(a, id).filter(|s| !s.error.is_empty()));
    assert!(st.error.contains("v1 and v2 hashes do not describe the same data"), "{}", st.error);
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(other);
}

/// `with`'s info dictionary with `from`'s v1 "pieces".
fn mixed_info(with: &TorrentInfo, from: &TorrentInfo) -> Vec<u8> {
    let doc = bdecode(with.info_section()).unwrap();
    let mut info = Entry::from_node(doc.root().unwrap());
    let theirs = bdecode(from.info_section()).unwrap();
    let pieces = theirs.root().unwrap().dict_find_string_value("pieces").unwrap().to_vec();
    *info.key("pieces") = Entry::Bytes(pieces);
    info.bencode()
}

/// `torrent` with its info dictionary replaced.
fn with_info(torrent: &[u8], info: Vec<u8>) -> Vec<u8> {
    let doc = bdecode(torrent).unwrap();
    let mut t = Entry::from_node(doc.root().unwrap());
    *t.key("info") = Entry::Preformatted(info);
    t.bencode()
}

/// A seed for `torrent` with the files in a new directory; returns the session, its directory
/// and its listen port.
fn seed(name: &str, torrent: &[u8], files: &[(String, Vec<u8>)]) -> (Session, PathBuf, u16) {
    let dir = temp_dir(name);
    write_files(files, &dir);
    let s = start(None);
    let mut w = Watch::new(s.handle());
    let port = listen_port(&mut w);
    let id = add(&mut w, torrent, &dir).unwrap();
    wait_state(&mut w, id, TorrentState::Seeding, 20);
    (s, dir, port)
}

/// Adds `source` to a leech with `state` (or none), connects it to the seed on `port`, and
/// waits for the download to finish.
fn leech(
    name: &str,
    source: TorrentSource,
    port: u16,
    state: Option<PathBuf>,
) -> (Session, PathBuf, TorrentId) {
    let dir = temp_dir(name);
    let s = start(state);
    let mut w = Watch::new(s.handle());
    let mut p = AddTorrentParams::new(source, dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    w.h.add_torrent(p);
    let id = wait(&mut w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    });
    s.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert_eq!(st.total_failed_bytes, 0, "{name}: no piece failed its check");
    (s, dir, id)
}

/// A v2-only `.torrent` without its piece layers asks the seed for them (hash requests answered
/// from the seed's piece layers), then downloads as usual.
#[test]
fn a_v2_only_torrent_without_piece_layers_fetches_them_from_a_seed() {
    let files = files(5);
    let src = temp_dir("no-layers-src");
    write_files(&files, &src);
    let torrent = make_torrent(&src, CreateFlags::V2_ONLY);
    let doc = bdecode(&torrent).unwrap();
    let mut t = Entry::from_node(doc.root().unwrap());
    t.dict_mut().remove(b"piece layers".as_slice());
    let stripped = t.bencode();
    assert!(!TorrentInfo::from_bytes(&stripped).unwrap().v2_piece_layers_complete());

    let (seeder, seed_dir, port) = seed("no-layers-seed", &torrent, &files);
    let (s, dir, _) = leech("no-layers-leech", TorrentSource::TorrentFile(stripped), port, None);
    s.shutdown();
    seeder.shutdown();
    assert_files(&files, &dir);
    for d in [src, seed_dir, dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// A v2 magnet (`btmh` only): the metadata is checked against the SHA-256 info-hash, the piece
/// layers come from the seed, the download follows, and the store keeps the layers so a
/// restarted session passes a recheck.
#[test]
fn a_v2_magnet_fetches_metadata_and_piece_layers_then_downloads() {
    let files = files(7);
    let src = temp_dir("magnet-src");
    write_files(&files, &src);
    let torrent = make_torrent(&src, CreateFlags::V2_ONLY);
    let ti = TorrentInfo::from_bytes(&torrent).unwrap();
    let v2 = ti.info_hashes().v2.unwrap();
    let magnet = format!("magnet:?xt=urn:btmh:1220{}&dn=v2-test", v2.to_hex());

    let (seeder, seed_dir, port) = seed("magnet-seed", &torrent, &files);
    let state = temp_dir("magnet-state");
    let (s, dir, _) = leech("magnet-leech", TorrentSource::Magnet(magnet), port, Some(state.clone()));
    s.shutdown();
    seeder.shutdown();
    assert_files(&files, &dir);

    // restart from the store, without the seed: the recheck needs the stored piece layers
    let s = start(Some(state.clone()));
    let mut w = Watch::new(s.handle());
    let id = first_restored(&mut w).id;
    wait_state(&mut w, id, TorrentState::Seeding, 20);
    s.handle().torrent(id).force_recheck();
    let st = wait(&mut w, 30, "recheck", |a| {
        status_of(a, id)
            .filter(|s| s.state == TorrentState::Seeding && s.error.is_empty() && s.num_pieces > 0)
    });
    assert_eq!(st.num_pieces, ti.num_pieces());
    s.shutdown();
    for d in [src, seed_dir, dir, state] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// A `btmh`-only magnet of a hybrid learns the v1 hash from the metadata (and is stored under
/// it from then on, so it is found again after a restart); the v1 hashes verify the pieces.
#[test]
fn a_v2_magnet_of_a_hybrid_learns_its_v1_hash() {
    let files = files(8);
    let src = temp_dir("hybrid-magnet-src");
    write_files(&files, &src);
    let torrent = make_torrent(&src, CreateFlags::empty());
    let ti = TorrentInfo::from_bytes(&torrent).unwrap();
    let magnet = format!("magnet:?xt=urn:btmh:1220{}", ti.info_hashes().v2.unwrap().to_hex());

    let (seeder, seed_dir, port) = seed("hybrid-magnet-seed", &torrent, &files);
    let state = temp_dir("hybrid-magnet-state");
    let (s, dir, id) = leech("hybrid-magnet-leech", TorrentSource::Magnet(magnet), port, Some(state.clone()));
    let mut w = Watch::new(s.handle());
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert_eq!(st.info_hashes, ti.info_hashes(), "the v1 hash is known now");
    s.shutdown();
    seeder.shutdown();
    assert_files(&files, &dir);

    let s = start(Some(state.clone()));
    let mut w = Watch::new(s.handle());
    let id = {
        let st = first_restored(&mut w);
        (st.id, st.info_hashes)
    };
    assert_eq!(id.1, ti.info_hashes());
    wait_state(&mut w, id.0, TorrentState::Seeding, 20);
    s.shutdown();
    for d in [src, seed_dir, dir, state] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// A scripted peer on a raw TCP connection.
struct RawPeer(std::net::TcpStream);

impl RawPeer {
    /// Connects and sends a handshake with `info_hash`, offering v2 when `v2_bit`.
    fn connect(port: u16, info_hash: &[u8], v2_bit: bool) -> RawPeer {
        use std::io::Write;
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut hs = vec![19u8];
        hs.extend_from_slice(b"BitTorrent protocol");
        let mut reserved = [0u8; 8];
        reserved[5] |= 0x10;
        reserved[7] |= 0x04;
        if v2_bit {
            reserved[7] |= 0x10;
        }
        hs.extend_from_slice(&reserved);
        hs.extend_from_slice(info_hash);
        hs.extend_from_slice(b"-XX0000-scriptedpeer");
        s.write_all(&hs).unwrap();
        RawPeer(s)
    }

    /// The other side's handshake: reserved bits and info-hash.
    fn handshake(&mut self) -> ([u8; 8], [u8; 20]) {
        use std::io::Read;
        let mut buf = [0u8; 68];
        self.0.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[1..20], b"BitTorrent protocol");
        (buf[20..28].try_into().unwrap(), buf[28..48].try_into().unwrap())
    }

    fn send(&mut self, id: u8, payload: &[u8]) {
        use std::io::Write;
        let mut m = ((payload.len() + 1) as u32).to_be_bytes().to_vec();
        m.push(id);
        m.extend_from_slice(payload);
        self.0.write_all(&m).unwrap();
    }

    /// The next message with one of `ids`, skipping the others; `None` once the connection is
    /// closed.
    fn next(&mut self, ids: &[u8]) -> Option<(u8, Vec<u8>)> {
        use std::io::Read;
        loop {
            let mut len = [0u8; 4];
            self.0.read_exact(&mut len).ok()?;
            let len = u32::from_be_bytes(len) as usize;
            let mut m = vec![0u8; len];
            self.0.read_exact(&mut m).ok()?;
            if len > 0 && ids.contains(&m[0]) {
                return Some((m[0], m[1..].to_vec()));
            }
        }
    }
}

fn hash_msg(root: &[u8], base: i32, index: i32, count: i32, proof_layers: i32) -> Vec<u8> {
    let mut m = root.to_vec();
    for v in [base, index, count, proof_layers] {
        m.extend_from_slice(&v.to_be_bytes());
    }
    m
}

fn sha256_pair(l: &[u8], r: &[u8]) -> [u8; 32] {
    use patorrent_core::crypto::Sha256;
    let mut h = Sha256::new();
    h.update(l).update(r);
    h.finalize().0
}

/// A v1 connection to a hybrid that offers the v2 upgrade gets the v2 hash back (libtorrent's
/// upgrade), then answers hash requests from the piece layers with proofs, and rejects requests
/// for block hashes, which it does not keep. A hash request on a plain v1 connection ends it.
#[test]
fn a_v2_peer_gets_piece_layer_hashes_and_a_v1_peer_is_dropped_for_asking() {
    let files = files(6);
    let dir = temp_dir("hash-requests");
    write_files(&files, &dir);
    let torrent = make_torrent(&dir, CreateFlags::empty());
    let ti = TorrentInfo::from_bytes(&torrent).unwrap();
    let ih = ti.info_hashes();
    let (v1, v2) = (ih.v1.unwrap(), ih.truncated_v2().unwrap());

    let s = start(None);
    let mut w = Watch::new(s.handle());
    let port = listen_port(&mut w);
    let id = add(&mut w, &torrent, &dir).unwrap();
    wait_state(&mut w, id, TorrentState::Seeding, 20);

    let fs = ti.files();
    let beta = (0..fs.num_files()).find(|&i| fs.file_name(i) == "beta.bin").unwrap();
    let root = fs.root(beta).0;
    let layer = ti.piece_layer(beta).to_vec();
    let pieces = layer.len() / 32;
    assert_eq!(pieces, 31);

    // v1 hash with the v2 bit: upgraded
    let mut peer = RawPeer::connect(port, &v1.0, true);
    let (reserved, got) = peer.handshake();
    assert_eq!(got, v2.0, "the answer carries the truncated v2 hash");
    assert_eq!(reserved[7] & 0x10, 0, "no upgrade offer on a v2 connection");

    // the whole piece layer (2 blocks per piece: layer 1), padded to 32 with the pad hash
    peer.send(21, &hash_msg(&root, 1, 0, 32, 0));
    let (id_, reply) = peer.next(&[22, 23]).expect("an answer");
    assert_eq!(id_, 22, "hashes");
    assert_eq!(&reply[..48], &hash_msg(&root, 1, 0, 32, 0)[..]);
    let hashes = &reply[48..];
    assert_eq!(hashes.len(), 32 * 32);
    assert_eq!(&hashes[..layer.len()], &layer[..]);
    assert_eq!(&hashes[layer.len()..], &sha256_pair(&[0; 32], &[0; 32])[..]);

    // four piece hashes with proofs up to the root: 2 hashes for the subtree, 3 uncles
    peer.send(21, &hash_msg(&root, 1, 0, 4, 4));
    let (id_, reply) = peer.next(&[22, 23]).expect("an answer");
    assert_eq!(id_, 22);
    let hashes: Vec<&[u8]> = reply[48..].chunks(32).collect();
    assert_eq!(hashes.len(), 4 + 3);
    let mut node = sha256_pair(&sha256_pair(hashes[0], hashes[1]), &sha256_pair(hashes[2], hashes[3]));
    for uncle in &hashes[4..] {
        node = sha256_pair(&node, uncle);
    }
    assert_eq!(node, root, "the proof leads to the file's root");

    // block hashes are not kept: rejected
    peer.send(21, &hash_msg(&root, 0, 0, 2, 0));
    let (id_, reply) = peer.next(&[22, 23]).expect("an answer");
    assert_eq!((id_, &reply[..]), (23, &hash_msg(&root, 0, 0, 2, 0)[..]));
    // an invalid range: rejected too
    peer.send(21, &hash_msg(&root, 1, 30, 4, 0));
    assert_eq!(peer.next(&[22, 23]).expect("an answer").0, 23);

    // a plain v1 connection is offered the upgrade, and dropped for a hash request
    let mut v1_peer = RawPeer::connect(port, &v1.0, false);
    let (reserved, got) = v1_peer.handshake();
    assert_eq!(got, v1.0);
    assert_eq!(reserved[7] & 0x10, 0x10, "the hybrid offers the v2 upgrade");
    v1_peer.send(21, &hash_msg(&root, 1, 0, 32, 0));
    assert_eq!(v1_peer.next(&[22, 23]), None, "closed");

    // the truncated v2 hash finds the torrent directly
    let mut v2_peer = RawPeer::connect(port, &v2.0, false);
    assert_eq!(v2_peer.handshake().1, v2.0);
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
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

type Swarms = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<Vec<u8>, Vec<u16>>>>;
type Announces = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<Vec<u8>, usize>>>;

/// An HTTP tracker with one swarm per info-hash, as real trackers keep them: each announcer gets
/// the other ports of its own hash on 127.0.0.1. Also counts the announces per info-hash. Runs
/// until the test process ends.
fn swarm_tracker() -> (String, Swarms, Announces) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/announce", l.local_addr().unwrap());
    let swarms: Swarms = Default::default();
    let announces: Announces = Default::default();
    let shared = swarms.clone();
    let counts = announces.clone();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for conn in l.incoming() {
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
            let param = |name: &str| {
                req.split(&format!("{name}="))
                    .nth(1)
                    .and_then(|r| r.split(['&', ' ']).next())
                    .unwrap_or_default()
            };
            let ih = percent_decode(param("info_hash"));
            let port: u16 = param("port").parse().unwrap_or(0);
            *counts.lock().unwrap().entry(ih.clone()).or_default() += 1;
            let mut peers = Vec::new();
            {
                let mut s = shared.lock().unwrap();
                let swarm = s.entry(ih).or_default();
                for &p in swarm.iter().filter(|&&p| p != port) {
                    peers.extend_from_slice(&[127, 0, 0, 1]);
                    peers.extend_from_slice(&p.to_be_bytes());
                }
                if !swarm.contains(&port) {
                    swarm.push(port);
                }
            }
            let mut body = format!("d8:intervali1800e5:peers{}:", peers.len()).into_bytes();
            body.extend_from_slice(&peers);
            body.push(b'e');
            let head =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = c.write_all(head.as_bytes());
            let _ = c.write_all(&body);
        }
    });
    (url, swarms, announces)
}

/// A hybrid announces under both of its hashes, each with its own tracker state, so a peer that
/// knows only the v2 hash (a `btmh` magnet) finds the seed through the tracker, connects with
/// the v2 hash and downloads.
#[test]
fn a_hybrid_is_announced_under_both_hashes() {
    let files = files(9);
    let src = temp_dir("dual-src");
    write_files(&files, &src);
    let torrent = make_torrent(&src, CreateFlags::empty());
    let ti = TorrentInfo::from_bytes(&torrent).unwrap();
    let ih = ti.info_hashes();
    let (url, swarms, _) = swarm_tracker();

    let seed_dir = temp_dir("dual-seed");
    write_files(&files, &seed_dir);
    let seeder = start(None);
    let mut sw = Watch::new(seeder.handle());
    let mut p =
        AddTorrentParams::new(TorrentSource::TorrentFile(torrent.clone()), seed_dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    p.trackers = vec![url.clone()];
    seeder.handle().add_torrent(p);
    let sid = wait(&mut sw, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    wait_state(&mut sw, sid, TorrentState::Seeding, 20);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = swarms.lock().unwrap();
        let v1 = s.contains_key(ih.v1.unwrap().as_bytes().as_slice());
        let v2 = s.contains_key(ih.truncated_v2().unwrap().as_bytes().as_slice());
        if v1 && v2 {
            break;
        }
        drop(s);
        assert!(Instant::now() < deadline, "the seed announced under v1: {v1}, v2: {v2}");
        std::thread::sleep(Duration::from_millis(50));
    }

    let magnet = format!("magnet:?xt=urn:btmh:1220{}&tr={}", ih.v2.unwrap().to_hex(), url);
    let dir = temp_dir("dual-leech");
    let leech = start(None);
    let mut w = Watch::new(leech.handle());
    let mut p = AddTorrentParams::new(TorrentSource::Magnet(magnet), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    leech.handle().add_torrent(p);
    let id = wait(&mut w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    wait(&mut w, 60, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
    leech.shutdown();
    seeder.shutdown();
    assert_files(&files, &dir);
    for d in [src, seed_dir, dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// A forced reannounce of a hybrid announces under both of its hashes, not only the v1 one.
#[test]
fn a_forced_reannounce_of_a_hybrid_announces_both_hashes() {
    let files = files(10);
    let dir = temp_dir("reannounce-hybrid");
    write_files(&files, &dir);
    let torrent = make_torrent(&dir, CreateFlags::empty());
    let ih = TorrentInfo::from_bytes(&torrent).unwrap().info_hashes();
    let (v1, v2) = (ih.v1.unwrap().as_bytes().to_vec(), ih.truncated_v2().unwrap().as_bytes().to_vec());
    let (url, _, announces) = swarm_tracker();
    let count = |h: &[u8]| announces.lock().unwrap().get(h).copied().unwrap_or(0);
    let wait_for = |n: usize, what: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while count(&v1) < n || count(&v2) < n {
            assert!(Instant::now() < deadline, "{what}: v1 {}, v2 {}", count(&v1), count(&v2));
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    let s = start(None);
    let mut w = Watch::new(s.handle());
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(torrent), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    p.trackers = vec![url];
    s.handle().add_torrent(p);
    let id = wait(&mut w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    wait_state(&mut w, id, TorrentState::Seeding, 20);
    wait_for(1, "the first announces");
    // the replies ask for 30 minutes: only the forced reannounce comes before that
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!((count(&v1), count(&v2)), (1, 1));
    s.handle().torrent(id).force_reannounce();
    wait_for(2, "the forced reannounces");
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
