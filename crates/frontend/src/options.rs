//! What the Preferences dialog edits: the engine's [`Settings`] plus the frontend's own options
//! (folders, tray behaviour, per-torrent defaults, the IP filter file), loaded from and stored
//! to the preferences file. Toolkit-independent, so the GTK dialog can share it.

use std::path::{Path, PathBuf};

use patorrent_core::{AddTorrentParams, MoveCompleted, Settings, TorrentFlags};

use crate::prefs::Prefs;
use crate::properties::Defaults;
use crate::scheduler::Schedule;
use crate::seeding::{SeedGoal, SeedingGoal};
use crate::settings;
use crate::torrent_folders::TorrentFolders;

/// The frontend's options (preferences file keys in brackets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuiOptions {
    /// Where new downloads go (`dir.download`).
    pub download_dir: String,
    /// Move finished downloads to `completed_dir` (`dir.move_completed`; µTorrent's "Move
    /// completed downloads to").
    pub move_completed: bool,
    /// Where finished downloads go (`dir.completed`).
    pub completed_dir: String,
    /// Move only torrents saved in `download_dir` (`dir.move_only_default`; µTorrent's "Only
    /// move from the default download directory", on by default as there).
    pub move_only_default: bool,
    /// "Location of .torrents": stored copies, finished jobs, auto-load (`dir.store_torrents`,
    /// `dir.torrents`, `dir.move_torrents`, `dir.finished_torrents`, `dir.autoload`,
    /// `dir.autoload_dir`, `dir.autoload_delete`).
    pub torrent_folders: TorrentFolders,
    /// Ask before removing torrents (`ui.confirm_delete`).
    pub confirm_delete: bool,
    /// Ask at start when PaTorrent does not open `.torrent` files and `magnet:` links
    /// (`ui.check_assoc`; µTorrent's "Check association on startup"). Off by default here.
    pub check_association: bool,
    /// Close / minimize to the tray (`ui.close_to_tray`, `ui.minimize_to_tray`).
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
    /// Tray balloon when a download completes (`ui.balloons`).
    pub balloons: bool,
    /// Add torrents stopped instead of queued (`ui.add_stopped`).
    pub add_stopped: bool,
    /// Show the Add New Torrent dialog for `.torrent` files (`ui.show_add_dialog`; µTorrent's
    /// "Show a window that displays the files inside the torrent").
    pub show_add_dialog: bool,
    /// Pick a new listen port at every start (`net.random_port`).
    pub random_port: bool,
    /// Peer exchange for every torrent (`bt.pex`); off sets `DISABLE_PEX` on each one.
    pub pex: bool,
    /// Connected peers per new torrent, 0 = no limit (`bt.max_peers`).
    pub max_peers_per_torrent: u32,
    /// Upload slots per new torrent, 0 = no limit (`bt.upload_slots`).
    pub upload_slots_per_torrent: u32,
    /// Queueing > Seeding Goal: "Minimum ratio (%)" (`queue.seed_ratio`, 150) and "Minimum
    /// seeding time (minutes)" (`queue.seed_time`, 0); 0 switches a target off.
    pub seed_ratio: u32,
    pub seed_minutes: u32,
    /// "Limit the upload rate to X kB/s [0: stop]" when the goal is reached
    /// (`queue.seed_limit`, `queue.seed_limit_kib`); off stops the torrent.
    pub seed_limit_upload: bool,
    pub seed_limit_kib: u32,
    /// Load the IP filter at start and on Apply (`ipfilter.enable`).
    pub ip_filter: bool,
    /// The block list; empty = `ipfilter.dat` in the settings folder, like µTorrent
    /// (`ipfilter.path`).
    pub ip_filter_path: String,
    /// Preferences > Scheduler (`sched.*`).
    pub schedule: Schedule,
}

