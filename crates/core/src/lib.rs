//! PaTorrent's BitTorrent engine.
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
//! Status (Phase 2): [`Session::start`] runs the real engine: TCP peers (BEP 3, 6, 10), HTTP and
//! UDP trackers, pread disk I/O with incremental hashing, and a persistent store. The simulated
//! engine stays available behind the `mock` feature ([`Session::start_mock`]) for frontend and
//! scale work.

mod alert;
pub mod bencode;
pub mod crypto;
mod dht;
mod disk;
mod engine;
mod error;
mod flags;
pub mod metainfo;
mod move_completed;
mod net;
mod params;
mod proto;
pub mod remote;
mod resume;
mod session;
mod settings;
mod status;
mod store;
mod types;
pub mod util;
mod utp;
mod version;

#[cfg(feature = "mock")]
mod mock;

pub use alert::{
    Alert, AlertCategory, AlertPriority, AlertQueue, BlockReason, FileOperation, ProxyOperation,
};
/// Peer classes (libtorrent `peer_class_t`, `peer_class_info`, `peer_class_type_filter`).
pub use engine::peer_class::{PeerClassId, PeerClassInfo, PeerClassTypeFilter, SocketType};
pub use error::Error;
pub use move_completed::{MoveCompleted, same_dir};
/// A blocking HTTP / HTTPS `GET` for frontends (Add Torrent from URL, RSS feeds).
pub use net::fetch;
pub use net::ip_filter::{IpFilter, IpFilterLoad, IpRange, PortFilter};
/// TLS over the OS library: HTTPS for the Web UI (servers) and pinned clients.
pub use net::tls;
pub use params::{
    AddTorrentParams, DownloadPriority, MoveFlags, QueueMove, RemoveFlags, TorrentFlags, TorrentSource,
};
/// The forgiving XML scanner (libtorrent `xml_parse`), for RSS and Atom feeds.
pub use proto::xml;
pub use session::{Session, SessionHandle, SessionParams, TorrentHandle};
pub use settings::{EncLevel, EncPolicy, MixedModeAlgorithm, ProxyType, Settings};
pub use status::{
    AnnounceEntry, BlockState, FileEntry, PartialPiece, PeerFlags, PeerInfo, PeerSource, SessionStats,
    TorrentMetadata, TorrentState, TorrentStatus, TrackerStatus,
};
pub use types::{InfoHashes, Sha1Hash, Sha256Hash, TorrentId};
pub use util::bitfield::Bitfield;
pub use version::{PEER_ID_PREFIX, USER_AGENT, VERSION};

#[cfg(feature = "mock")]
pub use mock::MockConfig;

/// Internal parsers re-exported for the cargo-fuzz targets in `fuzz/`. Not a stable API.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing {
    pub use crate::dht::{DhtSettings, DhtState, DhtTracker};
    pub use crate::disk::part_file::SlotMap as PartFileSlotMap;
    pub use crate::proto::http::{Url, parse_response, resolve_redirect};
    pub use crate::proto::http_parser::{
        parse_chunk_header, parse_head as parse_http_head, resolve_redirect_location, split_url,
    };
    pub use crate::proto::lsd::{parse as parse_lsd, render as render_lsd};
    pub use crate::proto::natpmp::parse_response as parse_natpmp_response;
    pub use crate::proto::pe_handshake::{PeError, PeHandshake, PeStep};
    pub use crate::proto::peer_wire::{
        Message as PeerWireMessage, decode, decode_dont_have, decode_ext_handshake, decode_handshake,
        identify_client,
    };
    pub use crate::proto::proxy::{
        Handshake as ProxyHandshake, Socks5Command, Step as ProxyStep, Target as ProxyTarget, udp_unwrap,
    };
    pub use crate::proto::tracker::{
        UdpResponse, parse_announce_response, parse_compact_peers, parse_scrape_response,
        udp_decode as decode_udp_tracker_response,
    };
    pub use crate::proto::upnp::{
        parse_device_description, parse_error_code, parse_external_address, parse_ssdp,
    };
    pub use crate::proto::ut_metadata::{
        Message as UtMetadataMessage, MetadataFetch, Received as UtMetadataReceived,
        decode as decode_ut_metadata,
    };
    pub use crate::proto::ut_pex::{PeerPex, TorrentPex, encode_full as encode_ut_pex_full};
    pub use crate::proto::web_peer::{WebEvent, WebPeer};
    pub use crate::proto::xml::{Token as XmlToken, parse as parse_xml};
    pub use crate::remote::fuzz_wire as remote_wire;
    pub use crate::utp::{
        ReadOutcome as UtpReadOutcome, SendOutcome as UtpSendOutcome, State as UtpState, UtpIo, UtpManager,
        UtpSettings,
    };

    /// Decodes a fast-resume record (libtorrent `read_resume_data`); a record that decodes must
    /// encode to one that decodes to the same values. Returns whether it decoded.
    pub fn resume_data_round_trip(buf: &[u8]) -> bool {
        use crate::resume::ResumeData;
        let Ok(r) = ResumeData::decode(buf) else { return false };
        let again = ResumeData::decode(&r.encode()).expect("our own encoding decodes");
        assert_eq!(again, r, "resume data round trip");
        true
    }

    /// The store's start-up read without files: `resume.db` parsed, `resume.log` replayed over
    /// it, and every record decoded. Returns how many records decoded.
    pub fn store_records(db: &[u8], log: &[u8]) -> usize {
        let mut records = std::collections::BTreeMap::new();
        let _ = crate::store::parse_db(db, &mut records);
        let intact = crate::store::replay_log(log, &mut records);
        assert!(intact <= log.len());
        records.values().filter(|r| crate::resume::ResumeData::decode(&r[..]).is_ok()).count()
    }
}
