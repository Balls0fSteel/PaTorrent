//! Torrent metadata: `.torrent` files, magnet links and the file layout.
//!
//! Ported from libtorrent-rasterbar 2.0.11 (see each file's header). The parsers keep
//! libtorrent's limits and error codes, so hostile metadata is rejected the same way.

pub mod create_torrent;
mod error;
pub mod file_storage;
pub mod magnet;
pub mod merkle;
pub mod sanitize;
pub mod torrent_info;

pub use create_torrent::{CreateFlags, CreateTorrent, CreateTorrentError, add_files, set_piece_hashes};
pub use error::MetainfoError;
pub use file_storage::{FileFlags, FileSlice, FileStorage, PeerRequest};
pub use magnet::{MagnetUri, make_magnet_uri, make_magnet_uri_from_info, parse_magnet_uri};
pub use torrent_info::{AnnounceUrl, LoadLimits, TorrentInfo, TrackerSource, WebSeed, WebSeedKind};
