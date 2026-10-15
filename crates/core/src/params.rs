//! Parameters for adding and controlling torrents (libtorrent `add_torrent_params`,
//! `torrent_flags`, `remove_flags`, `download_priority_t`).

use crate::flags::bit_flags;
use crate::types::InfoHashes;

bit_flags! {
    /// Per-torrent flags (libtorrent `torrent_flags_t`, same names and meanings).
    pub struct TorrentFlags: u32 {
        /// Assume all files exist and are complete; verify pieces lazily when first requested.
        const SEED_MODE = 0;
        /// Never download; set automatically on disk-full and similar errors.
        const UPLOAD_MODE = 1;
        const APPLY_IP_FILTER = 3;
        const PAUSED = 4;
        /// Subject to the queue: the session starts and pauses it to honour the active limits.
        const AUTO_MANAGED = 5;
        /// Adding a torrent that already exists fails instead of returning the existing one.
        const DUPLICATE_IS_ERROR = 6;
        const SUPER_SEEDING = 8;
        const SEQUENTIAL_DOWNLOAD = 9;
        /// Pause as soon as the torrent is ready to download (checked, metadata present).
        const STOP_WHEN_READY = 10;
        const NEED_SAVE_RESUME = 13;
        const DISABLE_DHT = 19;
        const DISABLE_LSD = 20;
        const DISABLE_PEX = 21;
        /// Trust resume data / seed mode without verifying files on disk.
        const NO_VERIFY_FILES = 22;
        /// New files default to "don't download" (for add-dialog style selection).
        const DEFAULT_DONT_DOWNLOAD = 23;
    }
}

impl TorrentFlags {
    /// libtorrent `torrent_flags::default_flags`: queued (paused and auto-managed).
    pub const DEFAULT: TorrentFlags = TorrentFlags(
        TorrentFlags::AUTO_MANAGED.0
            | TorrentFlags::PAUSED.0
            | TorrentFlags::APPLY_IP_FILTER.0
            | TorrentFlags::NEED_SAVE_RESUME.0,
    );
}

bit_flags! {
    /// libtorrent `remove_flags_t`.
    pub struct RemoveFlags: u8 {
        /// Delete the downloaded files too.
        const DELETE_FILES = 0;
        /// Delete only the part file holding pieces of unwanted files.
        const DELETE_PARTFILE = 1;
    }
}

/// libtorrent `download_priority_t`: 0 = skip, 1 = low, 4 = normal (default), 7 = top.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DownloadPriority(pub u8);

impl DownloadPriority {
    pub const DONT_DOWNLOAD: DownloadPriority = DownloadPriority(0);
    pub const LOW: DownloadPriority = DownloadPriority(1);
    pub const NORMAL: DownloadPriority = DownloadPriority(4);
    pub const TOP: DownloadPriority = DownloadPriority(7);
}

impl Default for DownloadPriority {
    fn default() -> DownloadPriority {
        DownloadPriority::NORMAL
    }
}

/// Queue position changes (libtorrent `queue_position_up/down/top/bottom`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QueueMove {
    Up,
    Down,
    Top,
    Bottom,
}

/// Where a torrent comes from.
#[derive(Clone, Debug)]
pub enum TorrentSource {
    /// Contents of a `.torrent` file.
    TorrentFile(Vec<u8>),
    /// A `magnet:` URI.
    Magnet(String),
    /// Fast-resume data written by a previous session.
    Resume(Vec<u8>),
    /// A bare info-hash; metadata is fetched from peers (DHT / PEX / trackers).
    InfoHash(InfoHashes),
}

/// libtorrent `add_torrent_params`, minus the deprecated and resume-only fields.
#[derive(Clone, Debug)]
pub struct AddTorrentParams {
    pub source: TorrentSource,
    /// UTF-8 path of the download directory.
    pub save_path: String,
    /// Display name override, used until metadata is known.
    pub name: Option<String>,
    pub flags: TorrentFlags,
    /// Extra trackers; `tracker_tiers[i]` is the tier of `trackers[i]` (default 0).
    pub trackers: Vec<String>,
    pub tracker_tiers: Vec<u32>,
    pub file_priorities: Vec<DownloadPriority>,
    /// Bytes per second; 0 = unlimited.
    pub download_limit: u32,
    pub upload_limit: u32,
    pub max_connections: Option<u32>,
    pub max_uploads: Option<u32>,
}

impl AddTorrentParams {
    pub fn new(source: TorrentSource, save_path: impl Into<String>) -> AddTorrentParams {
        AddTorrentParams {
            source,
            save_path: save_path.into(),
            name: None,
            flags: TorrentFlags::DEFAULT,
            trackers: Vec::new(),
            tracker_tiers: Vec::new(),
            file_priorities: Vec::new(),
            download_limit: 0,
            upload_limit: 0,
            max_connections: None,
            max_uploads: None,
        }
    }
}
