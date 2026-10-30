//! File > Import: torrents from qBittorrent's `BT_backup` folder or µTorrent's `resume.dat`
//! (V1.1), carried over with their statistics, priorities, trackers and labels.
//!
//! Each source torrent becomes a libtorrent resume record with its info dictionary embedded,
//! added as [`TorrentSource::Resume`]. The source's claimed pieces are not trusted: the engine
//! checks the files (SEC-16, [`distrust`]), and mapped file paths stay inside the save folder.
//! The confirm question lists the save folders. The source client's files are only read; a
//! running source client is refused, since both rewrite their files when they exit.
//!
//! * qBittorrent: `<hash>.fastresume` already is a libtorrent resume record (`resume.rs` reads
//!   its keys); `<hash>.torrent` gives the info dictionary. `qBt-name` is the name,
//!   `qBt-category` (or the first tag) the label. `save_path` is where the data is now, so a
//!   torrent qBittorrent keeps in its incomplete folder stays there. `qBt-ratioLimit` and
//!   `qBt-seedingTimeLimit` become the torrent's own seeding goal ([`qbittorrent_goal`]); its
//!   stopped state is libtorrent's `paused` without `auto_managed` ([`qbittorrent_flags`]).
//!   Checked against qBittorrent's `bencoderesumedatastorage.cpp` (v4_6_x and master, 5.x).
//! * µTorrent: `resume.dat` is one dictionary keyed by the `.torrent` file's name in the same
//!   folder. Read from a real 2.2.1 file: `path` is the torrent's folder (or single file),
//!   `have` its pieces (least significant bit first), `prio` one byte per file (`0x80` and
//!   `0` skip, `0x08` normal), `trackers` one list (an empty string starts a new tier).
//!   `started` 0 is Stopped and 3 Paused (from µTorrent's Web UI status; not yet seen in a file).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use patorrent_core::bencode::{Entry, Node, bdecode};
use patorrent_core::metainfo::TorrentInfo;
use patorrent_core::metainfo::sanitize::sanitize_append_path_element;
use patorrent_core::{AddTorrentParams, InfoHashes, Sha1Hash, TorrentFlags, TorrentSource};

use crate::lang::S;
use crate::prefs::Prefs;
use crate::seeding::SeedGoal;

/// A client to import from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Client {
    QBittorrent,
    UTorrent,
}

impl Client {
    pub const ALL: [Client; 2] = [Client::QBittorrent, Client::UTorrent];

    /// Its name, as messages show it (not translated).
    pub fn name(self) -> &'static str {
        match self {
            Client::QBittorrent => "qBittorrent",
            Client::UTorrent => "µTorrent",
        }
    }

    /// The File menu's entry.
    pub fn menu_text(self) -> &'static str {
        match self {
            Client::QBittorrent => crate::tr!(MenuImportQbt),
            Client::UTorrent => crate::tr!(MenuImportUt),
        }
    }

    /// Its `--import` value in patorrent-nox.
    pub fn id(self) -> &'static str {
        match self {
            Client::QBittorrent => "qbittorrent",
            Client::UTorrent => "utorrent",
        }
    }

    pub fn from_id(id: &str) -> Option<Client> {
        Client::ALL.into_iter().find(|c| c.id().eq_ignore_ascii_case(id))
    }

    /// Where the client keeps its torrents for this user, if that folder exists.
    pub fn default_dir(self) -> Option<PathBuf> {
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let dir = match self {
            Client::QBittorrent if cfg!(windows) => env("LOCALAPPDATA")?.join("qBittorrent"),
            Client::QBittorrent if cfg!(target_os = "macos") => {
                env("HOME")?.join("Library/Application Support/qBittorrent")
            }
            Client::QBittorrent => {
                env("XDG_DATA_HOME").or_else(|| Some(env("HOME")?.join(".local/share")))?.join("qBittorrent")
            }
            Client::UTorrent if cfg!(windows) => env("APPDATA")?.join("uTorrent"),
            Client::UTorrent => return None,
        };
        let dir = match self {
            Client::QBittorrent => dir.join("BT_backup"),
            Client::UTorrent => dir,
        };
        self.holds_torrents(&dir).then_some(dir)
    }

    /// Whether `dir` is this client's folder: `BT_backup`, or the folder with `resume.dat`.
    /// For qBittorrent its parent (the data folder) is accepted too, see [`Client::source_dir`].
    pub fn holds_torrents(self, dir: &Path) -> bool {
        match self {
            Client::QBittorrent => std::fs::read_dir(dir).is_ok_and(|mut d| {
                d.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "fastresume")))
            }),
            Client::UTorrent => dir.join("resume.dat").is_file(),
        }
    }

    /// The folder to read when the user picked `dir`: qBittorrent's data folder leads to its
    /// `BT_backup`.
    pub fn source_dir(self, dir: &Path) -> PathBuf {
        let backup = dir.join("BT_backup");
        if self == Client::QBittorrent && !self.holds_torrents(dir) && self.holds_torrents(&backup) {
            backup
        } else {
            dir.to_owned()
        }
    }

    /// Whether the client runs now (for this user or another): its files would be rewritten
    /// when it exits. Not known on macOS and the BSDs (`false`).
    pub fn is_running(self) -> bool {
        let names: &[&str] = match self {
            Client::QBittorrent => &["qbittorrent", "qbittorrent-nox"],
            Client::UTorrent => &["utorrent"],
        };
        running_processes().iter().any(|p| {
            let p = p.to_ascii_lowercase();
            let p = p.strip_suffix(".exe").unwrap_or(&p);
            names.contains(&p)
        })
    }
}

/// A torrent ready to add.
#[derive(Debug)]
pub struct Import {
    pub name: String,
    /// [`InfoHashes::key_hex`]: the frontends' preference keys (`torrent.<key>.label`).
    pub key: String,
    pub label: String,
    /// µTorrent's Paused rather than Stopped (the frontend's `torrent.<key>.paused`).
    pub user_paused: bool,
    /// The torrent's own seeding goal (`torrent.<key>.seed_ratio` and `.seed_time`), when the
    /// source client had one.
    pub seed_goal: Option<SeedGoal>,
    pub params: AddTorrentParams,
}

/// A torrent that cannot be imported, with the reason (translated).
#[derive(Debug, PartialEq, Eq)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Found {
    pub torrents: Vec<Import>,
    pub skipped: Vec<Skipped>,
}

