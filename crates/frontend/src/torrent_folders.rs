//! µTorrent 2.2.1's "Location of .torrents" (Preferences > Directories): a copy of every added
//! `.torrent` kept in one folder, moved to another when its job finishes, and a folder watched
//! for new `.torrent` files to add.
//!
//! [`TorrentFolders`] is what the page edits. [`Poller`] watches the auto-load folder and hands
//! out the files that stopped changing; [`TorrentFiles`] follows the session's alerts to store
//! the copies once the engine has added a torrent, move them when it finishes and rename (or
//! delete) the auto-loaded files once they are in the session. Toolkit-independent: the Win32
//! GUI and nox drive them from their own timers and alert loops.
//!
//! What µTorrent did and what is done here:
//!
//! * Store: the copy is named after the torrent (`<name>.torrent`, sanitised for every file
//!   system; a different torrent of the same name gets `<name>.1.torrent`, ...). An identical
//!   file already there is kept.
//! * Move for finished jobs: on `TorrentFinished` the stored copy (found by name and checked by
//!   info-hash) moves to the second folder, by rename or, across volumes, copy and delete.
//! * Auto-load: the folder is scanned every [`POLL_INTERVAL`]; a `*.torrent` whose size and
//!   modification time are the same on two scans is added without the Add New Torrent dialog,
//!   and once the engine has it the file is renamed to `*.torrent.loaded` (µTorrent's suffix)
//!   or deleted with "Delete loaded .torrents". A file that cannot be read or parsed is left
//!   alone until it changes.
//!
//! Magnet links have no `.torrent` to copy: the engine has no API that hands out a torrent's
//! info dictionary yet, so their copies are not stored.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use patorrent_core::metainfo::TorrentInfo;
use patorrent_core::{AddTorrentParams, Alert, TorrentId, TorrentSource};

use crate::prefs::Prefs;

/// How often the auto-load folder is scanned (µTorrent looked every few seconds). A file is
/// added after two scans that agree, so 2 to 4 s after it stops changing.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The suffix µTorrent appends to auto-loaded files.
pub const LOADED_SUFFIX: &str = ".loaded";

/// Preferences > Directories, "Location of .torrents" (preferences file keys in brackets).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TorrentFolders {
    /// "Store .torrents in:" (`dir.store_torrents`, folder `dir.torrents`).
    pub store: bool,
    pub store_dir: String,
    /// "Move .torrents for finished jobs to:" (`dir.move_torrents`, folder
    /// `dir.finished_torrents`); only with `store`, as in µTorrent.
    pub move_finished: bool,
    pub finished_dir: String,
    /// "Automatically load .torrents from:" (`dir.autoload`, folder `dir.autoload_dir`).
    pub autoload: bool,
    pub autoload_dir: String,
    /// "Delete loaded .torrents" (`dir.autoload_delete`): delete instead of renaming to
    /// `.torrent.loaded`.
    pub autoload_delete: bool,
}

impl TorrentFolders {
    pub fn load(prefs: &Prefs) -> TorrentFolders {
        let dir = |k: &str| prefs.get(k).unwrap_or_default().to_owned();
        TorrentFolders {
            store: prefs.get_bool("dir.store_torrents", false),
            store_dir: dir("dir.torrents"),
            move_finished: prefs.get_bool("dir.move_torrents", false),
            finished_dir: dir("dir.finished_torrents"),
            autoload: prefs.get_bool("dir.autoload", false),
            autoload_dir: dir("dir.autoload_dir"),
            autoload_delete: prefs.get_bool("dir.autoload_delete", false),
        }
    }

    pub fn store(&self, prefs: &mut Prefs) {
        let mut dir = |k: &str, v: &str| match v.trim() {
            "" => prefs.remove(k),
            v => prefs.set(k, v),
        };
        dir("dir.torrents", &self.store_dir);
        dir("dir.finished_torrents", &self.finished_dir);
        dir("dir.autoload_dir", &self.autoload_dir);
        prefs.set_bool("dir.store_torrents", self.store);
        prefs.set_bool("dir.move_torrents", self.move_finished);
        prefs.set_bool("dir.autoload", self.autoload);
        prefs.set_bool("dir.autoload_delete", self.autoload_delete);
    }

