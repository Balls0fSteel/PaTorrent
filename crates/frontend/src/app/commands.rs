//! The torrent commands, the selection and Preferences (GTK `torrent_action`, `set_label`,
//! `set_limit`, `toggle_super_seeding`, `set_download_location`, `set_paused`, `remove`,
//! `menu_state`, `super_seed_state`, `selection_changed`, the Files tab's actions,
//! `show_properties`' logic, `apply_options`, `load_ip_filter`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rookparcel_core::{
    DownloadPriority, IpFilter, MoveFlags, QueueMove, RemoveFlags, TorrentFlags, TorrentId, same_dir,
};

use super::{Core, Events};
use crate::actions;
use crate::detail::Tab;
use crate::model::Filter;
use crate::options::{GuiOptions, Options};
use crate::properties::{Changes, Properties, TorrentProps};
use crate::tr;

/// A torrent command of the toolbar, the Torrent menu and the context menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TorrentAction {
    Start,
    ForceStart,
    Pause,
    Stop,
    QueueUp,
    QueueDown,
    QueueTop,
    QueueBottom,
    /// Force Re-check.
    Recheck,
    /// Update Tracker.
    Reannounce,
}

/// The torrent menu's state for a selection: its texts, check marks and greying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuState {
    /// The labels in use (Label ▸'s items).
    pub labels: Vec<String>,
    /// The label every torrent has (`Some("")`: none of them has one), checked in Label ▸;
    /// `None` while they differ.
    pub label: Option<String>,
    /// Remove Label is enabled (some torrent has a label).
    pub can_remove_label: bool,
    /// One finished torrent: "Open Containing Folder" names its own folder
    /// (`MenuOpenFolder`, else `MenuOpenFolderMany`).
    pub single_done: bool,
    /// Open Containing Folder is enabled (not in a remote window: the server's folders are not
    /// on this computer).
    pub can_open_folder: bool,
    /// The first torrent's limits in kB/s, as Bandwidth ▸'s items ([`actions::LIMITS`]) check
    /// them; `u32::MAX` when the limit is not a whole number of kB/s (no item is checked).
    pub download_kib: u32,
    pub upload_kib: u32,
    /// Set Download Location is enabled (no torrent is moving its files).
    pub can_set_location: bool,
    /// Initial Seeding is enabled (every torrent is a seed) and checked (every one has it).
    pub can_super_seed: bool,
    pub super_seeding: bool,
}

impl Core {
    /// The selection changed: the detail tabs show its first torrent. True when that changed.
    pub fn select(&mut self, ids: &[TorrentId]) -> bool {
        let id = ids.first().copied();
        let changed = self.detail.id != id;
        self.detail.select(id);
        if changed {
            self.files_requested = None;
            self.request_detail();
            self.ev.detail = true;
        }
        changed
    }

