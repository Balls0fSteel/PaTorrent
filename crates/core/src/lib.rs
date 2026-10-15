//! LibreTorrent's BitTorrent engine.
//!
//! This crate is a Rust port of libtorrent-rasterbar 2.0.11 (see `docs/PORTING.md`). The public
//! surface mirrors libtorrent's `session_handle` / `torrent_handle` / alert model:
//!
//! * [`Session`] owns the engine thread. [`SessionHandle`] and [`TorrentHandle`] are cheap,
//!   cloneable, `Send` handles that post commands to it.
//! * Results come back asynchronously as [`Alert`]s, drained with [`SessionHandle::pop_alerts`].
//!   [`SessionHandle::set_alert_notify`] registers a callback that fires when the alert queue goes
//!   from empty to non-empty, so frontends can wake their own event loop.
//! * [`SessionHandle::post_torrent_updates`] reports only torrents whose status changed since the
//!   previous call. This is what keeps frontends cheap with thousands of torrents loaded.
//!
//! Status (Phase 0): the API is defined and backed by a simulated engine (`mock` feature,
//! [`Session::start_mock`]). The real network engine replaces it in Phase 2 without API changes.

mod alert;
pub mod bencode;
pub mod crypto;
mod error;
mod flags;
pub mod metainfo;
mod params;
mod session;
mod settings;
mod status;
mod types;
pub mod util;
mod version;

#[cfg(feature = "mock")]
mod mock;

pub use alert::{Alert, AlertPriority, AlertQueue};
pub use error::Error;
pub use params::{AddTorrentParams, DownloadPriority, QueueMove, RemoveFlags, TorrentFlags, TorrentSource};
pub use session::{Session, SessionHandle, TorrentHandle};
pub use settings::{EncLevel, EncPolicy, Settings};
pub use status::{
    AnnounceEntry, FileEntry, PeerFlags, PeerInfo, PeerSource, SessionStats, TorrentState, TorrentStatus,
    TrackerStatus,
};
pub use types::{InfoHashes, Sha1Hash, Sha256Hash, TorrentId};
pub use util::bitfield::Bitfield;
pub use version::{PEER_ID_PREFIX, USER_AGENT, VERSION};

#[cfg(feature = "mock")]
pub use mock::MockConfig;