    /// Where copies of added `.torrent` files go, when that is on and has a folder.
    pub fn stores_in(&self) -> Option<&str> {
        Some(self.store_dir.trim()).filter(|d| self.store && !d.is_empty())
    }

    /// Where the copies of finished torrents go, when that (and storing) is on.
    pub fn moves_finished_to(&self) -> Option<&str> {
        self.stores_in()?;
        Some(self.finished_dir.trim()).filter(|d| self.move_finished && !d.is_empty())
    }

    /// The folder to watch, when auto-loading is on and has a folder.
    pub fn loads_from(&self) -> Option<&str> {
        Some(self.autoload_dir.trim()).filter(|d| self.autoload && !d.is_empty())
    }
}

/// A torrent's name as a file name that every file system takes: no separators, wildcards,
/// control characters, trailing dots or spaces, nor DOS device names; at most 200 bytes.
pub fn file_name(name: &str) -> String {
    let mut s: String =
        name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    if s.len() > 200 {
        let mut end = 200;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    let s = s.trim_end_matches(['.', ' ']).trim_start();
    if s.is_empty() {
        return "torrent".to_owned();
    }
    let stem = s.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if device { format!("_{s}") } else { s.to_owned() }
}

/// The file names a copy of torrent `name` may have in a folder: `<name>.torrent`, then
/// `<name>.1.torrent`, `<name>.2.torrent`, ...
fn candidates(name: &str) -> impl Iterator<Item = String> {
    let base = file_name(name);
    (0..1000).map(move |i| if i == 0 { format!("{base}.torrent") } else { format!("{base}.{i}.torrent") })
}

/// Puts `bytes` into `dir` as torrent `name`'s copy (creating the folder): an identical file
/// already there is kept, a different one gets the next free name. Returns the copy's path.
pub fn place(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    for file in candidates(name) {
        let path = dir.join(file);
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                use std::io::Write;
                if let Err(e) = f.write_all(bytes).and_then(|()| f.sync_all()) {
                    drop(f);
                    let _ = fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if fs::read(&path).is_ok_and(|b| b == bytes) {
                    return Ok(path);
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "too many copies of the same name"))
}

/// Torrent `name`'s copy in `dir`: the first of its [candidate names](place) whose info-hash
/// is `key` (`InfoHashes::key_hex`).
pub fn find(dir: &Path, name: &str, key: &str) -> Option<PathBuf> {
    let mut misses = 0;
    for file in candidates(name) {
        let path = dir.join(file);
        match fs::read(&path) {
            Ok(b) => {
                misses = 0;
                if TorrentInfo::from_bytes(&b).is_ok_and(|ti| ti.info_hashes().key_hex() == key) {
                    return Some(path);
                }
            }
            // a gap left by a moved copy; a few in a row end the search
            Err(_) => {
                misses += 1;
                if misses > 3 {
                    return None;
                }
            }
        }
    }
    None
}

/// Moves torrent `name`'s copy (info-hash `key`) from `from` into `to`. `Ok(None)`: there is
/// none in `from`. Renames when it can, else copies and deletes (another volume).
pub fn move_copy(from: &Path, to: &Path, name: &str, key: &str) -> io::Result<Option<PathBuf>> {
    let Some(src) = find(from, name, key) else { return Ok(None) };
    if same_dir(from, to) {
        return Ok(Some(src));
    }
    let bytes = fs::read(&src)?;
    fs::create_dir_all(to)?;
    // the name the copy would get there, without writing it yet
    let target = candidates(name)
        .map(|f| to.join(f))
        .find(|p| !p.exists() || fs::read(p).is_ok_and(|b| b == bytes))
        .ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "too many copies of the same name"))?;
    if target.exists() {
        fs::remove_file(&src)?;
    } else if fs::rename(&src, &target).is_err() {
        let target = place(to, name, &bytes)?;
        fs::remove_file(&src)?;
        return Ok(Some(target));
    }
    Ok(Some(target))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    patorrent_core::same_dir(&a.to_string_lossy(), &b.to_string_lossy())
}