/// Reads the client's torrents from `dir` (see [`Client::source_dir`]). `Err` when the folder
/// or `resume.dat` cannot be read at all.
pub fn read(client: Client, dir: &Path) -> Result<Found, String> {
    let dir = client.source_dir(dir);
    let mut found = match client {
        Client::QBittorrent => read_qbittorrent(&dir)?,
        Client::UTorrent => read_utorrent(&dir)?,
    };
    found.torrents.sort_by(|a, b| crate::columns::natural_cmp(&a.name, &b.name));
    Ok(found)
}

/// Writes the labels, pause marks and seeding goals of the torrents not yet in the list (`known` tells by
/// [`Import::key`]) and returns the ones to add, with [`TorrentFlags::DUPLICATE_IS_ERROR`] set;
/// the others go to `found.skipped`. The caller saves `prefs` and adds the torrents.
pub fn prepare(found: &mut Found, prefs: &mut Prefs, known: impl Fn(&str) -> bool) -> Vec<Import> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for mut t in std::mem::take(&mut found.torrents) {
        if known(&t.key) || !seen.insert(t.key.clone()) {
            found
                .skipped
                .push(Skipped { name: t.name, reason: crate::lang::text(S::ImportInList).to_owned() });
            continue;
        }
        if !t.label.is_empty() {
            prefs.set(&format!("torrent.{}.label", t.key), &t.label);
        }
        if t.user_paused {
            prefs.set_bool(&format!("torrent.{}.paused", t.key), true);
        }
        if t.seed_goal.is_some() {
            SeedGoal::store_override(prefs, &t.key, t.seed_goal);
        }
        t.params.flags.insert(TorrentFlags::DUPLICATE_IS_ERROR);
        out.push(t);
    }
    out
}

/// File > Import's first step, the same in every GUI.
pub enum Begin {
    /// Show this error and stop.
    Refuse(String),
    /// The client's usual folder is not there: let the user pick one (the text is the
    /// picker's title), then call [`begin`] again with it.
    PickFolder(String),
    /// Ask this yes/no question; on yes call [`finish`].
    Confirm(String, Found),
}

/// Refuses while the client runs, finds its folder (`picked`, or its usual one) and reads it.
pub fn begin(client: Client, picked: Option<&Path>) -> Begin {
    let name = client.name();
    if client.is_running() {
        return Begin::Refuse(crate::tr!(ImportRunning, client = name));
    }
    let dir = match picked.map(Path::to_owned).or_else(|| client.default_dir()) {
        Some(d) => d,
        None => return Begin::PickFolder(crate::tr!(ImportPickFolder, client = name)),
    };
    match read(client, &dir) {
        Err(e) => Begin::Refuse(e),
        Ok(found) if found.torrents.is_empty() => {
            Begin::Refuse(crate::tr!(ImportNoneFound, client = name, dir = dir.display()))
        }
        Ok(found) => {
            let mut text = crate::tr!(ImportConfirm, count = found.torrents.len(), client = name);
            text.push_str("\n\n");
            text.push_str(&save_folders(&found));
            Begin::Confirm(text, found)
        }
    }
}

/// The folders the torrents would save to, one per line (at most [`MAX_FOLDERS_SHOWN`]): the
/// confirm question shows them, since a folder copied from elsewhere decides where data goes.
fn save_folders(found: &Found) -> String {
    let folders: std::collections::BTreeSet<&str> =
        found.torrents.iter().map(|t| t.params.save_path.as_str()).collect();
    let mut lines: Vec<String> = folders.iter().take(MAX_FOLDERS_SHOWN).map(|f| (*f).to_owned()).collect();
    if folders.len() > MAX_FOLDERS_SHOWN {
        lines.push(format!("… (+{})", folders.len() - MAX_FOLDERS_SHOWN));
    }
    lines.join("\n")
}

const MAX_FOLDERS_SHOWN: usize = 12;

/// What to do after the user said yes.
pub struct Finished {
    /// Add these (with [`TorrentFlags::DUPLICATE_IS_ERROR`]), after saving the preferences.
    pub add: Vec<AddTorrentParams>,
    /// Logger lines: each torrent imported or left out.
    pub log: Vec<String>,
    /// The message to show.
    pub summary: String,
}

/// Writes the labels and pause marks to `prefs` (the caller saves them) and says what to add;
/// `known` tells whether a torrent ([`Import::key`]) is in the list already.
pub fn finish(client: Client, mut found: Found, prefs: &mut Prefs, known: impl Fn(&str) -> bool) -> Finished {
    let name = client.name();
    let add = prepare(&mut found, prefs, known);
    let mut log: Vec<String> =
        add.iter().map(|t| crate::tr!(LogImported, client = name, name = t.name)).collect();
    log.extend(
        found
            .skipped
            .iter()
            .map(|s| crate::tr!(LogImportSkipped, client = name, name = s.name, reason = s.reason)),
    );
    let mut summary = crate::tr!(ImportDone, added = add.len(), client = name);
    if !found.skipped.is_empty() {
        summary = format!("{summary} {}", crate::tr!(ImportLeftOut, skipped = found.skipped.len()));
    }
    Finished { add: add.into_iter().map(|t| t.params).collect(), log, summary }
}

fn bad_record() -> String {
    crate::tr!(ImportBadRecord).to_owned()
}

fn bad_torrent(e: impl std::fmt::Display) -> String {
    crate::tr!(ImportBadTorrent, error = e)
}

/// The `.torrent`'s info dictionary and the keys outside it a resume record keeps.
struct Metadata {
    info: TorrentInfo,
    /// `comment`, `created by`, `creation date`, `url-list`, `httpseeds`, as in the file.
    outer: Vec<(&'static str, Entry)>,
}

fn load_torrent(path: &Path) -> Result<Metadata, String> {
    let bytes = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            crate::tr!(ImportNoTorrentFile).to_owned()
        } else {
            bad_torrent(e)
        }
    })?;
    let info = TorrentInfo::from_bytes(&bytes).map_err(bad_torrent)?;
    let doc = bdecode(&bytes).map_err(|e| bad_torrent(e.kind.message()))?;
    let root = doc.root().ok_or_else(|| bad_torrent("empty"))?;
    let outer = ["comment", "created by", "creation date", "url-list", "httpseeds"]
        .into_iter()
        .filter_map(|k| Some((k, Entry::from_node(root.dict_find(k)?))))
        .collect();
    Ok(Metadata { info, outer })
}

