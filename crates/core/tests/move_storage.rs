//! Loopback swarm tests for `TorrentHandle::move_storage` (libtorrent `test_transfer.cpp`'s
//! `move_storage` case and `test_torrent.cpp`'s `test_move_storage_no_metadata`, plus our own):
//! both ends moving during a transfer, a seed moved while seeding and while stopped (the new
//! path surviving a restart), a skipped file's part file following the move, `FailIfExist` and
//! `DontReplace` against files already at the destination, a magnet without metadata, and
//! `MoveCompleted` moving a finished download.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, DownloadPriority, FileOperation, MoveCompleted, MoveFlags, Session,
    SessionHandle, SessionParams, Settings, TorrentFlags, TorrentId, TorrentSource, TorrentState,
    TorrentStatus,
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
    let d = std::env::temp_dir().join(format!("lt-move-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// alpha, beta and gamma share their boundary pieces; beta is in a subdirectory.
fn fixture() -> Fixture {
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
    *info.key("name") = Entry::Bytes(b"move-test".to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let info_hash = Sha1::digest(&info.bencode()).0;
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    Fixture { torrent: t.bencode(), files, info_hash }
}

fn write_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let p = dir.join("move-test").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

fn assert_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let got = std::fs::read(dir.join("move-test").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(got == *data, "{name} differs");
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

fn start_with(state_dir: Option<PathBuf>, f: impl FnOnce(&mut Settings)) -> Session {
    let mut s = settings();
    f(&mut s);
    Session::start(SessionParams { settings: s, state_dir }).expect("session starts")
}

/// A session handle plus the alerts popped but not looked at yet.
struct Watch {
    h: SessionHandle,
    backlog: VecDeque<Alert>,
}

impl Watch {
    fn new(h: SessionHandle) -> Watch {
        Watch { h, backlog: VecDeque::new() }
    }
}

/// Takes alerts in order until `f` returns `Some`, failing after `secs`.
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

fn add_with(w: &mut Watch, f: &Fixture, dir: &Path, prio: Vec<DownloadPriority>) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    p.file_priorities = prio;
    w.h.add_torrent(p);
    wait(w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}

fn add(w: &mut Watch, f: &Fixture, dir: &Path) -> TorrentId {
    add_with(w, f, dir, Vec::new())
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

/// Waits for the move's outcome: `Ok((old, new))` or `Err((file, operation))`.
fn wait_moved(w: &mut Watch, id: TorrentId) -> Result<(String, String), (String, FileOperation)> {
    wait(w, 30, "storage moved", |a| match a {
        Alert::StorageMoved { id: i, old, new } if *i == id => Some(Ok((old.clone(), new.clone()))),
        Alert::StorageMovedFailed { id: i, file, operation, .. } if *i == id => {
            Some(Err((file.clone(), *operation)))
        }
        _ => None,
    })
}

fn same(a: &str, b: &Path) -> bool {
    Path::new(a) == b
}

fn seeder(f: &Fixture) -> (Session, Watch, PathBuf, u16, TorrentId) {
    let dir = temp_dir("seed");
    write_files(f, &dir);
    let s = start_with(None, |_| {});
    let mut w = Watch::new(s.handle());
    let port = listen_port(&mut w);
    let id = add(&mut w, f, &dir);
    wait_state(&mut w, id, TorrentState::Seeding, 20);
    (s, w, dir, port, id)
}

fn local(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn both_ends_move_during_a_transfer() {
    // test_transfer.cpp `move_storage`: once the leech is a bit in, both torrents move.
    let f = fixture();
    let (seed, mut sw, seed_dir, port, sid) = seeder(&f);
    let dir = temp_dir("leech");
    let leech = start_with(None, |s| s.download_rate_limit = 250_000);
    let mut w = Watch::new(leech.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(local(port));
    wait(&mut w, 20, "some payload", |a| status_of(a, id).filter(|s| s.total_payload_download > 200_000));

    let (seed_new, new) = (seed_dir.join("moved"), dir.join("moved"));
    seed.handle().torrent(sid).move_storage(seed_new.to_string_lossy(), MoveFlags::AlwaysReplaceFiles);
    leech.handle().torrent(id).move_storage(new.to_string_lossy(), MoveFlags::AlwaysReplaceFiles);
    let mut finished_first = false;
    let (old, moved_to) = wait(&mut w, 30, "leech moved", |a| match a {
        Alert::TorrentFinished { .. } => {
            finished_first = true;
            None
        }
        Alert::StorageMoved { id: i, old, new } if *i == id => Some((old.clone(), new.clone())),
        _ => None,
    });
    assert!(!finished_first, "the move should come before the end of the download");
    assert!(same(&old, &dir) && same(&moved_to, &new), "{old} -> {moved_to}");
    assert!(wait_moved(&mut sw, sid).is_ok(), "seed moved");

    let st = wait_state(&mut w, id, TorrentState::Seeding, 60);
    assert!(same(&st.save_path, &new));
    assert_eq!(st.total_failed_bytes, 0);
    leech.shutdown();
    seed.shutdown();
    assert_files(&f, &new);
    assert_files(&f, &seed_new);
    assert!(!dir.join("move-test").exists() && !seed_dir.join("move-test").exists(), "nothing left behind");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn a_seed_moves_while_seeding_and_while_stopped_and_keeps_its_path() {
    let f = fixture();
    let dir = temp_dir("seed-move");
    let state = temp_dir("seed-move-state");
    write_files(&f, &dir);
    let (a, b) = (dir.join("a"), dir.join("b"));
    {
        let s = start_with(Some(state.clone()), |_| {});
        let mut w = Watch::new(s.handle());
        listen_port(&mut w);
        let id = add(&mut w, &f, &dir);
        wait_state(&mut w, id, TorrentState::Seeding, 20);

        // While seeding.
        s.handle().torrent(id).move_storage(a.to_string_lossy(), MoveFlags::AlwaysReplaceFiles);
        assert!(wait_moved(&mut w, id).is_ok());
        assert_files(&f, &a);

        // While stopped: the files move too (libtorrent keeps a paused torrent's storage).
        s.handle().torrent(id).pause();
        wait(&mut w, 10, "paused", |al| matches!(al, Alert::TorrentPaused { .. }).then_some(()));
        s.handle().torrent(id).move_storage(b.to_string_lossy(), MoveFlags::AlwaysReplaceFiles);
        let (old, new) = wait_moved(&mut w, id).expect("moved while stopped");
        assert!(same(&old, &a) && same(&new, &b));
        assert_files(&f, &b);
        assert!(!a.join("move-test").exists());
        s.shutdown();
    }

    // The new path is in the resume data; the files there pass resume verification and seed.
    let s = start_with(Some(state.clone()), |_| {});
    let mut w = Watch::new(s.handle());
    let port = listen_port(&mut w);
    let id = first_restored(&mut w).id;
    s.handle().torrent(id).resume();
    let st = wait_state(&mut w, id, TorrentState::Seeding, 20);
    assert!(same(&st.save_path, &b), "{}", st.save_path);

    let ldir = temp_dir("seed-move-leech");
    let leech = start_with(None, |_| {});
    let mut lw = Watch::new(leech.handle());
    let lid = add(&mut lw, &f, &ldir);
    wait_state(&mut lw, lid, TorrentState::Downloading, 10);
    leech.handle().torrent(lid).connect_peer(local(port));
    wait_state(&mut lw, lid, TorrentState::Seeding, 60);
    leech.shutdown();
    s.shutdown();
    assert_files(&f, &ldir);
    for d in [dir, state, ldir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn a_skipped_files_part_file_follows_the_move() {
    let f = fixture();
    let (seed, _sw, seed_dir, port, _) = seeder(&f);
    let dir = temp_dir("skip");
    let new = dir.join("moved");
    let part = format!(".{}.parts", hex(&f.info_hash));
    let leech = start_with(None, |_| {});
    let mut w = Watch::new(leech.handle());
    let id = add_with(&mut w, &f, &dir, vec![DownloadPriority::NORMAL, DownloadPriority::DONT_DOWNLOAD]);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(local(port));
    wait_state(&mut w, id, TorrentState::Finished, 60);
    assert!(dir.join(&part).exists());

    leech.handle().torrent(id).move_storage(new.to_string_lossy(), MoveFlags::AlwaysReplaceFiles);
    assert!(wait_moved(&mut w, id).is_ok());
    assert!(!dir.join(&part).exists() && new.join(&part).exists(), "the part file moved");
    assert!(!new.join("move-test/sub/beta.bin").exists());
    assert!(!dir.join("move-test").exists());

    // A recheck reads the boundary pieces from the moved part file; unskipping exports them
    // into beta at the new place. A second seed: the finished leech dropped the first one as
    // redundant and would wait min_reconnect_time for it.
    leech.handle().torrent(id).force_recheck();
    wait(&mut w, 20, "checked", |a| matches!(a, Alert::TorrentChecked { id: i } if *i == id).then_some(()));
    let st = wait_state(&mut w, id, TorrentState::Finished, 20);
    assert_eq!(st.total_wanted_done, st.total_wanted);
    leech.handle().torrent(id).set_file_priority(1, DownloadPriority::NORMAL);
    let (seed2, _sw2, seed2_dir, port2, _) = seeder(&f);
    leech.handle().torrent(id).connect_peer(local(port2));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 60);
    assert_eq!(st.total_failed_bytes, 0);
    leech.shutdown();
    seed.shutdown();
    seed2.shutdown();
    assert_files(&f, &new);
    assert!(!new.join(&part).exists());
    for d in [dir, seed_dir, seed2_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn fail_if_exist_and_dont_replace_respect_files_at_the_destination() {
    let f = fixture();
    let (seed, _sw, seed_dir, port, _) = seeder(&f);
    let dir = temp_dir("exist");
    write_files(&f, &dir);
    let other = dir.join("other");
    let alpha = other.join("move-test/alpha.bin");
    std::fs::create_dir_all(alpha.parent().unwrap()).unwrap();
    std::fs::write(&alpha, vec![0x55u8; 300_000]).unwrap();

    let s = start_with(None, |_| {});
    let mut w = Watch::new(s.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Seeding, 20);

    // FailIfExist: nothing moves, the torrent keeps seeding from where it was.
    s.handle().torrent(id).move_storage(other.to_string_lossy(), MoveFlags::FailIfExist);
    let (file, op) = wait_moved(&mut w, id).expect_err("alpha exists");
    assert!(file.ends_with("alpha.bin"), "{file}");
    assert_eq!(op, FileOperation::FileStat);
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert!(same(&st.save_path, &dir));
    assert_files(&f, &dir);

    // DontReplace: their alpha stays, ours is left behind, and the recheck finds alpha's
    // pieces missing; the seed supplies them.
    s.handle().torrent(id).move_storage(other.to_string_lossy(), MoveFlags::DontReplace);
    assert!(wait_moved(&mut w, id).is_ok());
    wait(&mut w, 20, "rechecked", |a| matches!(a, Alert::TorrentChecked { id: i } if *i == id).then_some(()));
    let st = wait_state(&mut w, id, TorrentState::Downloading, 10);
    assert!(same(&st.save_path, &other));
    assert!(st.total_done < st.total);
    assert!(dir.join("move-test/alpha.bin").exists(), "ours stays at the old place");
    s.handle().torrent(id).connect_peer(local(port));
    wait_state(&mut w, id, TorrentState::Seeding, 60);
    s.shutdown();
    seed.shutdown();
    assert_files(&f, &other);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn a_magnet_without_metadata_just_changes_its_path() {
    // test_torrent.cpp `test_move_storage_no_metadata`.
    let dir = temp_dir("magnet");
    let s = start_with(None, |_| {});
    let mut w = Watch::new(s.handle());
    let p = AddTorrentParams::new(
        TorrentSource::Magnet("magnet:?xt=urn:btih:abababababababababababababababababababab".into()),
        dir.join("save_path").to_string_lossy(),
    );
    s.handle().add_torrent(p);
    let id = wait(&mut w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    let new = dir.join("save_path_1");
    s.handle().torrent(id).move_storage(new.to_string_lossy(), MoveFlags::AlwaysReplaceFiles);
    let (_, moved_to) = wait_moved(&mut w, id).expect("moved");
    assert!(same(&moved_to, &new));
    wait(&mut w, 5, "new path", |a| status_of(a, id).filter(|st| same(&st.save_path, &new)));
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn move_completed_moves_a_finished_download() {
    let f = fixture();
    let (seed, _sw, seed_dir, port, _) = seeder(&f);
    let dir = temp_dir("completed");
    let done = dir.join("done");
    let leech = start_with(None, |_| {});
    let mut w = Watch::new(leech.handle());
    let mut mover = MoveCompleted::new(done.to_string_lossy());
    mover.only_from = Some(dir.to_string_lossy().into_owned());
    let id = add(&mut w, &f, &dir);
    leech.handle().torrent(id).connect_peer(local(port));
    let h = leech.handle();
    let mut asked = false;
    let new = wait(&mut w, 60, "moved", |a| {
        asked |= mover.on_alert(a, &h) == Some(id);
        match a {
            Alert::StorageMoved { id: i, new, .. } if *i == id => Some(new.clone()),
            _ => None,
        }
    });
    assert!(asked && same(&new, &done));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 10);
    assert!(same(&st.save_path, &done));
    assert!(!mover.wants(&st.save_path), "not moved again");
    leech.shutdown();
    seed.shutdown();
    assert_files(&f, &done);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn commands_queued_behind_a_move_see_the_new_path() {
    use patorrent_core::RemoveFlags;
    let f = fixture();
    let dir = temp_dir("queued");
    write_files(&f, &dir);
    let (a, b) = (dir.join("a"), dir.join("b"));
    let s = start_with(None, |_| {});
    let mut w = Watch::new(s.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Seeding, 20);

    // Stop, move, start at once: the start waits for the move and trusts the files there.
    let t = s.handle().torrent(id);
    t.pause();
    t.move_storage(a.to_string_lossy(), MoveFlags::AlwaysReplaceFiles);
    t.resume();
    assert!(wait_moved(&mut w, id).is_ok());
    let st = wait_state(&mut w, id, TorrentState::Seeding, 20);
    assert!(same(&st.save_path, &a));
    assert_eq!(st.total_done, st.total);
    assert_files(&f, &a);

    // Remove with the data right behind a move: the files are deleted at the destination.
    t.move_storage(b.to_string_lossy(), MoveFlags::AlwaysReplaceFiles);
    s.handle().remove_torrent(id, RemoveFlags::DELETE_FILES);
    wait(&mut w, 20, "deleted", |al| match al {
        Alert::TorrentDeleted { id: i, .. } if *i == id => Some(()),
        Alert::TorrentDeleteFailed { error, .. } => panic!("delete failed: {error}"),
        _ => None,
    });
    s.shutdown();
    assert!(!a.join("move-test").exists() && !b.join("move-test").exists(), "nothing left anywhere");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_seed_moves_between_the_temp_and_target_directories() {
    // On a machine where the temporary directory is another file system (tmpfs) the rename
    // fails with EXDEV and the files are copied, then deleted.
    let f = fixture();
    let dir = temp_dir("volume");
    write_files(&f, &dir);
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(dir.file_name().unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::create_dir_all(&target).unwrap();
        let (a, b) = (std::fs::metadata(&dir).unwrap().dev(), std::fs::metadata(&target).unwrap().dev());
        println!("across file systems: {}", a != b);
    }
    let s = start_with(None, |_| {});
    let mut w = Watch::new(s.handle());
    let id = add(&mut w, &f, &dir);
    wait_state(&mut w, id, TorrentState::Seeding, 20);
    s.handle().torrent(id).move_storage(target.to_string_lossy(), MoveFlags::AlwaysReplaceFiles);
    assert!(wait_moved(&mut w, id).is_ok());
    s.handle().torrent(id).force_recheck();
    wait(&mut w, 20, "checked", |a| matches!(a, Alert::TorrentChecked { id: i } if *i == id).then_some(()));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 20);
    assert_eq!(st.total_done, st.total);
    s.shutdown();
    assert_files(&f, &target);
    assert!(!dir.join("move-test").exists());
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(target);
}