/// Reads and parses a `.torrent` the poller handed out.
pub fn read(path: &Path) -> Result<(Vec<u8>, TorrentInfo), String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let ti = TorrentInfo::from_bytes(&bytes).map_err(|e| e.to_string())?;
    Ok((bytes, ti))
}

/// Done with an auto-loaded file: deleted, or renamed to `<file>.loaded` (replacing an older
/// one). Returns the new name, `None` when deleted.
pub fn finish_loaded(path: &Path, delete: bool) -> io::Result<Option<PathBuf>> {
    if delete {
        fs::remove_file(path)?;
        return Ok(None);
    }
    let mut loaded = path.as_os_str().to_owned();
    loaded.push(LOADED_SUFFIX);
    let loaded = PathBuf::from(loaded);
    fs::rename(path, &loaded)?;
    Ok(Some(loaded))
}

fn is_torrent_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
}

/// A file's size and modification time: equal on two scans means nobody is writing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

/// Watches the auto-load folder.
#[derive(Debug, Default)]
pub struct Poller {
    dir: PathBuf,
    last: Option<Instant>,
    /// Every `*.torrent` of the last scan.
    seen: HashMap<PathBuf, Stamp>,
    /// Files handed out, with the stamp they had: not again until they change.
    handed: HashMap<PathBuf, Stamp>,
    /// The folder's read error, and whether it was taken already.
    error: Option<(String, bool)>,
}

impl Poller {
    pub fn new() -> Poller {
        Poller::default()
    }

    /// Scans `dir` when [`POLL_INTERVAL`] has passed since the last scan (or the folder
    /// changed); see [`Poller::scan`].
    pub fn poll(&mut self, dir: &Path, now: Instant) -> Vec<PathBuf> {
        if self.dir == dir && self.last.is_some_and(|t| now < t + POLL_INTERVAL) {
            return Vec::new();
        }
        self.last = Some(now);
        self.scan(dir)
    }

    /// The `*.torrent` files in `dir` that have the same size and modification time as at the
    /// previous scan and were not handed out with them before, sorted by path.
    pub fn scan(&mut self, dir: &Path) -> Vec<PathBuf> {
        if self.dir != dir {
            *self = Poller { dir: dir.to_owned(), last: self.last, ..Poller::default() };
        }
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                let msg = format!("{}: {e}", dir.display());
                if self.error.as_ref().is_none_or(|(m, _)| *m != msg) {
                    self.error = Some((msg, false));
                }
                self.seen.clear();
                return Vec::new();
            }
        };
        self.error = None;
        let mut now = HashMap::new();
        let mut out = Vec::new();
        for e in entries.flatten() {
            let path = e.path();
            if !is_torrent_file(&path) {
                continue;
            }
            let Ok(md) = fs::metadata(&path) else { continue };
            if !md.is_file() {
                continue;
            }
            let stamp = Stamp { len: md.len(), modified: md.modified().ok() };
            if stamp.len > 0 && self.seen.get(&path) == Some(&stamp) && self.handed.get(&path) != Some(&stamp)
            {
                self.handed.insert(path.clone(), stamp);
                out.push(path.clone());
            }
            now.insert(path, stamp);
        }
        self.handed.retain(|p, _| now.contains_key(p));
        self.seen = now;
        out.sort();
        out
    }

    /// Why the folder could not be read, once per new error.
    pub fn take_error(&mut self) -> Option<String> {
        match &mut self.error {
            Some((msg, taken @ false)) => {
                *taken = true;
                Some(msg.clone())
            }
            _ => None,
        }
    }
}

