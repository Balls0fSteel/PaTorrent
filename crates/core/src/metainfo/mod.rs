//! Torrent metadata: `.torrent` files, magnet links and the file layout.
//!
//! Ported from libtorrent-rasterbar 2.0.11 (see each file's header). The parsers keep
//! libtorrent's limits and error codes, so hostile metadata is rejected the same way.

mod error;
pub mod file_storage;
pub mod magnet;
pub mod merkle;
pub mod sanitize;
pub mod torrent_info;

pub use error::MetainfoError;
pub use file_storage::{FileFlags, FileSlice, FileStorage, PeerRequest};
pub use magnet::{MagnetUri, make_magnet_uri, parse_magnet_uri};
pub use torrent_info::{AnnounceUrl, LoadLimits, TorrentInfo, TrackerSource, WebSeed, WebSeedKind};