impl GuiOptions {
    pub fn load(prefs: &Prefs, default_download_dir: &str) -> GuiOptions {
        let u32_of = |k: &str| prefs.get_i32(k).unwrap_or(0).max(0) as u32;
        GuiOptions {
            download_dir: prefs
                .get("dir.download")
                .filter(|d| !d.trim().is_empty())
                .unwrap_or(default_download_dir)
                .to_owned(),
            move_completed: prefs.get_bool("dir.move_completed", false),
            completed_dir: prefs.get("dir.completed").unwrap_or_default().to_owned(),
            move_only_default: prefs.get_bool("dir.move_only_default", true),
            torrent_folders: TorrentFolders::load(prefs),
            confirm_delete: prefs.get_bool("ui.confirm_delete", true),
            check_association: prefs.get_bool("ui.check_assoc", false),
            close_to_tray: prefs.get_bool("ui.close_to_tray", true),
            minimize_to_tray: prefs.get_bool("ui.minimize_to_tray", false),
            balloons: prefs.get_bool("ui.balloons", true),
            add_stopped: prefs.get_bool("ui.add_stopped", false),
            show_add_dialog: prefs.get_bool("ui.show_add_dialog", true),
            random_port: prefs.get_bool("net.random_port", false),
            pex: prefs.get_bool("bt.pex", true),
            max_peers_per_torrent: u32_of("bt.max_peers"),
            upload_slots_per_torrent: u32_of("bt.upload_slots"),
            seed_ratio: prefs
                .get_i32("queue.seed_ratio")
                .map_or(SeedGoal::default().ratio_pct, |v| v.max(0) as u32),
            seed_minutes: u32_of("queue.seed_time"),
            seed_limit_upload: prefs.get_bool("queue.seed_limit", false),
            seed_limit_kib: u32_of("queue.seed_limit_kib"),
            ip_filter: prefs.get_bool("ipfilter.enable", false),
            ip_filter_path: prefs.get("ipfilter.path").unwrap_or_default().to_owned(),
            schedule: Schedule::load(prefs),
        }
    }

    pub fn store(&self, prefs: &mut Prefs) {
        prefs.set("dir.download", &self.download_dir);
        prefs.set_bool("dir.move_completed", self.move_completed);
        if self.completed_dir.trim().is_empty() {
            prefs.remove("dir.completed");
        } else {
            prefs.set("dir.completed", self.completed_dir.trim());
        }
        prefs.set_bool("dir.move_only_default", self.move_only_default);
        self.torrent_folders.store(prefs);
        prefs.set_bool("ui.confirm_delete", self.confirm_delete);
        prefs.set_bool("ui.check_assoc", self.check_association);
        prefs.set_bool("ui.close_to_tray", self.close_to_tray);
        prefs.set_bool("ui.minimize_to_tray", self.minimize_to_tray);
        prefs.set_bool("ui.balloons", self.balloons);
        prefs.set_bool("ui.add_stopped", self.add_stopped);
        prefs.set_bool("ui.show_add_dialog", self.show_add_dialog);
        prefs.set_bool("net.random_port", self.random_port);
        prefs.set_bool("bt.pex", self.pex);
        prefs.set("bt.max_peers", self.max_peers_per_torrent);
        prefs.set("bt.upload_slots", self.upload_slots_per_torrent);
        prefs.set("queue.seed_ratio", self.seed_ratio);
        prefs.set("queue.seed_time", self.seed_minutes);
        prefs.set_bool("queue.seed_limit", self.seed_limit_upload);
        prefs.set("queue.seed_limit_kib", self.seed_limit_kib);
        prefs.set_bool("ipfilter.enable", self.ip_filter);
        if self.ip_filter_path.trim().is_empty() {
            prefs.remove("ipfilter.path");
        } else {
            prefs.set("ipfilter.path", self.ip_filter_path.trim());
        }
        self.schedule.store(prefs);
    }

    /// The block list to load: the configured path, or `ipfilter.dat` in `settings_dir`.
    pub fn ip_filter_file(&self, settings_dir: &Path) -> PathBuf {
        match self.ip_filter_path.trim() {
            "" => settings_dir.join("ipfilter.dat"),
            p => PathBuf::from(p),
        }
    }

    /// "Move completed downloads" as the alert-loop helper, or `None` when it is off (or has no
    /// folder).
    pub fn move_completed(&self) -> Option<MoveCompleted> {
        let dir = self.completed_dir.trim();
        if !self.move_completed || dir.is_empty() {
            return None;
        }
        let mut m = MoveCompleted::new(dir);
        m.only_from = self.move_only_default.then(|| self.download_dir.clone());
        Some(m)
    }