/// What an imported record may not decide (SEC-16): a folder copied from elsewhere could claim
/// pieces of files it never had (and get a private file seeded, if it only exists and is long
/// enough), or map files to any path on this computer. Claimed pieces (and file-version 2's
/// `verified` pieces) are dropped, so the engine checks the files, and every mapped path is
/// cleaned like a torrent's file names and stays inside the save folder (a file µTorrent
/// relocated elsewhere is downloaded again there).
fn distrust(rec: &mut Entry) {
    let d = rec.dict_mut();
    d.remove(b"pieces".as_slice());
    d.remove(b"verified".as_slice());
    d.remove(b"unfinished".as_slice());
    if let Some(Entry::List(mapped)) = d.get_mut(b"mapped_files".as_slice()) {
        for m in mapped {
            if let Entry::Bytes(path) = m {
                *path = inside_save_folder(path).into_bytes();
            }
        }
    }
}

/// `path` as a relative path below the save folder: each element sanitized, `.`, `..`, roots and
/// drive letters gone (`C:` becomes `C_`).
fn inside_save_folder(path: &[u8]) -> String {
    let mut out = String::new();
    for part in path.split(|&c| c == b'/' || c == b'\\').filter(|p| !p.is_empty()) {
        sanitize_append_path_element(&mut out, part);
    }
    out
}

/// A µTorrent path split into its folder and last part, as `Path::parent` and `file_name` do on
/// Windows. µTorrent writes Windows paths, which `Path` on Unix does not split at `\`.
fn split_windows_path(path: &str) -> (String, String) {
    let path = path.trim_end_matches(['/', '\\']);
    let Some(cut) = path.rfind(['/', '\\']) else { return (String::new(), path.to_owned()) };
    let mut folder = path[..cut].to_owned();
    // a root keeps its separator: `D:\x` is in `D:\`, `\x` in `\`
    if folder.is_empty() || folder.ends_with(':') {
        folder.push_str(&path[cut..=cut]);
    }
    (folder, path[cut + 1..].to_owned())
}

/// The record as bytes with the `.torrent`'s info dictionary as it was (so the info-hash
/// holds) and its outer keys where the record has none; see [`distrust`] for what it loses.
fn finish_record(mut rec: Entry, meta: &Metadata) -> Vec<u8> {
    distrust(&mut rec);
    // a resume record uses the .torrent's own key names for these
    for (key, v) in &meta.outer {
        let have = rec.get(key).is_some_and(|e| !matches!(e, Entry::List(l) if l.is_empty()));
        if !have {
            *rec.key(key) = v.clone();
        }
    }
    *rec.key("info") = Entry::Preformatted(meta.info.info_section().to_vec());
    rec.bencode()
}

fn params(record: Vec<u8>, save_path: &str) -> AddTorrentParams {
    AddTorrentParams::new(TorrentSource::Resume(record), save_path)
}

// ---- qBittorrent

fn read_qbittorrent(dir: &Path) -> Result<Found, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut found = Found::default();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "fastresume") {
            continue;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match qbittorrent_torrent(&path) {
            Ok(t) => found.torrents.push(t),
            Err((name, reason)) => found.skipped.push(Skipped { name: name.unwrap_or(stem), reason }),
        }
    }
    Ok(found)
}

