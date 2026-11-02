//! "Move completed downloads" (µTorrent Preferences > Directories): moves each torrent that
//! finishes downloading into one folder, through [`TorrentHandle::move_storage`], or with
//! "Append the torrent's label" into the folder for its label under it.
//!
//! libtorrent has no such setting; qBittorrent and µTorrent do this in the client on
//! `torrent_finished_alert`. [`MoveCompleted`] is that client side, so every frontend gets the
//! same behaviour: feed it every alert.
//!
//! [`TorrentHandle::move_storage`]: crate::TorrentHandle::move_storage

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::alert::Alert;
use crate::params::MoveFlags;
use crate::session::SessionHandle;
use crate::types::TorrentId;

/// Moves finished torrents to `dir` (or, with `append_label`, to their label's folder in it).
#[derive(Debug, Clone)]
pub struct MoveCompleted {
    /// Where finished downloads go.
    pub dir: String,
    /// µTorrent's "Append the torrent's label" (`dir_add_label`): a torrent with a label goes
    /// to its folder under `dir` ([`label_dir`]), one without to `dir`. The label counts as it
    /// is when the torrent finishes; labelling it later does not move it.
    pub append_label: bool,
    pub flags: MoveFlags,
    /// µTorrent's "Only move from the default download directory": move only torrents saved
    /// here. `None` moves every finished torrent.
    pub only_from: Option<String>,
    /// Save paths from status updates.
    paths: HashMap<TorrentId, String>,
    /// Finished before a status update told us where they are.
    waiting: HashSet<TorrentId>,
}

impl MoveCompleted {
    pub fn new(dir: impl Into<String>) -> MoveCompleted {
        MoveCompleted {
            dir: dir.into(),
            append_label: false,
            flags: MoveFlags::default(),
            only_from: None,
            paths: HashMap::new(),
            waiting: HashSet::new(),
        }
    }

    /// Looks at one alert; returns the torrent it asked the session to move and where to, if
    /// any. Status updates ([`Alert::StateUpdate`]) tell it where torrents are, so the frontend
    /// should keep posting them (frontends do, for their lists). `label_of` gives a torrent's
    /// label (`None` or empty: none); it is asked only when one finishes.
    pub fn on_alert(
        &mut self,
        alert: &Alert,
        session: &SessionHandle,
        label_of: impl Fn(TorrentId) -> Option<String>,
    ) -> Option<(TorrentId, String)> {
        let target = |m: &MoveCompleted, id| m.target(label_of(id).as_deref().unwrap_or(""));
        match alert {
            Alert::StateUpdate(v) => {
                let mut moved = None;
                for st in v {
                    self.paths.insert(st.id, st.save_path.clone());
                    if self.waiting.remove(&st.id) {
                        let to = target(self, st.id);
                        if self.wants(&st.save_path, &to) {
                            self.start(st.id, &to, session);
                            moved = Some((st.id, to));
                        }
                    }
                }
                moved
            }
            Alert::StorageMoved { id, new, .. } => {
                self.paths.insert(*id, new.clone());
                None
            }
            Alert::TorrentRemoved { id, .. } => {
                self.paths.remove(id);
                self.waiting.remove(id);
                None
            }
            Alert::TorrentFinished { id } => match self.paths.get(id) {
                None if self.only_from.is_some() => {
                    self.waiting.insert(*id);
                    None
                }
                path => {
                    let to = target(self, *id);
                    if path.is_some_and(|p| !self.wants(p, &to)) {
                        return None;
                    }
                    self.start(*id, &to, session);
                    Some((*id, to))
                }
            },
            _ => None,
        }
    }

    /// Where a finished torrent with `label` (empty: none) goes: `dir`, or with `append_label`
    /// its label's folder under `dir` ([`label_dir`]; `dir` when the label names none).
    pub fn target(&self, label: &str) -> String {
        let sub = if self.append_label { label_dir(label) } else { PathBuf::new() };
        if sub.as_os_str().is_empty() {
            return self.dir.clone();
        }
        Path::new(&self.dir).join(sub).to_string_lossy().into_owned()
    }

    /// Whether a torrent saved in `save_path` is moved to `target` ([`MoveCompleted::target`])
    /// when it finishes: it is not there yet, and it is in `only_from` if that is set.
    pub fn wants(&self, save_path: &str, target: &str) -> bool {
        !same_dir(save_path, target) && self.only_from.as_deref().is_none_or(|f| same_dir(save_path, f))
    }

    fn start(&self, id: TorrentId, target: &str, session: &SessionHandle) {
        session.torrent(id).move_storage(target.to_owned(), self.flags);
    }
}

