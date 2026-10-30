//! The auto-manager's slow-torrent rule (libtorrent `dont_count_slow_torrents`): with one
//! download slot, a download that has no peers turns inactive after `auto_manage_startup`
//! seconds and the next queued one starts beside it. With the setting off, or with
//! `active_limit` full, the queued one waits.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use patorrent_core::metainfo::{CreateFlags, CreateTorrent, FileStorage, add_files, set_piece_hashes};
use patorrent_core::{
    AddTorrentParams, Alert, Session, SessionHandle, SessionParams, Settings, TorrentFlags, TorrentId,
    TorrentSource,
};

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d =
        std::env::temp_dir().join(format!("lt-queue-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-file torrent of `size` bytes, its data differing by `tag`.
fn make_torrent(dir: &Path, tag: u8, size: usize) -> Vec<u8> {
    let src = dir.join(format!("src-{tag}"));
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join(format!("file-{tag}.bin")), vec![tag; size]).unwrap();
    let mut fs = FileStorage::new();
    add_files(&mut fs, &src, &mut |_| true, CreateFlags::empty()).unwrap();
    let mut t = CreateTorrent::new(fs, 32 * 1024, CreateFlags::empty()).unwrap();
    set_piece_hashes(&mut t, dir, &mut |_| true).unwrap();
    t.generate_buf().unwrap()
}

struct Queue {
    h: SessionHandle,
    backlog: VecDeque<Alert>,
    /// When each torrent was last resumed by the auto-manager, if it is running.
    running: HashMap<TorrentId, Instant>,
}

impl Queue {
    fn poll(&mut self) {
        let mut alerts = Vec::new();
        self.h.pop_alerts(&mut alerts);
        self.backlog.extend(alerts);
        while let Some(a) = self.backlog.pop_front() {
            match a {
                Alert::TorrentResumed { id } => {
                    self.running.insert(id, Instant::now());
                }
                Alert::TorrentPaused { id } => {
                    self.running.remove(&id);
                }
                _ => {}
            }
        }
    }

    /// Polls for `secs` seconds or until `done`.
    fn watch(&mut self, secs: f64, mut done: impl FnMut(&Queue) -> bool) {
        let deadline = Instant::now() + Duration::from_secs_f64(secs);
        while Instant::now() < deadline {
            self.poll();
            if done(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn add(&mut self, torrent: &[u8], dir: &Path) -> TorrentId {
        let mut p =
            AddTorrentParams::new(TorrentSource::TorrentFile(torrent.to_vec()), dir.to_string_lossy());
        p.flags = TorrentFlags::DEFAULT;
        self.h.add_torrent(p);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut alerts = Vec::new();
            self.h.pop_alerts(&mut alerts);
            let mut added = None;
            for a in alerts {
                match a {
                    Alert::TorrentAdded { id, .. } => added = Some(id),
                    Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
                    _ => self.backlog.push_back(a),
                }
            }
            if let Some(id) = added {
                return id;
            }
            assert!(Instant::now() < deadline, "timed out adding");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Two queued downloads without peers under `active_downloads = 1`; returns the session, the
/// queue watch and the ids, the first one already running.
fn two_downloads(name: &str, settings: Settings) -> (Session, Queue, TorrentId, TorrentId, PathBuf) {
    let dir = temp_dir(name);
    let torrents = [make_torrent(&dir, 1, 100_000), make_torrent(&dir, 2, 100_000)];
    let session = Session::start(SessionParams {
        settings: Settings {
            listen_interfaces: "127.0.0.1:0".into(),
            tick_interval: 100,
            enable_dht: false,
            enable_lsd: false,
            enable_upnp: false,
            enable_natpmp: false,
            active_downloads: 1,
            ..settings
        },
        state_dir: None,
    })
    .unwrap();
    let mut q = Queue { h: session.handle(), backlog: VecDeque::new(), running: HashMap::new() };
    let save = dir.join("save");
    let a = q.add(&torrents[0], &save);
    let b = q.add(&torrents[1], &save);
    q.watch(5.0, |q| q.running.contains_key(&a));
    assert!(q.running.contains_key(&a), "the first download starts");
    (session, q, a, b, dir)
}

#[test]
fn an_inactive_download_does_not_take_the_slot() {
    let settings = Settings { auto_manage_startup: 2, ..Settings::default() };
    let (session, mut q, a, b, dir) = two_downloads("on", settings);
    let a_started = q.running[&a];
    q.watch(10.0, |q| q.running.contains_key(&b));
    let b_started = *q.running.get(&b).expect("the second download starts beside the inactive first");
    let waited = b_started.duration_since(a_started);
    assert!(waited >= Duration::from_millis(1500), "after auto_manage_startup, not at once: {waited:?}");
    assert!(q.running.contains_key(&a), "the inactive download keeps running");
    session.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn slow_downloads_count_when_the_setting_is_off() {
    let settings =
        Settings { dont_count_slow_torrents: false, auto_manage_startup: 1, ..Settings::default() };
    let (session, mut q, _a, b, dir) = two_downloads("off", settings);
    q.watch(4.0, |q| q.running.contains_key(&b));
    assert!(!q.running.contains_key(&b), "the second download stays queued");
    session.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn inactive_downloads_still_count_against_active_limit() {
    let settings = Settings { auto_manage_startup: 1, active_limit: 1, ..Settings::default() };
    let (session, mut q, _a, b, dir) = two_downloads("limit", settings);
    q.watch(4.0, |q| q.running.contains_key(&b));
    assert!(!q.running.contains_key(&b), "active_limit is full");
    session.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