/// One `BT_backup` torrent; on failure its name if known and the reason.
fn qbittorrent_torrent(path: &Path) -> Result<Import, (Option<String>, String)> {
    let bytes = std::fs::read(path).map_err(|_| (None, bad_record()))?;
    let doc = bdecode(&bytes).map_err(|_| (None, bad_record()))?;
    let root = doc.root().filter(Node::is_dict).ok_or((None, bad_record()))?;
    let mut rec = Entry::from_node(root);
    let qbt_name = root.dict_find_str("qBt-name").unwrap_or_default().to_owned();
    let save_path = root.dict_find_str("save_path").unwrap_or_default().to_owned();
    let tag = root.dict_find_list("qBt-tags").and_then(|l| l.list_at(0)).and_then(|n| n.str_value());
    let label = root.dict_find_str("qBt-category").filter(|c| !c.is_empty()).or(tag).unwrap_or_default();
    let seed_goal = qbittorrent_goal(root);
    // only libtorrent's keys go on; the engine ignores the rest anyway
    rec.dict_mut().retain(|k, _| !k.starts_with(b"qBt-"));

    let meta = match load_torrent(&path.with_extension("torrent")) {
        Ok(m) => m,
        Err(reason) => {
            // a magnet still without metadata: its hash, name and save path are enough
            let name = (!qbt_name.is_empty()).then(|| qbt_name.clone());
            let hashes = record_hashes(root).ok_or((name.clone(), reason))?;
            let mut p = AddTorrentParams::new(TorrentSource::InfoHash(hashes), &save_path);
            p.name = name.clone();
            p.trackers = root
                .dict_find_list("trackers")
                .map(|tiers| {
                    tiers
                        .list_iter()
                        .flat_map(|t| t.list_iter())
                        .filter_map(|u| u.str_value())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            // the full record carries these flags itself
            let (paused, auto_managed) = qbittorrent_flags(root);
            p.flags.set(TorrentFlags::PAUSED, paused);
            p.flags.set(TorrentFlags::AUTO_MANAGED, auto_managed);
            return Ok(Import {
                name: name.unwrap_or_else(|| hashes.key_hex()),
                key: hashes.key_hex(),
                label: label.to_owned(),
                user_paused: false,
                seed_goal,
                params: p,
            });
        }
    };
    let hashes = meta.info.info_hashes();
    let name = if qbt_name.is_empty() { meta.info.name().to_owned() } else { qbt_name };
    *rec.key("name") = Entry::Bytes(name.clone().into_bytes());
    Ok(Import {
        key: hashes.key_hex(),
        label: label.to_owned(),
        user_paused: false,
        seed_goal,
        params: params(finish_record(rec, &meta), &save_path),
        name,
    })
}

/// A qBittorrent torrent's run state as its own loader reads it, in 4.x and 5.x alike (no key
/// of its own): `paused` without `auto_managed` is stopped, neither is forced, else queued
/// (paused and auto-managed: the queue decides). 4.x calls the stopped state "Paused", 5.x
/// "Stopped"; it is this list's Stopped (paused outside the queue), not µTorrent's Pause, so
/// no pause mark is written for it.
fn qbittorrent_flags(root: Node<'_>) -> (bool, bool) {
    let flag = |k: &str| root.dict_find_int_value(k).unwrap_or(0) != 0;
    let (paused, auto_managed) = (flag("paused"), flag("auto_managed"));
    if paused && !auto_managed {
        (true, false)
    } else if !paused && !auto_managed {
        (false, false)
    } else {
        (true, true)
    }
}

/// A qBittorrent share limit: its "use the global one" (-2), "none" (-1), or a value.
#[derive(Clone, Copy, PartialEq, Debug)]
enum QbtLimit {
    Global,
    None,
    Value(f64),
}

impl QbtLimit {
    fn of(v: f64) -> QbtLimit {
        if v == -1.0 {
            QbtLimit::None
        } else if v < 0.0 || !v.is_finite() {
            // -2, or anything else qBittorrent would not write
            QbtLimit::Global
        } else {
            QbtLimit::Value(v)
        }
    }

    /// This goal's target: `None` for the global one, 0 (off) for none, else the value; a
    /// limit of 0 (stop at once) becomes the smallest target, since 0 switches ours off.
    fn target(self) -> Option<u32> {
        match self {
            QbtLimit::Global => None,
            QbtLimit::None => Some(0),
            QbtLimit::Value(v) => Some((v.round() as u32).max(1)),
        }
    }
}

/// The torrent's own seeding goal from `qBt-ratioLimit` (the ratio ×1000 as an integer, or
/// a decimal string, which older files have) and `qBt-seedingTimeLimit` (minutes); both -2 for
/// "use the global limit" (`None`: our default goal applies), -1 for "no limit". qBittorrent's
/// inactive seeding time and what it does at the limit (stop, remove, super-seed) have no
/// equivalent: our goal stops the torrent or limits its upload as the Preferences say.
fn qbittorrent_goal(root: Node<'_>) -> Option<SeedGoal> {
    let ratio = match root.dict_find_str("qBt-ratioLimit") {
        Some(s) => s.trim().parse::<f64>().map_or(QbtLimit::Global, QbtLimit::of),
        None => root
            .dict_find_int_value("qBt-ratioLimit")
            .map_or(QbtLimit::Global, |v| QbtLimit::of(v as f64 / 1000.0)),
    };
    let ratio = match ratio {
        QbtLimit::Value(r) => QbtLimit::Value(r * 100.0),
        other => other,
    };
    let time =
        root.dict_find_int_value("qBt-seedingTimeLimit").map_or(QbtLimit::Global, |v| QbtLimit::of(v as f64));
    let (ratio_pct, minutes) = match (ratio.target(), time.target()) {
        (None, None) => return None,
        // one global: that side has no target here (our goal cannot name qBittorrent's)
        (r, t) => (r.unwrap_or(0), t.unwrap_or(0)),
    };
    // qBittorrent acts when EITHER limit is reached ("MatchAny", the only behaviour before
    // 5.0 and its default since); ours needs BOTH. With both set only the ratio is taken, the
    // target qBittorrent users set most. 5.x's "MatchAll" mode is ours: both are taken.
    let all = root.dict_find_str("qBt-shareLimitsMode") == Some("MatchAll");
    let minutes = if ratio_pct > 0 && minutes > 0 && !all { 0 } else { minutes };
    Some(SeedGoal { ratio_pct, minutes })
}

fn record_hashes(root: Node<'_>) -> Option<InfoHashes> {
    let v1 = root.dict_find_string_value("info-hash").and_then(|b| <[u8; 20]>::try_from(b).ok());
    let v2 = root.dict_find_string_value("info-hash2").and_then(|b| <[u8; 32]>::try_from(b).ok());
    let hashes = InfoHashes {
        v1: v1.map(Sha1Hash).filter(|h| h.0 != [0; 20]),
        v2: v2.map(patorrent_core::Sha256Hash).filter(|h| h.0 != [0; 32]),
    };
    (hashes.v1.is_some() || hashes.v2.is_some()).then_some(hashes)
}

// ---- µTorrent

fn read_utorrent(dir: &Path) -> Result<Found, String> {
    let path = dir.join("resume.dat");
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc = bdecode(&bytes).map_err(|e| format!("{}: {}", path.display(), e.kind.message()))?;
    let root =
        doc.root().filter(Node::is_dict).ok_or_else(|| format!("{}: {}", path.display(), bad_record()))?;
    let mut found = Found::default();
    for (key, rec) in root.dict_iter() {
        // `.fileguard` and `rec` (µTorrent 3) are not torrents
        if !rec.is_dict() || key.starts_with(b".") || key == b"rec" {
            continue;
        }
        let file = String::from_utf8_lossy(key).into_owned();
        match utorrent_torrent(dir, &file, rec) {
            Ok(t) => found.torrents.push(t),
            Err(reason) => {
                let name = rec.dict_find_str("caption").filter(|c| !c.is_empty()).map(str::to_owned);
                let name = name.unwrap_or_else(|| file.trim_end_matches(".torrent").to_owned());
                found.skipped.push(Skipped { name, reason });
            }
        }
    }
    Ok(found)
}

/// µTorrent's `prio` byte as a libtorrent priority.
fn utorrent_priority(b: u8) -> u8 {
    match b {
        0 => 0,
        b if b & 0x80 != 0 => 0,
        1..=7 => 1,
        8 => 4,
        _ => 7,
    }
}

fn utorrent_torrent(dir: &Path, file: &str, r: Node<'_>) -> Result<Import, String> {
    let int = |k: &str| r.dict_find_int_value(k).unwrap_or(0);
    let path = r.dict_find_str("path").unwrap_or_default();
    let caption = r.dict_find_str("caption").unwrap_or_default();
    let label = r
        .dict_find_str("label")
        .filter(|l| !l.is_empty())
        .or_else(|| r.dict_find_list("labels").and_then(|l| l.list_at(0)).and_then(|n| n.str_value()))
        .unwrap_or_default()
        .to_owned();
    // 0 Stopped, 1 Force Started, 2 Started, 3 Paused
    let started = int("started");
    let (paused, auto_managed) = match started {
        0 | 3 => (true, false),
        1 => (false, false),
        _ => (false, true),
    };
    let trackers: Vec<Entry> = {
        let mut tiers: Vec<Vec<Entry>> = vec![Vec::new()];
        for u in r.dict_find_list("trackers").map(|l| l.list_iter().collect::<Vec<_>>()).unwrap_or_default() {
            match u.str_value().map(str::trim) {
                Some("") if !tiers.last().is_some_and(Vec::is_empty) => tiers.push(Vec::new()),
                Some("") | None => {}
                Some(url) => tiers.last_mut().expect("one").push(Entry::Bytes(url.as_bytes().to_vec())),
            }
        }
        tiers.into_iter().filter(|t| !t.is_empty()).map(Entry::List).collect()
    };

    let torrent = Path::new(file);
    let meta = match load_torrent(&if torrent.is_absolute() { torrent.to_owned() } else { dir.join(torrent) })
    {
        Ok(m) => m,
        Err(reason) => {
            // a magnet still without metadata
            let v1 =
                r.dict_find_string_value("info").and_then(|b| <[u8; 20]>::try_from(b).ok()).ok_or(reason)?;
            let hashes = InfoHashes { v1: Some(Sha1Hash(v1)), v2: None };
            let mut p = AddTorrentParams::new(TorrentSource::InfoHash(hashes), path);
            p.name = (!caption.is_empty()).then(|| caption.to_owned());
            for (tier, urls) in trackers.iter().enumerate() {
                for u in urls.as_list().unwrap_or_default() {
                    p.trackers.push(u.as_str().unwrap_or_default().to_owned());
                    p.tracker_tiers.push(tier as u32);
                }
            }
            if paused {
                p.flags.insert(TorrentFlags::PAUSED);
                p.flags.remove(TorrentFlags::AUTO_MANAGED);
            }
            return Ok(Import {
                name: p.name.clone().unwrap_or_else(|| hashes.key_hex()),
                key: hashes.key_hex(),
                label,
                user_paused: started == 3,
                seed_goal: None,
                params: p,
            });
        }
    };
    let ti = &meta.info;
    let hashes = ti.info_hashes();
    let files = ti.files();
    // a torrent with a folder, even of one file
    let multi = files.file_path(0, "") != files.file_name(0);

    // `path` is the torrent's own folder (multi-file) or file: its parent is the save path,
    // and a different last part means µTorrent renamed it
    let (save_path, stored) = split_windows_path(path);
    let mut mapped: BTreeMap<u32, String> = BTreeMap::new();
    let root_name = if multi { ti.name() } else { files.file_name(0) };
    if !stored.is_empty() && stored != root_name {
        for f in 0..files.num_files() {
            let p = files.file_path(f, "").replace('\\', "/");
            let rest =
                if multi { p.strip_prefix(ti.name()).unwrap_or(&p).trim_start_matches('/') } else { "" };
            mapped
                .insert(f as u32, if rest.is_empty() { stored.clone() } else { format!("{stored}/{rest}") });
        }
    }
    // renamed files: [index, name], the name relative to the torrent's folder or absolute
    for t in r.dict_find_list("targets").map(|l| l.list_iter().collect::<Vec<_>>()).unwrap_or_default() {
        let (Some(i), Some(n)) = (t.list_int_value_at(0), t.list_string_value_at(1)) else { continue };
        let n = String::from_utf8_lossy(n).into_owned();
        let Ok(i) = u32::try_from(i) else { continue };
        if (i as usize) < files.num_files() {
            let rel = if Path::new(&n).is_absolute() || !multi { n } else { format!("{stored}/{n}") };
            mapped.insert(i, rel);
        }
    }

    let n = ti.num_pieces() as usize;
    let have = r.dict_find_string_value("have").unwrap_or_default();
    let pieces: Vec<u8> = (0..n).map(|i| have.get(i / 8).map_or(0, |b| (b >> (i % 8)) & 1)).collect();
    let prio = r.dict_find_string_value("prio").unwrap_or_default();

    let mut rec = Entry::new_dict();
    let int_e = Entry::Int;
    let s = |v: &str| Entry::Bytes(v.as_bytes().to_vec());
    *rec.key("file-format") = s("libtorrent resume file");
    *rec.key("file-version") = int_e(1);
    if let Some(v1) = hashes.v1 {
        *rec.key("info-hash") = Entry::Bytes(v1.0.to_vec());
    }
    let name = if caption.is_empty() { ti.name() } else { caption }.to_owned();
    *rec.key("name") = s(&name);
    *rec.key("save_path") = s(&save_path);
    if have.len() * 8 >= n {
        *rec.key("pieces") = Entry::Bytes(pieces);
    }
    *rec.key("total_uploaded") = int_e(int("uploaded"));
    *rec.key("total_downloaded") = int_e(int("downloaded"));
    *rec.key("active_time") = int_e(int("runtime"));
    *rec.key("seeding_time") = int_e(int("seedtime"));
    *rec.key("finished_time") = int_e(int("seedtime"));
    *rec.key("added_time") = int_e(int("added_on"));
    *rec.key("completed_time") = int_e(int("completed_on"));
    *rec.key("paused") = int_e(paused as i64);
    *rec.key("auto_managed") = int_e(auto_managed as i64);
    *rec.key("super_seeding") = int_e((int("superseed") != 0) as i64);
    if !prio.is_empty() {
        *rec.key("file_priority") = Entry::List(
            prio.iter().take(files.num_files()).map(|&b| int_e(utorrent_priority(b) as i64)).collect(),
        );
    }
    if let Some(&last) = mapped.keys().next_back() {
        *rec.key("mapped_files") =
            Entry::List((0..=last).map(|i| s(mapped.get(&i).map_or("", String::as_str))).collect());
    }
    *rec.key("trackers") = Entry::List(trackers);
    *rec.key("upload_rate_limit") = int_e(int("upspeed").max(0));
    *rec.key("download_rate_limit") = int_e(int("downspeed").max(0));
    Ok(Import {
        key: hashes.key_hex(),
        label,
        user_paused: started == 3,
        seed_goal: None,
        params: params(finish_record(rec, &meta), &save_path),
        name,
    })
}

// ---- Running processes

#[cfg(windows)]
fn running_processes() -> Vec<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: a snapshot handle closed below; the entry's size is set before the first call.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push(String::from_utf16_lossy(&e.szExeFile[..len]));
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(target_os = "linux")]
fn running_processes() -> Vec<String> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten()
        .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|e| std::fs::read_to_string(e.path().join("comm")).ok())
        .map(|s| s.trim_end().to_owned())
        .collect()
}

