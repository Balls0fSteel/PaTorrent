//! "Move completed downloads" (µTorrent Preferences > Directories): moves each torrent that
//! finishes downloading into one folder, through [`TorrentHandle::move_storage`].
//!
//! libtorrent has no such setting; qBittorrent and µTorrent do this in the client on
//! `torrent_finished_alert`. [`MoveCompleted`] is that client side, so every frontend gets the
//! same behaviour: feed it every alert.
//!
//! [`TorrentHandle::move_storage`]: crate::TorrentHandle::move_storage

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::alert::Alert;
use crate::params::MoveFlags;
use crate::session::SessionHandle;
use crate::types::TorrentId;

/// Moves finished torrents to `dir`.
#[derive(Debug, Clone)]
pub struct MoveCompleted {
    /// Where finished downloads go.
    pub dir: String,
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
            flags: MoveFlags::default(),
            only_from: None,
            paths: HashMap::new(),
            waiting: HashSet::new(),
        }
    }

    /// Looks at one alert; returns the torrent it asked the session to move, if any. Status
    /// updates ([`Alert::StateUpdate`]) tell it where torrents are, so the frontend should keep
    /// posting them (frontends do, for their lists).
    pub fn on_alert(&mut self, alert: &Alert, session: &SessionHandle) -> Option<TorrentId> {
        match alert {
            Alert::StateUpdate(v) => {
                let mut moved = None;
                for st in v {
                    self.paths.insert(st.id, st.save_path.clone());
                    if self.waiting.remove(&st.id) && self.wants(&st.save_path) {
                        self.start(st.id, session);
                        moved = Some(st.id);
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
                Some(path) if !self.wants(path) => None,
                None if self.only_from.is_some() => {
                    self.waiting.insert(*id);
                    None
                }
                _ => {
                    self.start(*id, session);
                    Some(*id)
                }
            },
            _ => None,
        }
    }

    /// Whether a torrent saved in `save_path` is moved when it finishes.
    pub fn wants(&self, save_path: &str) -> bool {
        !same_dir(save_path, &self.dir) && self.only_from.as_deref().is_none_or(|f| same_dir(save_path, f))
    }

    fn start(&self, id: TorrentId, session: &SessionHandle) {
        session.torrent(id).move_storage(self.dir.clone(), self.flags);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_torrents_move() {
        let mut m = MoveCompleted::new("/done");
        assert!(m.wants("/downloads"));
        assert!(!m.wants("/done"));
        assert!(!m.wants("/done/"));
        m.only_from = Some("/downloads".into());
        assert!(m.wants("/downloads/"));
        assert!(!m.wants("/elsewhere"));
        assert!(!same_dir("/a", "/a/b"));
    }
}