    /// The detail tab on screen changed: it is asked for what it shows.
    pub fn show_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.request_detail();
        self.ev.detail = true;
    }

    /// The sidebar picked a category or label: the list shows those torrents (not feed items).
    pub fn show_torrents(&mut self, filter: Filter) {
        self.list.set_filter(filter);
        self.show_feed(None);
    }

    /// The search box's name filter.
    pub fn set_search(&mut self, text: &str) {
        self.list.set_search(text);
    }

    /// Whether the torrent has finished (a double click opens its folder then, else
    /// Properties).
    pub fn is_finished(&self, id: TorrentId) -> bool {
        self.list.get(id).is_some_and(|st| st.is_finished)
    }

    /// Runs `a` on the torrents `ids` (Start All / Stop All: on [`Core::ids`]).
    pub fn torrent_action(&mut self, a: TorrentAction, ids: &[TorrentId]) {
        // moving several up or to the top keeps their order: the first one goes last
        let ids: Vec<TorrentId> = if matches!(a, TorrentAction::QueueUp | TorrentAction::QueueTop) {
            ids.iter().rev().copied().collect()
        } else {
            ids.to_vec()
        };
        for id in ids {
            let t = self.handle.torrent(id);
            match a {
                TorrentAction::Start => {
                    actions::start(&t);
                    self.set_paused(id, false);
                }
                TorrentAction::ForceStart => {
                    actions::force_start(&t);
                    self.set_paused(id, false);
                }
                TorrentAction::Pause => {
                    actions::stop(&t);
                    self.set_paused(id, true);
                }
                TorrentAction::Stop => {
                    actions::stop(&t);
                    self.set_paused(id, false);
                }
                TorrentAction::QueueUp => t.queue_position(QueueMove::Up),
                TorrentAction::QueueDown => t.queue_position(QueueMove::Down),
                TorrentAction::QueueTop => t.queue_position(QueueMove::Top),
                TorrentAction::QueueBottom => t.queue_position(QueueMove::Bottom),
                TorrentAction::Recheck => t.force_recheck(),
                TorrentAction::Reannounce => t.force_reannounce(),
            }
        }
        self.handle.post_torrent_updates();
    }

    /// µTorrent's Pause mark (a stopped torrent shown as Paused), kept in the preferences.
    fn set_paused(&mut self, id: TorrentId, paused: bool) {
        self.list.set_user_paused(id, paused);
        if let Some(st) = self.list.get(id) {
            let key = format!("torrent.{}.paused", st.info_hashes.key_hex());
            if paused {
                self.prefs.set_bool(&key, true);
            } else {
                self.prefs.remove(&key);
            }
        }
    }

    /// Remove's question for `ids` (message, detail), or `None` when Preferences say not to
    /// ask ("Confirm when deleting torrents" off and the data is kept).
    pub fn remove_question(&self, ids: &[TorrentId], with_data: bool) -> Option<(String, String)> {
        if !(self.gui_options().confirm_delete || with_data) {
            return None;
        }
        let names: Vec<String> =
            ids.iter().filter_map(|id| self.list.get(*id).map(|s| s.name.clone())).collect();
        let message = match (names.as_slice(), with_data) {
            ([name], false) => tr!(AskRemove, name),
            ([name], true) => tr!(AskRemoveData, name),
            (_, false) => tr!(AskRemoveMany, count = names.len()),
            (_, true) => tr!(AskRemoveDataMany, count = names.len()),
        };
        let detail = if with_data { tr!(RemoveDeletesDataTrash) } else { tr!(RemoveKeepsData) };
        Some((message, detail.to_owned()))
    }

    /// Removes `ids` (after [`Core::remove_question`]'s answer), with their data or not; the
    /// part file goes either way.
    pub fn remove(&mut self, ids: &[TorrentId], with_data: bool) {
        let flags = if with_data {
            RemoveFlags::DELETE_FILES | RemoveFlags::DELETE_PARTFILE
        } else {
            RemoveFlags::DELETE_PARTFILE
        };
        for &id in ids {
            self.handle.remove_torrent(id, flags);
        }
    }

    /// Label ▸: gives `ids` the label (empty: Remove Label), kept in the preferences.
    pub fn set_label(&mut self, ids: &[TorrentId], label: &str) {
        for &id in ids {
            self.list.set_label(id, label);
            if let Some(st) = self.list.get(id) {
                let key = format!("torrent.{}.label", st.info_hashes.key_hex());
                if label.is_empty() {
                    self.prefs.remove(&key);
                } else {
                    self.prefs.set(&key, label);
                }
            }
        }
        self.save_prefs();
    }

    /// Bandwidth Allocation: the download or upload limit of `ids`, bytes per second (0: none).
    pub fn set_limit(&mut self, ids: &[TorrentId], download: bool, bytes: u32) {
        for &id in ids {
            let t = self.handle.torrent(id);
            if download {
                t.set_download_limit(bytes);
            } else {
                t.set_upload_limit(bytes);
            }
        }
        self.handle.post_torrent_updates();
    }

    /// Advanced > Initial Seeding: on for all unless every torrent has it, as in µTorrent.
    pub fn toggle_super_seeding(&mut self, ids: &[TorrentId]) {
        let (_, on) = self.super_seed_state(ids);
        for &id in ids {
            let t = self.handle.torrent(id);
            if on {
                t.unset_flags(TorrentFlags::SUPER_SEEDING);
            } else {
                t.set_flags(TorrentFlags::SUPER_SEEDING);
            }
        }
        self.handle.post_torrent_updates();
    }

    /// (every torrent is a seed, every torrent is super seeding).
    fn super_seed_state(&self, ids: &[TorrentId]) -> (bool, bool) {
        let all = |f: &dyn Fn(&rookparcel_core::TorrentStatus) -> bool| {
            ids.iter().all(|&id| self.list.get(id).is_some_and(f))
        };
        (all(&|s| s.is_seeding), all(&|s| s.super_seeding))
    }

    /// Set Download Location's starting folder: the first torrent's save path. (A remote
    /// window asks for a folder on the server by text, `WebSetLocation`.)
    pub fn download_location_start(&self, ids: &[TorrentId]) -> Option<String> {
        self.list.get(*ids.first()?).map(|st| st.save_path.clone())
    }

    /// Advanced > Set Download Location: the files of `ids` move to `dir` (µTorrent 2.2.1
    /// moves them; files already there are replaced, libtorrent's default).
    pub fn set_download_location(&mut self, ids: &[TorrentId], dir: &str) {
        if dir.is_empty() {
            return;
        }
        for &id in ids {
            let Some(st) = self.list.get(id) else { continue };
            if same_dir(&st.save_path, dir) {
                continue;
            }
            let name = st.name.clone();
            self.handle.torrent(id).move_storage(dir.to_owned(), MoveFlags::AlwaysReplaceFiles);
            self.log(tr!(LogMovingTo, name, dir));
        }
        self.handle.post_torrent_updates();
    }

    /// The torrent menu for `ids`: the label in common, the first torrent's limits, Initial
    /// Seeding for seeds only. `None` for an empty selection.
    pub fn menu_state(&self, ids: &[TorrentId]) -> Option<MenuState> {
        let first = *ids.first()?;
        let label = self.list.extra(first).label;
        let label = ids.iter().all(|&id| self.list.extra(id).label == label).then(|| label.to_owned());
        let st = self.list.get(first);
        let kib = |b: u32| if b % 1024 == 0 { b / 1024 } else { u32::MAX };
        let (seeds, super_seeding) = self.super_seed_state(ids);
        let moving = ids.iter().any(|&id| self.list.get(id).is_some_and(|s| s.moving_storage));
        Some(MenuState {
            labels: self.labels(),
            can_remove_label: label.as_deref() != Some(""),
            label,
            single_done: ids.len() == 1 && st.is_some_and(|s| s.is_finished),
            can_open_folder: !self.is_remote(),
            download_kib: st.map_or(0, |s| kib(s.download_limit)),
            upload_kib: st.map_or(0, |s| kib(s.upload_limit)),
            can_set_location: !moving,
            can_super_seed: seeds,
            super_seeding,
        })
    }

    /// What Open Containing Folder shows for the first of `ids`: its file or folder (the GUI
    /// reveals it). `None` in a remote window.
    pub fn folder_of(&self, ids: &[TorrentId]) -> Option<PathBuf> {
        if self.is_remote() {
            return None;
        }
        let st = self.list.get(*ids.first()?)?;
        Some(PathBuf::from(&st.save_path).join(&st.name))
    }

    /// The torrent the detail tabs show: its status and whether the user paused it (the
    /// General tab's texts, `detail::general`). `None` while nothing is selected.
    pub fn detail_torrent(&self) -> Option<(&rookparcel_core::TorrentStatus, bool)> {
        let id = self.detail.id?;
        Some((self.list.get(id)?, self.list.extra(id).user_paused))
    }

    // ---- the Files tab ----------------------------------------------------------------------

    /// The Files tab's quick filter (D50); true when the rows it lets through changed.
    pub fn set_file_filter(&mut self, text: &str) -> bool {
        let changed = self.detail.set_file_filter(text);
        self.ev.detail |= changed;
        changed
    }

    /// Shows the Files tab as the folder tree or the plain list (D62); true when it changed
    /// (the view reloads). The GUI keeps the choice in [`super::Layout::files_tree`].
    pub fn set_files_tree(&mut self, on: bool) -> bool {
        let changed = self.detail.set_files_tree(on);
        self.ev.detail |= changed;
        changed
    }

    /// The Files tab's row `row`: the torrent's save path and the file's path (Open, Open
    /// Containing Folder, Relocate). `None` in a remote window.
    pub fn file_path(&self, row: usize) -> Option<(PathBuf, PathBuf)> {
        if self.is_remote() {
            return None;
        }
        let st = self.list.get(self.detail.id?)?;
        Some((PathBuf::from(&st.save_path), self.detail.file_path(row, &st.save_path)?))
    }

    /// The Files tab's priorities for `rows` (`None`: Prioritize by File Order, which takes
    /// the whole torrent while the filter hides some).
    pub fn set_file_priority(&mut self, rows: &[usize], prio: Option<DownloadPriority>) {
        let Some(id) = self.detail.id else { return };
        let t = self.handle.torrent(id);
        let files: Vec<u32> = if prio.is_none() && self.detail.file_filter.is_on() {
            self.detail.all_files()
        } else {
            self.detail.file_indexes(rows)
        };
        let set = match prio {
            Some(prio) => files.into_iter().map(|f| (f, prio)).collect(),
            None => crate::detail::by_file_order(&files),
        };
        for (f, prio) in set {
            t.set_file_priority(f, prio);
        }
        // read the list back to show the new priorities
        self.files_requested = None;
        self.request_detail();
    }

    /// Relocate for the Files tab's row `row`: the file moves to `to`, or, when it is not on
    /// disk, the torrent looks for it there.
    pub fn relocate_file(&mut self, row: usize, to: &Path) {
        if self.is_remote() {
            return;
        }
        let Some(id) = self.detail.id else { return };
        let Some(file) = self.detail.file_index(row) else { return };
        if self.file_path(row).is_some_and(|(_, p)| p == to) {
            return;
        }
        self.handle.torrent(id).rename_file(file, to.to_string_lossy().into_owned());
        self.files_requested = None;
        self.request_detail();
    }

    // ---- Properties -------------------------------------------------------------------------

    /// The Torrent Properties dialog's fields for `ids` (waiting up to 2 s for their tracker
    /// lists), and the torrents' names for its title. `None` when none is in the list.
    pub fn properties(&mut self, ids: &[TorrentId]) -> Option<(Vec<String>, Properties)> {
        let ids: Vec<TorrentId> = ids.iter().copied().filter(|&id| self.list.get(id).is_some()).collect();
        if ids.is_empty() {
            return None;
        }
        let trackers =
            crate::properties::fetch_trackers(&self.handle, &ids, &mut self.alerts, Duration::from_secs(2));
        let mut items = Vec::new();
        let mut names = Vec::new();
        for id in &ids {
            let Some(st) = self.list.get(*id) else { continue };
            let mut t = TorrentProps::new(st, trackers.get(id).map(Vec::as_slice), &self.prefs);
            t.private = self.handle.torrent(*id).metadata().is_some_and(|m| m.private);
            items.push(t);
            names.push(st.name.clone());
        }
        let defaults = self.gui_options().property_defaults();
        Some((names, Properties::load(&items, defaults)))
    }

    /// Properties' OK: what changed goes to each of `ids` and to the preferences.
    pub fn apply_properties(&mut self, ids: &[TorrentId], changes: &Changes) {
        if changes.is_empty() {
            return;
        }
        let defaults = self.gui_options().property_defaults();
        for &id in ids {
            let Some(st) = self.list.get(id) else { continue };
            let key = st.info_hashes.key_hex();
            changes.apply(&self.handle.torrent(id));
            changes.store(&mut self.prefs, &key, &defaults);
        }
        self.save_prefs();
        self.handle.post_torrent_updates();
        self.request_detail();
    }

    // ---- Preferences ------------------------------------------------------------------------

    /// Stores `o` and hands it to the engine and the window, as GTK's OK and Apply do: the
    /// settings through the scheduler, the folders, seeding goal, update check and title, PEX
    /// on every torrent when it changed, the IP filter when it changed. (The GUI applies
    /// [`Options::integration`] itself.)
    pub fn apply_options(&mut self, o: &Options) -> Events {
        let old = self.options();
        self.listen.set_configured(crate::settings::listen_port(&o.settings));
        o.store(&mut self.prefs);
        self.save_prefs();
        if self.is_remote() && o.gui.language != old.gui.language {
            // one language for every window: it lives in the local client's settings
            let mut base = crate::prefs::Prefs::load(self.dirs.config.join("rookparcel.ini"));
            match o.gui.language.trim() {
                "" => base.remove("ui.language"),
                code => base.set("ui.language", code),
            }
            if let Err(e) = base.save() {
                self.log(tr!(LogPrefsNotSaved, error = e));
            }
        }
        self.save_path = o.gui.download_dir.clone();
        // the engine settings, as the scheduler's hour changes them
        self.schedule = o.gui.schedule.clone();
        self.run_scheduler(Some(o));
        o.gui.update_move_completed(&mut self.move_completed);
        self.seeding_goal = o.gui.seeding_goal();
        self.torrent_folders = o.gui.torrent_folders.clone();
        let automatic = o.gui.check_updates && self.updates_here;
        self.updates.set_automatic(automatic, std::time::Instant::now());
        self.update_beta = o.gui.update_beta;
        // the title gains or loses the speeds now, not at the next second
        self.title_speed = o.gui.title_speed;
        self.ev.status = true;
        if o.gui.alternate_rows != old.gui.alternate_rows {
            self.ev.alternate_rows = Some(o.gui.alternate_rows);
        }
        if o.gui.pex != old.gui.pex {
            for id in self.list.ids() {
                let t = self.handle.torrent(id);
                if o.gui.pex {
                    t.unset_flags(TorrentFlags::DISABLE_PEX);
                } else {
                    t.set_flags(TorrentFlags::DISABLE_PEX);
                }
            }
        }
        let dir = self.dirs.settings.clone();
        let changed = (o.gui.ip_filter, o.gui.ip_filter_file(&dir))
            != (old.gui.ip_filter, old.gui.ip_filter_file(&dir));
        // a server applies its own block list
        if changed && !self.is_remote() {
            if o.gui.ip_filter {
                self.load_ip_filter(&o.gui);
            } else {
                self.handle.set_ip_filter(IpFilter::new());
                self.log(tr!(LogIpFilterOff));
            }
        }
        self.take_events()
    }

    /// Preferences' Reload IP filter: reads `path` into the engine; the rule count or the
    /// error (also in the Logger).
    pub fn reload_ip_filter(&mut self, path: &Path) -> Result<usize, String> {
        let r = self.handle.load_ip_filter(path).map_err(|e| e.to_string());
        self.log_ip_filter(path, &r);
        r
    }

    /// Loads the configured block list (`ipfilter.dat` in the settings folder by default).
    pub(super) fn load_ip_filter(&mut self, gui: &GuiOptions) {
        let path = gui.ip_filter_file(&self.dirs.settings);
        let r = self.handle.load_ip_filter(&path).map_err(|e| e.to_string());
        self.log_ip_filter(&path, &r);
    }

    fn log_ip_filter(&mut self, path: &Path, r: &Result<usize, String>) {
        let line = match r {
            Ok(n) => tr!(LogIpFilterRules, count = n, path = path.display()),
            Err(e) => tr!(LogIpFilterError, path = path.display(), error = e),
        };
        self.log(line);
    }
}
