//! Loopback tests for `TorrentHandle::rename_file` and `AddTorrentParams::renamed_files`
//! (libtorrent `test_torrent.cpp`'s `test_rename_file` and `test_rename_file_fastresume`, plus
//! our own): a seed renaming files while seeding and while stopped, the names surviving a
//! restart and serving a leech from the renamed files, a download into names chosen at add
//! time, and the failures.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, Session, SessionHandle, SessionParams, Settings, TorrentFlags, TorrentId,
    TorrentSource, TorrentState, TorrentStatus,
};

const PIECE: usize = 32 * 1024;

struct Fixture {
    torrent: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
}

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d =
        std::env::temp_dir().join(format!("lt-rename-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Three files sharing boundary pieces; beta and gamma are in a subdirectory.
fn fixture() -> Fixture {
    let sizes = [("alpha.bin", 300_000usize), ("sub/beta.bin", 700_000), ("sub/gamma.txt", 12_345)];
    let mut all = Vec::new();
    let mut files = Vec::new();
    for (i, (name, size)) in sizes.iter().enumerate() {
        let mut x = 0x2545_f491u32.wrapping_mul(i as u32 + 3);
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
    *info.key("name") = Entry::Bytes(b"rename-test".to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    Fixture { torrent: t.bencode(), files }
}

fn write_files(f: &Fixture, dir: &Path) {
    for (name, data) in &f.files {
        let p = dir.join("rename-test").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

/// File `i` of the fixture is at `path` with its contents.
fn assert_file(f: &Fixture, i: usize, path: &Path) {
    let got = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(got == f.files[i].1, "{} differs", path.display());
}

fn start(state_dir: Option<PathBuf>) -> Session {
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
    Session::start(SessionParams { settings, state_dir }).expect("session starts")
}

/// A session handle plus the alerts popped but not looked at yet.
struct Watch {
    h: SessionHandle,
    backlog: VecDeque<Alert>,
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

fn watch(s: &Session) -> Watch {
    Watch { h: s.handle(), backlog: VecDeque::new() }
}

fn listen_port(w: &mut Watch) -> u16 {
    wait(w, 5, "listen", |a| match a {
        Alert::ListenSucceeded { address, .. } => Some(address.port()),
        _ => None,
    })
}

fn add(w: &mut Watch, p: AddTorrentParams) -> TorrentId {
    w.h.add_torrent(p);
    wait(w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}

fn params(f: &Fixture, dir: &Path) -> AddTorrentParams {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    p
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

/// Renames and waits for the outcome: `Ok((old_name, new_name))` or `Err(error text)`.
fn rename(w: &mut Watch, id: TorrentId, file: u32, name: &str) -> Result<(String, String), String> {
    w.h.torrent(id).rename_file(file, name);
    wait(w, 20, "file renamed", |a| match a {
        Alert::FileRenamed { id: i, index, old_name, new_name } if *i == id && *index == file => {
            Some(Ok((old_name.clone(), new_name.clone())))
        }
        Alert::FileRenameFailed { id: i, index, error } if *i == id && *index == file => {
            Some(Err(error.to_string()))
        }
        _ => None,
    })
}

fn file_paths(w: &mut Watch, id: TorrentId) -> Vec<String> {
    w.h.torrent(id).post_file_list();
    wait(w, 5, "file list", |a| match a {
        Alert::FileList { id: i, files } if *i == id => Some(files.iter().map(|f| f.path.clone()).collect()),
        _ => None,
    })
}

fn local(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// Native separators, as `FileStorage::file_path` joins them.
fn native(p: &str) -> String {
    Path::new(p).components().collect::<PathBuf>().to_string_lossy().into_owned()
}

#[test]
fn a_seed_renames_while_seeding_and_stopped_and_keeps_the_names() {
    let f = fixture();
    let dir = temp_dir("seed");
    let state = temp_dir("seed-state");
    write_files(&f, &dir);
    let root = dir.join("rename-test");
    {
        let s = start(Some(state.clone()));
        let mut w = watch(&s);
        listen_port(&mut w);
        let id = add(&mut w, params(&f, &dir));
        wait_state(&mut w, id, TorrentState::Seeding, 20);

        // While seeding: into a new directory inside the torrent's folder.
        let (old, new) = rename(&mut w, id, 1, "rename-test/moved/beta-2.bin").expect("renamed");
        assert_eq!(
            (native(&old), new.as_str()),
            (native("rename-test/sub/beta.bin"), "rename-test/moved/beta-2.bin")
        );
        assert_file(&f, 1, &root.join("moved/beta-2.bin"));
        assert!(!root.join("sub/beta.bin").exists());
        let paths = file_paths(&mut w, id);
        assert_eq!(native(&paths[1]), native("rename-test/moved/beta-2.bin"));

        // While stopped: out of the torrent's folder (the torrent gets a storage for it).
        s.handle().torrent(id).pause();
        wait(&mut w, 10, "paused", |a| matches!(a, Alert::TorrentPaused { .. }).then_some(()));
        rename(&mut w, id, 0, "alpha-outside.bin").expect("renamed while stopped");
        assert_file(&f, 0, &dir.join("alpha-outside.bin"));
        assert!(!root.join("alpha.bin").exists());

        // Errors: no such file, an empty name, a name taken by another file (libtorrent 2.0.12
        // does not overwrite it).
        assert!(rename(&mut w, id, 9, "x").is_err());
        let e = rename(&mut w, id, 2, "rename-test/moved/beta-2.bin").expect_err("taken");
        assert!(e.contains("exists"), "{e}");
        assert_file(&f, 1, &root.join("moved/beta-2.bin"));
        assert_file(&f, 2, &root.join("sub/gamma.txt"));
        assert!(rename(&mut w, id, 2, "").is_err());
        s.shutdown();
    }

    // The names are in the resume data: resume verification finds the files there, and a
    // leech gets the torrent's own layout from them.
    let s = start(Some(state.clone()));
    let mut w = watch(&s);
    let port = listen_port(&mut w);
    let id = first_restored(&mut w).id;
    s.handle().torrent(id).resume();
    wait_state(&mut w, id, TorrentState::Seeding, 20);
    let paths = file_paths(&mut w, id);
    assert_eq!(native(&paths[0]), "alpha-outside.bin");
    assert_eq!(native(&paths[1]), native("rename-test/moved/beta-2.bin"));

    let ldir = temp_dir("seed-leech");
    let leech = start(None);
    let mut lw = watch(&leech);
    let lid = add(&mut lw, params(&f, &ldir));
    wait_state(&mut lw, lid, TorrentState::Downloading, 10);
    leech.handle().torrent(lid).connect_peer(local(port));
    let st = wait_state(&mut lw, lid, TorrentState::Seeding, 60);
    assert_eq!(st.total_failed_bytes, 0);
    leech.shutdown();

    // Renaming back to the metadata's name works and drops the mapping.
    rename(&mut w, id, 0, "rename-test/alpha.bin").expect("renamed back");
    assert_file(&f, 0, &root.join("alpha.bin"));
    s.shutdown();
    for (i, (name, _)) in f.files.iter().enumerate() {
        assert_file(&f, i, &ldir.join("rename-test").join(name));
    }
    for d in [dir, state, ldir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn renamed_files_at_add_time_take_the_download() {
    let f = fixture();
    let seed_dir = temp_dir("add-seed");
    write_files(&f, &seed_dir);
    let seed = start(None);
    let mut sw = watch(&seed);
    let port = listen_port(&mut sw);
    let sid = add(&mut sw, params(&f, &seed_dir));
    wait_state(&mut sw, sid, TorrentState::Seeding, 20);

    // µTorrent's Add New Torrent "Name": the torrent's folder under another name.
    let dir = temp_dir("add-leech");
    let leech = start(None);
    let mut w = watch(&leech);
    let mut p = params(&f, &dir);
    for (i, (name, _)) in f.files.iter().enumerate() {
        p.renamed_files.insert(i as u32, format!("My Folder/{name}"));
    }
    p.renamed_files.insert(2, "My Folder/notes.txt".into());
    p.renamed_files.insert(77, "ignored".into());
    let id = add(&mut w, p);
    wait_state(&mut w, id, TorrentState::Downloading, 10);
    leech.handle().torrent(id).connect_peer(local(port));
    let st = wait_state(&mut w, id, TorrentState::Seeding, 60);
    assert_eq!(st.total_failed_bytes, 0);
    leech.shutdown();
    seed.shutdown();
    let folder = dir.join("My Folder");
    assert_file(&f, 0, &folder.join("alpha.bin"));
    assert_file(&f, 1, &folder.join("sub/beta.bin"));
    assert_file(&f, 2, &folder.join("notes.txt"));
    assert!(!dir.join("rename-test").exists(), "nothing under the metadata's name");
    for d in [dir, seed_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn renaming_before_the_metadata_is_known_fails() {
    let s = start(None);
    let mut w = watch(&s);
    let dir = temp_dir("magnet");
    let magnet = format!("magnet:?xt=urn:btih:{}", "ab".repeat(20));
    let mut p = AddTorrentParams::new(TorrentSource::Magnet(magnet), dir.to_string_lossy());
    p.flags = TorrentFlags::PAUSED;
    let id = add(&mut w, p);
    let e = rename(&mut w, id, 0, "x").expect_err("no metadata");
    assert!(e.contains("metadata"), "{e}");
    s.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
