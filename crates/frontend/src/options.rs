//! What the Preferences dialog edits: the engine's [`Settings`] plus the frontend's own options
//! (folders, tray behaviour, per-torrent defaults, the IP filter file), loaded from and stored
//! to the preferences file. Toolkit-independent, so the GTK dialog can share it.

use std::path::{Path, PathBuf};

use patorrent_core::{AddTorrentParams, Settings, TorrentFlags};

use crate::prefs::Prefs;
use crate::settings;

/// The frontend's options (preferences file keys in brackets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuiOptions {
    /// Where new downloads go (`dir.download`).
    pub download_dir: String,
    /// Ask before removing torrents (`ui.confirm_delete`).
    pub confirm_delete: bool,
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
    /// Load the IP filter at start and on Apply (`ipfilter.enable`).
    pub ip_filter: bool,
    /// The block list; empty = `ipfilter.dat` in the settings folder, like µTorrent
    /// (`ipfilter.path`).
    pub ip_filter_path: String,
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
            confirm_delete: prefs.get_bool("ui.confirm_delete", true),
            close_to_tray: prefs.get_bool("ui.close_to_tray", true),
            minimize_to_tray: prefs.get_bool("ui.minimize_to_tray", false),
            balloons: prefs.get_bool("ui.balloons", true),
            add_stopped: prefs.get_bool("ui.add_stopped", false),
            show_add_dialog: prefs.get_bool("ui.show_add_dialog", true),
            random_port: prefs.get_bool("net.random_port", false),
            pex: prefs.get_bool("bt.pex", true),
            max_peers_per_torrent: u32_of("bt.max_peers"),
            upload_slots_per_torrent: u32_of("bt.upload_slots"),
            ip_filter: prefs.get_bool("ipfilter.enable", false),
            ip_filter_path: prefs.get("ipfilter.path").unwrap_or_default().to_owned(),
        }
    }

    pub fn store(&self, prefs: &mut Prefs) {
        prefs.set("dir.download", &self.download_dir);
        prefs.set_bool("ui.confirm_delete", self.confirm_delete);
        prefs.set_bool("ui.close_to_tray", self.close_to_tray);
        prefs.set_bool("ui.minimize_to_tray", self.minimize_to_tray);
        prefs.set_bool("ui.balloons", self.balloons);
        prefs.set_bool("ui.add_stopped", self.add_stopped);
        prefs.set_bool("ui.show_add_dialog", self.show_add_dialog);
        prefs.set_bool("net.random_port", self.random_port);
        prefs.set_bool("bt.pex", self.pex);
        prefs.set("bt.max_peers", self.max_peers_per_torrent);
        prefs.set("bt.upload_slots", self.upload_slots_per_torrent);
        prefs.set_bool("ipfilter.enable", self.ip_filter);
        if self.ip_filter_path.trim().is_empty() {
            prefs.remove("ipfilter.path");
        } else {
            prefs.set("ipfilter.path", self.ip_filter_path.trim());
        }
    }

    /// The block list to load: the configured path, or `ipfilter.dat` in `settings_dir`.
    pub fn ip_filter_file(&self, settings_dir: &Path) -> PathBuf {
        match self.ip_filter_path.trim() {
            "" => settings_dir.join("ipfilter.dat"),
            p => PathBuf::from(p),
        }
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
        assert_eq!(o.gui.ip_filter_file(Path::new("S")), Path::new("S").join("ipfilter.dat"));

        let mut changed = o.clone();
        changed.settings.connections_limit = 77;
        changed.settings.enable_lsd = false;
        changed.gui = GuiOptions {
            download_dir: r"D:\in".into(),
            confirm_delete: false,
            close_to_tray: false,
            minimize_to_tray: true,
            balloons: false,
            add_stopped: true,
            show_add_dialog: false,
            random_port: true,
            pex: false,
            max_peers_per_torrent: 50,
            upload_slots_per_torrent: 4,
            ip_filter: true,
            ip_filter_path: r"E:\lists\level1.p2p".into(),
        };
        changed.store(&mut prefs);
        let text = prefs.to_text();
        let mut reread = Prefs::default();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let (k, v) = line.split_once('=').unwrap();
            reread.set(k, v);
        }
        assert_eq!(Options::load(&reread, "unused"), changed);
        assert_eq!(changed.gui.ip_filter_file(Path::new("S")), Path::new(r"E:\lists\level1.p2p"));

        changed.gui.ip_filter_path = "  ".into();
        changed.store(&mut prefs);
        assert_eq!(prefs.get("ipfilter.path"), None);
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