    /// Points a running [`MoveCompleted`] at these options, keeping the save paths it has
    /// learned (a new one would move the next finished torrent before knowing where it is).
    pub fn update_move_completed(&self, current: &mut Option<MoveCompleted>) {
        match (current.as_mut(), self.move_completed()) {
            (Some(m), Some(new)) => {
                m.dir = new.dir;
                m.only_from = new.only_from;
            }
            (_, new) => *current = new,
        }
    }

    /// Preferences > Queueing's seeding goal, for [`SeedingGoal::check`].
    pub fn seeding_goal(&self) -> SeedingGoal {
        SeedingGoal {
            goal: SeedGoal { ratio_pct: self.seed_ratio, minutes: self.seed_minutes },
            limit_upload: self.seed_limit_upload,
            limit_kib: self.seed_limit_kib,
        }
    }

    /// What the Properties dialog falls back to when a torrent has no setting of its own.
    pub fn property_defaults(&self) -> Defaults {
        Defaults { upload_slots: self.upload_slots_per_torrent, seed_goal: self.seeding_goal().goal }
    }

    /// Applies the per-torrent defaults to a torrent being added.
    pub fn apply_to_new(&self, p: &mut AddTorrentParams) {
        if self.add_stopped {
            // µTorrent's Stop: paused and outside the queue
            p.flags = p.flags & !TorrentFlags::AUTO_MANAGED | TorrentFlags::PAUSED;
        }
        if !self.pex {
            p.flags |= TorrentFlags::DISABLE_PEX;
        }
        p.max_connections = (self.max_peers_per_torrent > 0).then_some(self.max_peers_per_torrent);
        p.max_uploads = (self.upload_slots_per_torrent > 0).then_some(self.upload_slots_per_torrent);
    }
}

/// Everything the dialog edits.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub settings: Settings,
    pub gui: GuiOptions,
}

impl Options {
    pub fn load(prefs: &Prefs, default_download_dir: &str) -> Options {
        Options { settings: settings::load(prefs), gui: GuiOptions::load(prefs, default_download_dir) }
    }

    pub fn store(&self, prefs: &mut Prefs) {
        settings::store(prefs, &self.settings);
        self.gui.store(prefs);
    }
}

/// A listen port for "Random port" and "Randomize port each start": 10000..=65000, as µTorrent
/// picks, from the OS-seeded hasher (no RNG dependency).
pub fn random_port() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),
    );
    10_000 + (h.finish() % 55_001) as u16
}

#[cfg(test)]
mod tests {
    use patorrent_core::TorrentSource;

    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let mut prefs = Prefs::default();
        let o = Options::load(&prefs, r"C:\Downloads");
        assert_eq!(o.settings, Settings::default());
        assert_eq!(o.gui.download_dir, r"C:\Downloads");
        assert!(o.gui.confirm_delete && o.gui.close_to_tray && !o.gui.minimize_to_tray && o.gui.pex);
        assert!(!o.gui.ip_filter && o.gui.show_add_dialog);
        assert_eq!(o.gui.seeding_goal(), SeedingGoal::default(), "150 %, no time, stop");
        assert_eq!(o.gui.property_defaults(), Defaults { upload_slots: 0, seed_goal: SeedGoal::default() });
        assert!(!o.gui.move_completed && o.gui.move_only_default && o.gui.completed_dir.is_empty());
        assert!(o.gui.move_completed().is_none());
        assert_eq!(o.gui.ip_filter_file(Path::new("S")), Path::new("S").join("ipfilter.dat"));