#[cfg(not(any(windows, target_os = "linux")))]
fn running_processes() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use patorrent_core::metainfo::{CreateFlags, CreateTorrent, FileFlags, FileStorage};

    /// A two-file torrent of 3 pieces (16 KiB each): `dir/<name>/a.bin`, `dir/<name>/b.bin`.
    fn torrent(name: &str) -> (Vec<u8>, InfoHashes) {
        let mut fs = FileStorage::new();
        for f in ["a", "b"] {
            fs.add_file(&format!("{name}/{f}.bin"), 20_000, FileFlags::empty(), 0, "", None).unwrap();
        }
        let mut ct = CreateTorrent::new(fs, 16384, CreateFlags::V1_ONLY).unwrap();
        for p in 0..ct.num_pieces() {
            ct.set_hash(p, Sha1Hash([p as u8 + 1; 20])).unwrap();
        }
        ct.set_comment("hello");
        let bytes = ct.generate_buf().unwrap();
        let h = TorrentInfo::from_bytes(&bytes).unwrap().info_hashes();
        (bytes, h)
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("patorrent-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn decoded(t: &Import) -> Entry {
        let TorrentSource::Resume(b) = &t.params.source else { panic!("not a resume record") };
        Entry::from_node(bdecode(b).unwrap().root().unwrap())
    }

    #[test]
    fn utorrent_paths_split_as_on_windows() {
        let split = split_windows_path;
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(split(r"D:\dl\Show"), pair(r"D:\dl", "Show"));
        assert_eq!(split(r"D:\dl\Show\"), pair(r"D:\dl", "Show"));
        assert_eq!(split(r"D:\Show"), pair(r"D:\", "Show"));
        assert_eq!(split("/home/x/Show"), pair("/home/x", "Show"));
        assert_eq!(split(r"\Show"), pair(r"\", "Show"));
        assert_eq!(split("Show"), pair("", "Show"));
    }

    #[test]
    fn utorrent_resume_dat() {
        let dir = temp("ut");
        let (bytes, hashes) = torrent("Show");
        std::fs::write(dir.join("Show.torrent"), &bytes).unwrap();
        let mut rec = Entry::new_dict();
        *rec.key("caption") = Entry::Bytes(b"My Show".to_vec());
        *rec.key("path") = Entry::Bytes(b"D:\\dl\\Show renamed".to_vec());
        *rec.key("label") = Entry::Bytes(b"tv".to_vec());
        *rec.key("have") = Entry::Bytes(vec![0b101]);
        *rec.key("prio") = Entry::Bytes(vec![0x08, 0x80]);
        *rec.key("trackers") = Entry::List(
            ["http://a/announce", "http://b/announce", "", "udp://c:1/announce"]
                .iter()
                .map(|u| Entry::Bytes(u.as_bytes().to_vec()))
                .collect(),
        );
        *rec.key("started") = Entry::Int(3);
        *rec.key("uploaded") = Entry::Int(1234);
        *rec.key("added_on") = Entry::Int(1_700_000_000);
        let mut magnet = Entry::new_dict();
        *magnet.key("info") = Entry::Bytes(vec![7; 20]);
        *magnet.key("caption") = Entry::Bytes(b"Pending magnet".to_vec());
        *magnet.key("path") = Entry::Bytes(b"D:\\dl\\Pending magnet".to_vec());
        *magnet.key("started") = Entry::Int(2);
        let mut root = Entry::new_dict();
        *root.key(".fileguard") = Entry::Bytes(b"X".to_vec());
        *root.key("Show.torrent") = rec;
        *root.key("Pending.torrent") = magnet;
        *root.key("Gone.torrent") = Entry::new_dict();
        std::fs::write(dir.join("resume.dat"), root.bencode()).unwrap();

        assert!(Client::UTorrent.holds_torrents(&dir));
        let found = read(Client::UTorrent, &dir).unwrap();
        assert_eq!(found.skipped, [Skipped { name: "Gone".into(), reason: tr_missing() }]);
        assert_eq!(found.torrents.len(), 2);
        let show = &found.torrents[0];
        assert_eq!((show.name.as_str(), show.label.as_str(), show.user_paused), ("My Show", "tv", true));
        assert_eq!(show.key, hashes.key_hex());
        let e = decoded(show);
        assert_eq!(e.get("save_path").unwrap().as_str(), Some("D:\\dl"));
        assert!(e.get("pieces").is_none(), "claimed pieces are checked, not taken (SEC-16)");
        let prio: Vec<_> =
            e.get("file_priority").unwrap().as_list().unwrap().iter().map(|p| p.as_int()).collect();
        assert_eq!(prio, [Some(4), Some(0)]);
        let tiers = e.get("trackers").unwrap().as_list().unwrap();
        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[0].as_list().unwrap().len(), 2);
        let mapped: Vec<_> =
            e.get("mapped_files").unwrap().as_list().unwrap().iter().map(|m| m.as_str()).collect();
        assert_eq!(mapped, [Some("Show renamed/a.bin"), Some("Show renamed/b.bin")]);
        assert_eq!(
            (e.get("paused").unwrap().as_int(), e.get("auto_managed").unwrap().as_int()),
            (Some(1), Some(0))
        );
        assert_eq!(e.get("total_uploaded").unwrap().as_int(), Some(1234));
        assert_eq!(e.get("comment").unwrap().as_str(), Some("hello"));
        // the info dictionary exactly as in the .torrent: the hash holds
        let info = TorrentInfo::from_info_section(
            e.get("info").map(Entry::bencode).unwrap().as_slice(),
            Default::default(),
        )
        .unwrap();
        assert_eq!(info.info_hashes(), hashes);

        let m = &found.torrents[1];
        assert!(matches!(m.params.source, TorrentSource::InfoHash(h) if h.v1 == Some(Sha1Hash([7; 20]))));
        assert_eq!(
            (m.name.as_str(), m.params.save_path.as_str()),
            ("Pending magnet", "D:\\dl\\Pending magnet")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tr_missing() -> String {
        crate::tr!(ImportNoTorrentFile).to_owned()
    }

    #[test]
    fn qbittorrent_bt_backup() {
        let data = temp("qbt");
        let dir = data.join("BT_backup");
        std::fs::create_dir_all(&dir).unwrap();
        let (bytes, hashes) = torrent("Album");
        let hex = hashes.key_hex();
        std::fs::write(dir.join(format!("{hex}.torrent")), &bytes).unwrap();
        let mut rec = Entry::new_dict();
        *rec.key("file-format") = Entry::Bytes(b"libtorrent resume file".to_vec());
        *rec.key("file-version") = Entry::Int(1);
        *rec.key("info-hash") = Entry::Bytes(hashes.v1.unwrap().0.to_vec());
        *rec.key("save_path") = Entry::Bytes(b"/data/music".to_vec());
        *rec.key("pieces") = Entry::Bytes(vec![1, 1, 0]);
        *rec.key("paused") = Entry::Int(1);
        *rec.key("auto_managed") = Entry::Int(0);
        *rec.key("qBt-category") = Entry::Bytes(Vec::new());
        *rec.key("qBt-tags") = Entry::List(vec![Entry::Bytes(b"flac".to_vec())]);
        *rec.key("qBt-name") = Entry::Bytes(b"Best Album".to_vec());
        *rec.key("qBt-ratioLimit") = Entry::Int(1500);
        *rec.key("qBt-seedingTimeLimit") = Entry::Int(-2);
        std::fs::write(dir.join(format!("{hex}.fastresume")), rec.bencode()).unwrap();
        // a magnet without metadata, queued, never to stop seeding
        let mut m = Entry::new_dict();
        *m.key("info-hash") = Entry::Bytes(vec![9; 20]);
        *m.key("save_path") = Entry::Bytes(b"/data".to_vec());
        *m.key("qBt-category") = Entry::Bytes(b"linux".to_vec());
        *m.key("paused") = Entry::Int(1);
        *m.key("auto_managed") = Entry::Int(1);
        *m.key("qBt-ratioLimit") = Entry::Int(-1000);
        *m.key("qBt-seedingTimeLimit") = Entry::Int(-1);
        std::fs::write(dir.join(format!("{}.fastresume", "09".repeat(20))), m.bencode()).unwrap();
        std::fs::write(dir.join("broken.fastresume"), b"d3:foo").unwrap();

        // the data folder leads to BT_backup
        let mut found = read(Client::QBittorrent, &data).unwrap();
        assert_eq!(
            found.skipped,
            [Skipped { name: "broken".into(), reason: crate::tr!(ImportBadRecord).into() }]
        );
        assert_eq!(found.torrents.len(), 2);
        // sorted by name: the magnet's name is its hash
        let (magnet, album) = (&found.torrents[0], &found.torrents[1]);
        assert_eq!((album.name.as_str(), album.label.as_str()), ("Best Album", "flac"));
        assert_eq!(album.params.save_path, "/data/music");
        let e = decoded(album);
        assert!(e.get("qBt-name").is_none());
        assert_eq!(e.get("name").unwrap().as_str(), Some("Best Album"));
        assert!(e.get("pieces").is_none());
        assert_eq!(magnet.label, "linux");
        assert!(matches!(magnet.params.source, TorrentSource::InfoHash(_)));
        // stopped stays stopped (in the record), queued stays in the queue; no pause marks
        assert_eq!(
            (e.get("paused").unwrap().as_int(), e.get("auto_managed").unwrap().as_int()),
            (Some(1), Some(0))
        );
        let queued = TorrentFlags::PAUSED | TorrentFlags::AUTO_MANAGED;
        assert!(magnet.params.flags.contains(queued));
        assert!(!album.user_paused && !magnet.user_paused);
        // the share limits as the torrents' own seeding goals
        assert_eq!(album.seed_goal, Some(SeedGoal { ratio_pct: 150, minutes: 0 }));
        assert_eq!(magnet.seed_goal, Some(SeedGoal { ratio_pct: 0, minutes: 0 }), "seed forever");

        // already in the list: left out, no label written for it
        let mut prefs = Prefs::from_text("");
        let add = prepare(&mut found, &mut prefs, |k| k == hex);
        assert_eq!(add.len(), 1);
        assert_eq!(found.skipped.len(), 2);
        assert!(prefs.get(&format!("torrent.{hex}.label")).is_none());
        assert!(SeedGoal::load_override(&prefs, &hex).is_none());
        assert_eq!(prefs.get(&format!("torrent.{}.label", "09".repeat(20))), Some("linux"));
        let goal = SeedGoal::load_override(&prefs, &"09".repeat(20));
        assert_eq!(goal, Some(SeedGoal { ratio_pct: 0, minutes: 0 }));
        assert!(prefs.get(&format!("torrent.{}.paused", "09".repeat(20))).is_none());
        assert!(add[0].params.flags.contains(TorrentFlags::DUPLICATE_IS_ERROR));
        let _ = std::fs::remove_dir_all(&data);
    }

    /// qBittorrent on libtorrent 2.1 writes file-version 2 (packed `pieces`, `verified`): the
    /// record goes on with its version, the piece claims dropped as for version 1.
    #[test]
    fn qbittorrent_file_version_2() {
        let data = temp("qbt-v2");
        let dir = data.join("BT_backup");
        std::fs::create_dir_all(&dir).unwrap();
        let (bytes, hashes) = torrent("Album");
        let hex = hashes.key_hex();
        std::fs::write(dir.join(format!("{hex}.torrent")), &bytes).unwrap();
        let mut rec = Entry::new_dict();
        *rec.key("file-format") = Entry::Bytes(b"libtorrent resume file".to_vec());
        *rec.key("file-version") = Entry::Int(2);
        *rec.key("info-hash") = Entry::Bytes(hashes.v1.unwrap().0.to_vec());
        *rec.key("save_path") = Entry::Bytes(b"/data/music".to_vec());
        *rec.key("pieces") = Entry::Bytes(vec![0b1100_0000]);
        *rec.key("verified") = Entry::Bytes(vec![0b1110_0000]);
        std::fs::write(dir.join(format!("{hex}.fastresume")), rec.bencode()).unwrap();
        let found = read(Client::QBittorrent, &data).unwrap();
        assert!(found.skipped.is_empty());
        let e = decoded(&found.torrents[0]);
        assert_eq!(e.get("file-version").and_then(Entry::as_int), Some(2));
        assert!(e.get("pieces").is_none() && e.get("verified").is_none());
        let _ = std::fs::remove_dir_all(&data);
    }

    /// SEC-16: an imported folder decides nothing outside its torrents' save folders. Mapped
    /// files (µTorrent `targets`, qBittorrent `mapped_files`) are cleaned to relative paths,
    /// claimed pieces and partial pieces go, and the confirm question lists the save folders.
    #[test]
    fn imported_paths_and_pieces_are_not_trusted() {
        let dir = temp("ut-hostile");
        let (bytes, _) = torrent("Show");
        std::fs::write(dir.join("Show.torrent"), &bytes).unwrap();
        let mut rec = Entry::new_dict();
        *rec.key("path") = Entry::Bytes(b"D:\\dl\\Show".to_vec());
        *rec.key("have") = Entry::Bytes(vec![0b111]);
        *rec.key("targets") = Entry::List(vec![
            Entry::List(vec![
                Entry::Int(0),
                Entry::Bytes(
                    br"C:\Users\x\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\x.bat"
                        .to_vec(),
                ),
            ]),
            Entry::List(vec![Entry::Int(1), Entry::Bytes(b"../../../home/x/.ssh/id_rsa".to_vec())]),
        ]);
        let mut root = Entry::new_dict();
        *root.key("Show.torrent") = rec;
        std::fs::write(dir.join("resume.dat"), root.bencode()).unwrap();
        let found = read(Client::UTorrent, &dir).unwrap();
        let e = decoded(&found.torrents[0]);
        assert!(e.get("pieces").is_none());
        let mapped: Vec<String> = e
            .get("mapped_files")
            .unwrap()
            .as_list()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_owned())
            .collect();
        for m in &mapped {
            assert!(!Path::new(m).has_root() && !m.contains(".."), "{m}");
            // `:` names a drive or a stream only on Windows; elsewhere it is a plain character
            assert!(!cfg!(windows) || !m.contains(':'), "{m}");
        }
        assert!(mapped[0].ends_with("Startup/x.bat"), "{mapped:?}");
        // relative targets are under the torrent's folder: `Show/../../../home/...`, cleaned
        assert_eq!(mapped[1], "Show/home/x/.ssh/id_rsa");

        let mut rec = Entry::new_dict();
        *rec.key("mapped_files") = Entry::List(vec![
            Entry::Bytes(b"/etc/cron.d/x".to_vec()),
            Entry::Bytes(b"\\\\attacker\\share\\x".to_vec()),
            Entry::Bytes(b"Show/a.bin".to_vec()),
        ]);
        *rec.key("pieces") = Entry::Bytes(vec![1, 1, 1]);
        *rec.key("unfinished") = Entry::List(vec![Entry::new_dict()]);
        distrust(&mut rec);
        assert!(rec.get("pieces").is_none() && rec.get("unfinished").is_none());
        let mapped: Vec<_> =
            rec.get("mapped_files").unwrap().as_list().unwrap().iter().map(|m| m.as_str()).collect();
        assert_eq!(mapped, [Some("etc/cron.d/x"), Some("attacker/share/x"), Some("Show/a.bin")]);

        assert_eq!(save_folders(&found), "D:\\dl");
        let many = Found {
            torrents: (0..20)
                .map(|i| Import {
                    name: i.to_string(),
                    key: i.to_string(),
                    label: String::new(),
                    user_paused: false,
                    seed_goal: None,
                    params: AddTorrentParams::new(
                        TorrentSource::InfoHash(InfoHashes::default()),
                        format!("/d/{i:02}"),
                    ),
                })
                .collect(),
            skipped: Vec::new(),
        };
        let text = save_folders(&many);
        assert_eq!(text.lines().count(), MAX_FOLDERS_SHOWN + 1);
        assert!(text.starts_with("/d/00\n") && text.ends_with("… (+8)"), "{text}");
    }

    /// qBittorrent's share limits (`bencoderesumedatastorage.cpp`): the ratio ×1000 (or a
    /// string), minutes, -2 global, -1 none; it stops at either limit, ours at both.
    #[test]
    fn qbittorrent_share_limits() {
        let goal = |pairs: &[(&str, Entry)]| {
            let mut d = Entry::new_dict();
            for (k, v) in pairs {
                *d.key(k) = v.clone();
            }
            let bytes = d.bencode();
            let doc = bdecode(&bytes).unwrap();
            qbittorrent_goal(doc.root().unwrap()).map(|g| (g.ratio_pct, g.minutes))
        };
        let (r, t, int) = ("qBt-ratioLimit", "qBt-seedingTimeLimit", Entry::Int);
        assert_eq!(goal(&[]), None);
        assert_eq!(goal(&[(r, int(-2000)), (t, int(-2))]), None, "the global limits");
        assert_eq!(goal(&[(r, int(1504)), (t, int(-2))]), Some((150, 0)));
        assert_eq!(goal(&[(r, Entry::Bytes(b"1.25".to_vec()))]), Some((125, 0)), "the old string");
        assert_eq!(goal(&[(r, int(-1000)), (t, int(-1))]), Some((0, 0)), "no limit: seed forever");
        assert_eq!(goal(&[(r, int(-1000)), (t, int(-2))]), Some((0, 0)));
        assert_eq!(goal(&[(r, int(-2000)), (t, int(90))]), Some((0, 90)));
        assert_eq!(goal(&[(r, int(0)), (t, int(0))]), Some((1, 0)), "0 is at once, not off");
        // either limit in qBittorrent, both in ours: the ratio only, unless 5.x matches all
        assert_eq!(goal(&[(r, int(2000)), (t, int(120))]), Some((200, 0)));
        let all = ("qBt-shareLimitsMode", Entry::Bytes(b"MatchAll".to_vec()));
        assert_eq!(goal(&[(r, int(2000)), (t, int(120)), all.clone()]), Some((200, 120)));
        let any = ("qBt-shareLimitsMode", Entry::Bytes(b"MatchAny".to_vec()));
        assert_eq!(goal(&[(r, int(2000)), (t, int(120)), any]), Some((200, 0)));

        let flags = |paused: i64, auto: i64| {
            let mut d = Entry::new_dict();
            *d.key("paused") = int(paused);
            *d.key("auto_managed") = int(auto);
            let bytes = d.bencode();
            qbittorrent_flags(bdecode(&bytes).unwrap().root().unwrap())
        };
        assert_eq!(flags(1, 0), (true, false), "stopped (4.x: paused)");
        assert_eq!(flags(0, 0), (false, false), "forced");
        assert_eq!([flags(1, 1), flags(0, 1)], [(true, true); 2], "queued");
    }

    #[test]
    fn utorrent_priorities() {
        assert_eq!([0, 0x80, 0x88, 4, 8, 12, 15].map(utorrent_priority), [0, 0, 0, 1, 4, 7, 7]);
    }
}