/// Paths naming the same directory, ignoring trailing separators (and case on Windows).
pub fn same_dir(a: &str, b: &str) -> bool {
    let norm = |p: &str| {
        let abs = crate::disk::complete(p);
        let s = Path::new(&abs).components().collect::<std::path::PathBuf>().to_string_lossy().into_owned();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

/// The longest folder name [`label_dir`] makes, in bytes (most file systems' limit; in UTF-16
/// units, NTFS's, it is never more).
const MAX_NAME: usize = 255;

/// Names Windows keeps for devices, with or without an extension (`nul.txt`), in any case.
const DEVICES: [&str; 32] = [
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
    "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// The folder, relative, that "Append the torrent's label" puts a torrent with `label` in;
/// empty when the label names none (the torrent goes to the completed folder itself).
///
/// A label is user text that also arrives through the Web UI, the qBittorrent API's categories,
/// RSS filters and imports, so the folder never leaves the completed folder, and the same
/// label names the same folder on every system:
/// - `/` and `\` separate nested folders (qBittorrent's sub-categories are `/`-separated, and
///   µTorrent, appending the label to the path, makes either one a folder on Windows);
/// - in each name, the characters Windows forbids (`<>:"|?*` and control characters) become
///   `_`; spaces around it and dots at its end go (Windows drops trailing ones);
/// - a name left empty, as `.` and `..` are, is skipped, so nothing climbs out, and a drive
///   (`C:`) or a UNC or absolute path is only folder names below;
/// - a device name (`CON`, `NUL`, `COM1`, also `nul.txt`) gets a `_` in front;
/// - a name is cut to 255 bytes.
pub fn label_dir(label: &str) -> PathBuf {
    let mut dir = PathBuf::new();
    for part in label.split(['/', '\\']) {
        let mut name: String = part.chars().map(|c| if forbidden(c) { '_' } else { c }).collect();
        let stem = name.split('.').next().unwrap_or("").trim();
        if DEVICES.iter().any(|d| d.eq_ignore_ascii_case(stem)) {
            name.insert(0, '_');
        }
        if name.len() > MAX_NAME {
            let mut end = MAX_NAME;
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            name.truncate(end);
        }
        let name = name.trim().trim_end_matches(['.', ' ']).trim_end();
        if !name.is_empty() {
            dir.push(name);
        }
    }
    dir
}

/// A character Windows does not allow in a file name (`/` and `\` separate folders).
fn forbidden(c: char) -> bool {
    c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_torrents_move() {
        let mut m = MoveCompleted::new("/done");
        assert!(m.wants("/downloads", "/done"));
        assert!(!m.wants("/done", "/done"));
        assert!(!m.wants("/done/", "/done"));
        m.only_from = Some("/downloads".into());
        assert!(m.wants("/downloads/", "/done"));
        assert!(!m.wants("/elsewhere", "/done"));
        assert!(!same_dir("/a", "/a/b"));
    }

    #[test]
    fn the_label_picks_the_folder() {
        let join = |a: &str, b: &str| Path::new(a).join(b).to_string_lossy().into_owned();
        let mut m = MoveCompleted::new("/done");
        assert_eq!(m.target("tv"), "/done", "off by default, as in µTorrent");
        m.append_label = true;
        let tv = join("/done", "tv");
        assert_eq!(m.target("tv"), tv);
        assert_eq!(m.target(""), "/done", "no label: the folder itself");
        assert_eq!(m.target(" / .. "), "/done", "a label naming no folder");
        assert_eq!(m.target("tv/hd"), Path::new(&tv).join("hd").to_string_lossy());

        // already in its label's folder: stays; in the completed folder itself: goes in
        assert!(!m.wants(&tv, &m.target("tv")));
        assert!(!m.wants(&format!("{tv}/"), &m.target("tv")));
        assert!(m.wants("/done", &m.target("tv")));
        assert!(m.wants(&tv, &m.target("")));
        assert!(m.wants(&tv, &m.target("film")));

        // only from the download folder: the label does not widen it
        m.only_from = Some("/downloads".into());
        assert!(m.wants("/downloads", &m.target("tv")));
        assert!(!m.wants("/done", &m.target("tv")));
        assert!(!m.wants("/elsewhere", &m.target("tv")));
    }

    #[test]
    fn labels_make_safe_folders() {
        let parts = |label: &str| -> Vec<String> {
            let d = label_dir(label);
            for c in d.components() {
                assert!(matches!(c, std::path::Component::Normal(_)), "{label:?} gives {c:?}");
            }
            d.iter().map(|s| s.to_string_lossy().into_owned()).collect()
        };
        assert_eq!(parts("Linux ISOs"), ["Linux ISOs"]);
        assert_eq!(parts("Filmek/Ősz"), ["Filmek", "Ősz"]);
        assert_eq!(parts(r"tv\hd"), ["tv", "hd"]);
        assert!(parts("").is_empty() && parts("   ").is_empty() && parts("/").is_empty());
        assert!(parts(".").is_empty() && parts("..").is_empty() && parts("...").is_empty());
        assert_eq!(parts("../../etc"), ["etc"]);
        assert_eq!(parts(r"..\..\Windows\System32"), ["Windows", "System32"]);
        assert_eq!(parts("/etc/passwd"), ["etc", "passwd"]);
        assert_eq!(parts(r"C:\Windows"), ["C_", "Windows"]);
        assert_eq!(parts("C:"), ["C_"]);
        assert_eq!(parts(r"\\server\share\x"), ["server", "share", "x"]);
        assert_eq!(parts(r"\\?\C:\x"), ["_", "C_", "x"]);
        assert_eq!(parts("a<b>c:d\"e|f?g*h"), ["a_b_c_d_e_f_g_h"]);
        assert_eq!(parts("a\0b\nc\u{1b}d"), ["a_b_c_d"]);
        assert_eq!(parts(" tv. . "), ["tv"]);
        assert_eq!(parts(".hidden"), [".hidden"]);
        assert_eq!(parts("a//b"), ["a", "b"]);
        assert_eq!(parts("con"), ["_con"]);
        assert_eq!(parts("NUL.txt"), ["_NUL.txt"]);
        assert_eq!(parts("com1"), ["_com1"]);
        assert_eq!(parts("LPT¹"), ["_LPT¹"]);
        assert_eq!(parts("aux /x"), ["_aux", "x"]);
        assert_eq!(parts("console"), ["console"]);
        assert_eq!(parts("com10"), ["com10"]);

        let long = "é".repeat(200);
        let p = parts(&long);
        assert!(p[0].len() <= MAX_NAME && p[0].chars().all(|c| c == 'é'), "cut on a character");
        assert_eq!(parts(&format!("{}x", ".a".repeat(128))), [".a".repeat(127)], "no dot left at the cut");
    }
}