/// A torrent on its way into the session.
#[derive(Debug, Default)]
struct Pending {
    name: String,
    /// The `.torrent` to store once added.
    bytes: Option<Vec<u8>>,
    /// The auto-loaded file to rename or delete once added.
    loaded_from: Option<PathBuf>,
}

/// Follows the session's alerts for the `.torrent` folders: stores the copies of torrents the
/// engine added, moves them when the torrents finish, and finishes auto-loaded files.
#[derive(Debug, Default)]
pub struct TorrentFiles {
    /// By `InfoHashes::key_hex`.
    pending: HashMap<String, Pending>,
    /// Every torrent the session reported: its info-hash key and name.
    known: HashMap<TorrentId, (String, String)>,
}

impl TorrentFiles {
    pub fn new() -> TorrentFiles {
        TorrentFiles::default()
    }

    /// A torrent is being added with `p`: a `.torrent`'s copy is stored once the engine has it
    /// (when [`TorrentFolders::stores_in`] has a folder now). Nothing for magnets.
    pub fn adding(&mut self, folders: &TorrentFolders, p: &AddTorrentParams) {
        let TorrentSource::TorrentFile(bytes) = &p.source else { return };
        if folders.stores_in().is_none() {
            return;
        }
        let Ok(ti) = TorrentInfo::from_bytes(bytes) else { return };
        let e = self.pending.entry(ti.info_hashes().key_hex()).or_default();
        e.name = ti.name().to_owned();
        e.bytes = Some(bytes.clone());
    }

    /// `ti` was auto-loaded from `path`: the file is finished once the engine has the torrent.
    pub fn loading(&mut self, ti: &TorrentInfo, path: PathBuf) {
        let e = self.pending.entry(ti.info_hashes().key_hex()).or_default();
        e.name = ti.name().to_owned();
        e.loaded_from = Some(path);
    }

    /// Looks at one alert; returns lines for the log (`"<name>: <what happened>"`).
    pub fn on_alert(&mut self, alert: &Alert, folders: &TorrentFolders) -> Vec<String> {
        let mut log = Vec::new();
        match alert {
            Alert::TorrentAdded { id, info_hashes, name } => {
                let key = info_hashes.key_hex();
                self.known.insert(*id, (key.clone(), name.clone()));
                let Some(p) = self.pending.remove(&key) else { return log };
                if let (Some(bytes), Some(dir)) = (&p.bytes, folders.stores_in()) {
                    match place(Path::new(dir), name, bytes) {
                        Ok(path) => log.push(format!("{name}: .torrent stored as {}", path.display())),
                        Err(e) => log.push(format!("{name}: could not store the .torrent in {dir}: {e}")),
                    }
                }
                if let Some(path) = &p.loaded_from {
                    log.push(loaded(name, path, folders.autoload_delete));
                }
            }
            Alert::AddTorrentFailed { name, error } => {
                // the engine names a torrent from a file as its info says; files stay as they
                // are (the poller does not hand them out again until they change)
                self.pending.retain(|_, p| {
                    if p.name != *name {
                        return true;
                    }
                    if let Some(path) = &p.loaded_from {
                        log.push(format!("{}: not loaded: {error}", path.display()));
                    }
                    false
                });
            }
            Alert::StateUpdate(v) => {
                for st in v {
                    if let Some(k) = self.known.get_mut(&st.id) {
                        k.1.clone_from(&st.name);
                    } else {
                        self.known.insert(st.id, (st.info_hashes.key_hex(), st.name.clone()));
                    }
                }
            }
            Alert::TorrentRemoved { id, .. } => {
                self.known.remove(id);
            }
            Alert::TorrentFinished { id } => {
                let (Some(from), Some(to), Some((key, name))) =
                    (folders.stores_in(), folders.moves_finished_to(), self.known.get(id))
                else {
                    return log;
                };
                match move_copy(Path::new(from), Path::new(to), name, key) {
                    Ok(Some(path)) => log.push(format!("{name}: .torrent moved to {}", path.display())),
                    Ok(None) => {}
                    Err(e) => log.push(format!("{name}: could not move the .torrent to {to}: {e}")),
                }
            }
            _ => {}
        }
        log
    }
}