        let mut changed = o.clone();
        changed.settings.connections_limit = 77;
        changed.settings.enable_lsd = false;
        changed.gui = GuiOptions {
            download_dir: r"D:\in".into(),
            move_completed: true,
            completed_dir: r"D:\done".into(),
            move_only_default: false,
            torrent_folders: TorrentFolders {
                store: true,
                store_dir: r"D:\torrents".into(),
                autoload: true,
                autoload_dir: r"D:\watch".into(),
                ..Default::default()
            },
            confirm_delete: false,
            check_association: true,
            close_to_tray: false,
            minimize_to_tray: true,
            balloons: false,
            add_stopped: true,
            show_add_dialog: false,
            random_port: true,
            pex: false,
            max_peers_per_torrent: 50,
            upload_slots_per_torrent: 4,
            seed_ratio: 0,
            seed_minutes: 90,
            seed_limit_upload: true,
            seed_limit_kib: 20,
            ip_filter: true,
            ip_filter_path: r"E:\lists\level1.p2p".into(),
            schedule: crate::scheduler::Schedule { enabled: true, limited_up_kib: 25, ..Default::default() },
        };
        changed.store(&mut prefs);
        let text = prefs.to_text();
        let mut reread = Prefs::default();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let (k, v) = line.split_once('=').unwrap();
            reread.set(k, v);
        }
        assert_eq!(Options::load(&reread, "unused"), changed);
        let goal = changed.gui.seeding_goal();
        assert_eq!(goal.goal, SeedGoal { ratio_pct: 0, minutes: 90 });
        assert_eq!(goal.action(), crate::seeding::GoalAction::LimitUpload(20 * 1024));
        assert_eq!(changed.gui.ip_filter_file(Path::new("S")), Path::new(r"E:\lists\level1.p2p"));

        changed.gui.ip_filter_path = "  ".into();
        changed.store(&mut prefs);
        assert_eq!(prefs.get("ipfilter.path"), None);
    }

    #[test]
    fn move_completed_follows_the_options() {
        let mut g = GuiOptions::load(&Prefs::default(), r"C:\in");
        g.move_completed = true;
        assert!(g.move_completed().is_none(), "no folder, nothing to move to");
        g.completed_dir = r" D:\done ".into();
        let m = g.move_completed().unwrap();
        assert_eq!(m.dir, r"D:\done");
        assert_eq!(m.only_from.as_deref(), Some(r"C:\in"));
        g.move_only_default = false;
        assert_eq!(g.move_completed().unwrap().only_from, None);

        let mut current = Some(m);
        g.completed_dir = r"E:\finished".into();
        g.update_move_completed(&mut current);
        let m = current.as_ref().unwrap();
        assert_eq!((m.dir.as_str(), m.only_from.as_deref()), (r"E:\finished", None));
        g.move_completed = false;
        g.update_move_completed(&mut current);
        assert!(current.is_none());
        g.move_completed = true;
        g.update_move_completed(&mut current);
        assert!(current.is_some());

        let mut prefs = Prefs::default();
        g.store(&mut prefs);
        assert!(prefs.to_text().contains("dir.completed=E:\\finished\n"));
        assert!(prefs.to_text().contains("dir.move_only_default=0\n"));
        g.completed_dir = String::new();
        g.store(&mut prefs);
        assert_eq!(prefs.get("dir.completed"), None);
    }

    #[test]
    fn new_torrent_defaults() {
        let mut g = GuiOptions::load(&Prefs::default(), "x");
        let mut p = AddTorrentParams::new(TorrentSource::Magnet(String::new()), "x");
        p.flags = TorrentFlags::DEFAULT;
        g.apply_to_new(&mut p);
        assert_eq!(p.flags, TorrentFlags::DEFAULT);
        assert_eq!((p.max_connections, p.max_uploads), (None, None));

        g.add_stopped = true;
        g.pex = false;
        g.max_peers_per_torrent = 50;
        g.upload_slots_per_torrent = 4;
        g.apply_to_new(&mut p);
        assert!(p.flags.contains(TorrentFlags::PAUSED | TorrentFlags::DISABLE_PEX));
        assert!(!p.flags.contains(TorrentFlags::AUTO_MANAGED));
        assert!(p.flags.contains(TorrentFlags::APPLY_IP_FILTER));
        assert_eq!((p.max_connections, p.max_uploads), (Some(50), Some(4)));
    }

    #[test]
    fn random_ports_are_in_range() {
        for _ in 0..100 {
            assert!((10_000..=65_000).contains(&random_port()));
        }
    }
}