/// Renames or deletes an auto-loaded file whose torrent is in the session; the log line.
pub fn loaded(name: &str, path: &Path, delete: bool) -> String {
    match finish_loaded(path, delete) {
        Ok(Some(to)) => format!("{name}: loaded from {}, renamed to {}", path.display(), to.display()),
        Ok(None) => format!("{name}: loaded from {}, deleted", path.display()),
        Err(e) => {
            format!("{name}: loaded from {}, which could not be renamed or deleted: {e}", path.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use patorrent_core::{InfoHashes, TorrentStatus};

    use super::*;

    /// A unique empty folder under the temp directory.
    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("patorrent-folders-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// A single-file torrent named `name`; `n` makes the info-hash differ.
    fn torrent(name: &str, n: u32) -> Vec<u8> {
        format!(
            "d4:infod6:lengthi{}e4:name{}:{name}12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee",
            5 + n,
            name.len()
        )
        .into_bytes()
    }

    fn key(bytes: &[u8]) -> String {
        TorrentInfo::from_bytes(bytes).unwrap().info_hashes().key_hex()
    }

    fn hashes(bytes: &[u8]) -> InfoHashes {
        TorrentInfo::from_bytes(bytes).unwrap().info_hashes()
    }

    fn folders(store: &Path, finished: Option<&Path>) -> TorrentFolders {
        TorrentFolders {
            store: true,
            store_dir: store.to_string_lossy().into_owned(),
            move_finished: finished.is_some(),
            finished_dir: finished.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            ..TorrentFolders::default()
        }
    }

    #[test]
    fn options_round_trip_and_gating() {
        let mut prefs = Prefs::default();
        let d = TorrentFolders::load(&prefs);
        assert_eq!(d, TorrentFolders::default());
        assert_eq!((d.stores_in(), d.moves_finished_to(), d.loads_from()), (None, None, None));

        let f = TorrentFolders {
            store: true,
            store_dir: r" C:\t ".into(),
            move_finished: true,
            finished_dir: r"C:\done".into(),
            autoload: true,
            autoload_dir: r"C:\watch".into(),
            autoload_delete: true,
        };
        f.store(&mut prefs);
        let text = prefs.to_text();
        assert!(text.contains("dir.torrents=C:\\t\n") && text.contains("dir.autoload_delete=1\n"));
        let back = TorrentFolders::load(&prefs);
        assert_eq!(back, TorrentFolders { store_dir: r"C:\t".into(), ..f.clone() });
        assert_eq!(back.stores_in(), Some(r"C:\t"));
        assert_eq!(back.moves_finished_to(), Some(r"C:\done"));
        assert_eq!(back.loads_from(), Some(r"C:\watch"));

        // moving needs storing; an empty folder is off
        let mut g = back.clone();
        g.store = false;
        assert_eq!((g.stores_in(), g.moves_finished_to()), (None, None));
        g.autoload_dir = "  ".into();
        assert_eq!(g.loads_from(), None);
        g.store(&mut prefs);
        assert_eq!(prefs.get("dir.autoload_dir"), None);
    }

    #[test]
    fn file_names_are_sanitised() {
        assert_eq!(file_name("Debian 13.iso"), "Debian 13.iso");
        assert_eq!(file_name(r#"a/b\c:d*e?f"g<h>i|j"#), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(file_name("tab\there"), "tab_here");
        assert_eq!(file_name("trailing. . "), "trailing");
        assert_eq!(file_name("..."), "torrent");
        assert_eq!(file_name(""), "torrent");
        assert_eq!(file_name("con"), "_con");
        assert_eq!(file_name("Com1.txt"), "_Com1.txt");
        assert_eq!(file_name("console"), "console");
        let long = "é".repeat(150);
        let s = file_name(&long);
        assert!(s.len() <= 200 && s.chars().all(|c| c == 'é'));
    }

    #[test]
    fn copies_are_placed_found_and_moved() {
        let store = temp("store");
        let done = temp("done");
        let a = torrent("x", 1);
        let b = torrent("x", 2);
        // the same torrent twice is one file; another of the same name gets the next one
        let pa = place(&store, "x", &a).unwrap();
        assert_eq!(pa, store.join("x.torrent"));
        assert_eq!(place(&store, "x", &a).unwrap(), pa);
        let pb = place(&store, "x", &b).unwrap();
        assert_eq!(pb, store.join("x.1.torrent"));
        assert_eq!(find(&store, "x", &key(&b)), Some(pb.clone()));
        assert_eq!(find(&store, "x", &key(&a)), Some(pa.clone()));
        assert_eq!(find(&store, "y", &key(&a)), None);

        // moved: b lands as x.torrent in the new folder; a, moved after, as x.1.torrent
        assert_eq!(move_copy(&store, &done, "x", &key(&b)).unwrap(), Some(done.join("x.torrent")));
        assert!(!pb.exists());
        assert_eq!(move_copy(&store, &done, "x", &key(&a)).unwrap(), Some(done.join("x.1.torrent")));
        assert_eq!(fs::read(done.join("x.1.torrent")).unwrap(), a);
        assert_eq!(move_copy(&store, &done, "x", &key(&a)).unwrap(), None, "nothing left to move");

        // a gap (x.torrent gone) does not hide x.1.torrent; an identical target is reused
        place(&store, "x", &a).unwrap();
        fs::copy(store.join("x.torrent"), store.join("x.1.torrent")).unwrap();
        fs::remove_file(store.join("x.torrent")).unwrap();
        assert_eq!(find(&store, "x", &key(&a)), Some(store.join("x.1.torrent")));
        assert_eq!(move_copy(&store, &done, "x", &key(&a)).unwrap(), Some(done.join("x.1.torrent")));
        assert!(!store.join("x.1.torrent").exists());
        assert_eq!(fs::read_dir(&done).unwrap().count(), 2);

        // the same folder: left where it is
        place(&store, "x", &a).unwrap();
        assert_eq!(move_copy(&store, &store, "x", &key(&a)).unwrap(), Some(store.join("x.torrent")));
        let _ = fs::remove_dir_all(&store);
        let _ = fs::remove_dir_all(&done);
    }

    #[test]
    fn the_poller_waits_for_files_to_settle() {
        let dir = temp("poll");
        let mut p = Poller::new();
        fs::write(dir.join("a.torrent"), torrent("a", 0)).unwrap();
        fs::write(dir.join("notes.txt"), "x").unwrap();
        fs::write(dir.join("old.torrent.loaded"), torrent("o", 0)).unwrap();
        fs::write(dir.join("empty.torrent"), "").unwrap();
        fs::create_dir(dir.join("folder.torrent")).unwrap();
        assert!(p.scan(&dir).is_empty(), "first sight: not yet");
        assert_eq!(p.scan(&dir), [dir.join("a.torrent")]);
        assert!(p.scan(&dir).is_empty(), "handed out once");

        // still being written: only once two scans agree
        fs::write(dir.join("B.TORRENT"), "d4:info").unwrap();
        assert!(p.scan(&dir).is_empty());
        fs::write(dir.join("B.TORRENT"), torrent("b", 0)).unwrap();
        assert!(p.scan(&dir).is_empty(), "it grew");
        assert_eq!(p.scan(&dir), [dir.join("B.TORRENT")]);

        // a changed file comes again; a renamed one is gone and forgotten
        fs::write(dir.join("a.torrent"), torrent("a", 10)).unwrap();
        p.scan(&dir);
        assert_eq!(p.scan(&dir), [dir.join("a.torrent")]);
        let renamed = finish_loaded(&dir.join("a.torrent"), false).unwrap().unwrap();
        assert_eq!(renamed, dir.join("a.torrent.loaded"));
        assert!(p.scan(&dir).is_empty());
        assert_eq!(finish_loaded(&dir.join("B.TORRENT"), true).unwrap(), None);
        assert!(!dir.join("B.TORRENT").exists());

        // poll() keeps to the interval
        let t = Instant::now();
        fs::write(dir.join("c.torrent"), torrent("c", 0)).unwrap();
        assert!(p.poll(&dir, t).is_empty());
        assert!(p.poll(&dir, t + Duration::from_secs(1)).is_empty(), "too soon to look");
        assert_eq!(p.poll(&dir, t + POLL_INTERVAL), [dir.join("c.torrent")]);

        // a missing folder is reported once
        let gone = dir.join("missing");
        assert!(p.scan(&gone).is_empty());
        assert!(p.take_error().is_some_and(|e| e.contains("missing")));
        assert!(p.scan(&gone).is_empty());
        assert_eq!(p.take_error(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn alerts_store_move_and_finish() {
        let store = temp("alerts-store");
        let done = temp("alerts-done");
        let watch = temp("alerts-watch");
        let mut f = folders(&store, Some(&done));
        let mut files = TorrentFiles::new();
        let bytes = torrent("movie", 0);
        let id = TorrentId::from_raw(7);

        // added from a file: stored once the engine says so
        let p = AddTorrentParams::new(TorrentSource::TorrentFile(bytes.clone()), "x");
        files.adding(&f, &p);
        assert!(!store.join("movie.torrent").exists());
        let added = Alert::TorrentAdded { id, info_hashes: hashes(&bytes), name: "movie".into() };
        let log = files.on_alert(&added, &f);
        assert_eq!(fs::read(store.join("movie.torrent")).unwrap(), bytes);
        assert!(log[0].starts_with("movie: .torrent stored as"), "{log:?}");

        // finished: moved
        let log = files.on_alert(&Alert::TorrentFinished { id }, &f);
        assert!(done.join("movie.torrent").exists() && !store.join("movie.torrent").exists());
        assert!(log[0].starts_with("movie: .torrent moved to"), "{log:?}");
        assert!(files.on_alert(&Alert::TorrentFinished { id }, &f).is_empty(), "nothing left to move");

        // auto-loaded: renamed once added; with storing off nothing is copied
        f.store = false;
        let other = torrent("show", 0);
        let path = watch.join("show.torrent");
        fs::write(&path, &other).unwrap();
        let (b, ti) = read(&path).unwrap();
        let p = AddTorrentParams::new(TorrentSource::TorrentFile(b), "x");
        files.adding(&f, &p);
        files.loading(&ti, path.clone());
        let id2 = TorrentId::from_raw(8);
        let log = files
            .on_alert(&Alert::TorrentAdded { id: id2, info_hashes: hashes(&other), name: "show".into() }, &f);
        assert_eq!(log.len(), 1, "{log:?}");
        assert!(watch.join("show.torrent.loaded").exists() && !path.exists());
        assert!(!store.join("show.torrent").exists());

        // a failed add leaves the file alone
        let third = torrent("bad", 0);
        let path = watch.join("bad.torrent");
        fs::write(&path, &third).unwrap();
        let (_, ti) = read(&path).unwrap();
        files.loading(&ti, path.clone());
        let failed =
            Alert::AddTorrentFailed { name: "bad".into(), error: patorrent_core::Error::DuplicateTorrent };
        let log = files.on_alert(&failed, &f);
        assert!(log[0].contains("not loaded"), "{log:?}");
        assert!(path.exists());
        assert!(files.pending.is_empty());

        // names follow status updates (a magnet's name arrives with its metadata)
        let st = TorrentStatus { id: id2, name: "renamed".into(), ..TorrentStatus::default() };
        files.on_alert(&Alert::StateUpdate(vec![st]), &f);
        assert_eq!(files.known[&id2].1, "renamed");
        files.on_alert(&Alert::TorrentRemoved { id: id2, info_hashes: hashes(&other) }, &f);
        assert!(!files.known.contains_key(&id2));
        for d in [store, done, watch] {
            let _ = fs::remove_dir_all(d);
        }
    }
}
