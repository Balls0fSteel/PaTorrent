//! Alerts: the engine's only channel back to frontends (libtorrent `alert` + `alert_manager`).
//!
//! The queue is bounded (`Settings::alert_queue_size`). As in libtorrent, high-priority alerts
//! may use twice the limit and critical ones three times; anything beyond that is dropped and
//! counted, and the next [`AlertQueue::pop_all`] reports the loss as [`Alert::AlertsDropped`].
//!
//! `Settings::alert_mask` picks the [`AlertCategory`]s the engine posts (libtorrent
//! `alert_mask`); the replies to `post_*` requests are posted whatever it says.

use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::Error;
use crate::flags::bit_flags;
use crate::status::{
    AnnounceEntry, FileEntry, PartialPiece, PeerInfo, SessionStats, TorrentState, TorrentStatus,
};
use crate::types::{InfoHashes, TorrentId};
use crate::util::bitfield::Bitfield;

bit_flags! {
    /// libtorrent `alert_category`, the categories PaTorrent's alerts use, at libtorrent's bit
    /// positions (so an `alert_mask` means the same in both).
    pub struct AlertCategory: u32 {
        const ERROR = 0;
        const PEER = 1;
        const PORT_MAPPING = 2;
        const STORAGE = 3;
        const TRACKER = 4;
        /// Peers connecting and disconnecting ([`Alert::PeerDisconnected`]): chatty, off by
        /// default as in libtorrent.
        const CONNECT = 5;
        const STATUS = 6;
        const IP_BLOCK = 8;
        const DHT = 10;
        const SESSION_LOG = 13;
        const TORRENT_LOG = 14;
        const PORT_MAPPING_LOG = 19;
    }
}

impl AlertCategory {
    /// `Settings::alert_mask`'s default: everything but [`AlertCategory::CONNECT`]. A deliberate
    /// difference from libtorrent, whose default is errors only: PaTorrent's frontends read the
    /// status, tracker, storage and log alerts.
    pub const DEFAULT: AlertCategory =
        AlertCategory(AlertCategory::all().bits() & !AlertCategory::CONNECT.bits());
}

/// libtorrent `alert_priority`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum AlertPriority {
    Normal = 0,
    High = 1,
    Critical = 2,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PortmapTransport {
    Upnp,
    NatPmp,
}

/// Why a peer was refused (libtorrent `peer_blocked_alert::reason_t`, the reasons we have).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockReason {
    IpFilter,
    PortFilter,
    /// An incoming uTP connection while `enable_incoming_utp` is off.
    UtpDisabled,
    /// An incoming TCP connection while `enable_incoming_tcp` is off.
    TcpDisabled,
    /// A web seed on a local address whose URL has a query string, refused by
    /// `ssrf_mitigation` (libtorrent `torrent::connect_web_seed`).
    SsrfMitigation,
}

/// What failed when the proxy did (libtorrent `operation_t`, the values `socks5_alert` uses).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProxyOperation {
    HostnameLookup,
    Connect,
    Handshake,
    SockRead,
}

/// The file system operation that failed (libtorrent `operation_t`, the values the storage
/// uses).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileOperation {
    Unknown,
    FileStat,
    Mkdir,
    FileRename,
    FileCopy,
    PartfileMove,
}

#[derive(Clone, Debug)]
pub enum Alert {
    // Torrent lifecycle
    TorrentAdded {
        id: TorrentId,
        info_hashes: InfoHashes,
        name: String,
    },
    AddTorrentFailed {
        name: String,
        error: Error,
    },
    TorrentRemoved {
        id: TorrentId,
        info_hashes: InfoHashes,
    },
    /// The session store's torrents are back, at start: `count` of them, from `dir`. They post
    /// no [`Alert::TorrentAdded`] each (at 100,000 torrents those flooded the queue); the first
    /// status update names them all. PaTorrent's own: libtorrent leaves restoring to its client.
    TorrentsRestored {
        count: u32,
        dir: String,
    },
    TorrentDeleted {
        id: TorrentId,
        info_hashes: InfoHashes,
    },
    TorrentDeleteFailed {
        id: TorrentId,
        info_hashes: InfoHashes,
        error: Error,
    },
    /// `move_storage` finished: the torrent's save path is now `new` (libtorrent
    /// `storage_moved_alert`).
    StorageMoved {
        id: TorrentId,
        old: String,
        new: String,
    },
    /// `move_storage` failed; the save path is unchanged and moved files were moved back
    /// (libtorrent `storage_moved_failed_alert`). `file` is the path the error is about, empty
    /// if none.
    StorageMovedFailed {
        id: TorrentId,
        error: Error,
        file: String,
        operation: FileOperation,
    },
    /// `rename_file` finished: file `index` is now stored as `new_name` (libtorrent
    /// `file_renamed_alert`). Both names are relative to the save path unless absolute.
    FileRenamed {
        id: TorrentId,
        index: u32,
        old_name: String,
        new_name: String,
    },
    /// `rename_file` failed; the file keeps its name (libtorrent `file_rename_failed_alert`).
    FileRenameFailed {
        id: TorrentId,
        index: u32,
        error: Error,
    },
    StateChanged {
        id: TorrentId,
        state: TorrentState,
        prev_state: TorrentState,
    },
    TorrentPaused {
        id: TorrentId,
    },
    TorrentResumed {
        id: TorrentId,
    },
    TorrentChecked {
        id: TorrentId,
    },
    TorrentFinished {
        id: TorrentId,
    },
    MetadataReceived {
        id: TorrentId,
    },
    /// Metadata from peers did not match the info-hash, or could not be parsed.
    MetadataFailed {
        id: TorrentId,
        error: Error,
    },
    TorrentError {
        id: TorrentId,
        error: Error,
    },
    FileError {
        id: TorrentId,
        file: String,
        error: Error,
    },

    // Trackers
    TrackerReply {
        id: TorrentId,
        url: String,
        num_peers: u32,
    },
    TrackerWarning {
        id: TorrentId,
        url: String,
        message: String,
    },
    TrackerError {
        id: TorrentId,
        url: String,
        times_in_row: u32,
        message: String,
    },
    ScrapeReply {
        id: TorrentId,
        url: String,
        complete: u32,
        incomplete: u32,
    },
    /// A scrape failed (libtorrent `scrape_failed_alert`). Unlike an announce failure it does
    /// not count against the tracker: its status, failure count and next announce stay as they
    /// were.
    ScrapeFailed {
        id: TorrentId,
        url: String,
        message: String,
    },

    // Network
    ListenSucceeded {
        address: SocketAddr,
        protocol: Protocol,
    },
    ListenFailed {
        address: String,
        protocol: Protocol,
        error: Error,
    },
    PortmapSucceeded {
        transport: PortmapTransport,
        protocol: Protocol,
        external_port: u16,
    },
    PortmapFailed {
        transport: PortmapTransport,
        message: String,
    },
    /// What the port mappers are doing (libtorrent `portmap_log_alert`): chatty, for debugging
    /// a router that does not map.
    PortmapLog {
        transport: PortmapTransport,
        message: String,
    },
    ExternalIp {
        ip: IpAddr,
    },
    /// A DHT lookup for a torrent returned peers (libtorrent `dht_reply_alert`).
    DhtReply {
        id: TorrentId,
        num_peers: u32,
    },
    /// The DHT finished its startup bootstrap (libtorrent `dht_bootstrap_alert`).
    DhtBootstrap,
    /// A local peer announced a torrent we have (libtorrent `lsd_peer_alert`).
    LsdPeer {
        id: TorrentId,
        address: SocketAddr,
    },
    /// Local service discovery failed on an interface (libtorrent `lsd_error_alert`).
    LsdError {
        address: IpAddr,
        error: Error,
    },
    /// A peer was banned for sending bad data (libtorrent `peer_ban_alert`): it is never
    /// connected again while the torrent is loaded. `peer_id` is all zeros when the peer was
    /// not connected at the time.
    PeerBanned {
        id: TorrentId,
        addr: SocketAddr,
        peer_id: [u8; 20],
    },
    /// A peer was refused or disconnected by the IP or port filter (libtorrent
    /// `peer_blocked_alert`). `id` is `None` for an incoming connection refused before it
    /// named a torrent.
    PeerBlocked {
        id: Option<TorrentId>,
        addr: SocketAddr,
        reason: BlockReason,
    },
    /// A peer connection closed, or an outgoing one could not be made (libtorrent
    /// `peer_disconnected_alert`; [`AlertCategory::CONNECT`], off by default). `reason` says why
    /// ("connection closed by peer", "both sides are seeds", "connect timed out", ...); `error`
    /// is set when it failed (a connect, handshake or protocol error) rather than ended. `id` is
    /// `None` for an incoming connection closed before it named a torrent, `peer_id` all zeros
    /// before the handshake.
    PeerDisconnected {
        id: Option<TorrentId>,
        addr: SocketAddr,
        peer_id: [u8; 20],
        utp: bool,
        reason: String,
        error: bool,
    },
    /// The proxy failed (libtorrent `socks5_alert`, also for HTTP and SOCKS4 proxies): its name
    /// did not resolve, the SOCKS5 UDP associate could not be made or was lost, or it refused a
    /// connection for a reason of its own (authentication, protocol). `proxy` is
    /// `scheme://host:port`.
    Proxy {
        proxy: String,
        operation: ProxyOperation,
        error: String,
    },
    /// A web seed failed: an HTTP error, a broken response, a failed lookup or connect, or a
    /// URL that cannot be used (libtorrent `url_seed_alert`).
    UrlSeed {
        id: TorrentId,
        url: String,
        message: String,
    },

    // Replies to `post_*` requests
    /// Torrents whose status changed since the previous `post_torrent_updates()`.
    StateUpdate(Vec<TorrentStatus>),
    SessionStats(SessionStats),
    /// Every torrent's resume data has been written (the reply to
    /// [`SessionHandle::save_resume_data`](crate::SessionHandle::save_resume_data); sent also
    /// when the session keeps no state).
    ResumeDataSaved,
    PeerInfo {
        id: TorrentId,
        peers: Vec<PeerInfo>,
    },
    FileList {
        id: TorrentId,
        files: Arc<[FileEntry]>,
    },
    /// Completed bytes per file, in file order.
    FileProgress {
        id: TorrentId,
        done: Vec<u64>,
    },
    TrackerList {
        id: TorrentId,
        trackers: Vec<AnnounceEntry>,
    },
    /// Our pieces, the swarm's availability per piece, and the pieces being downloaded (with
    /// at least one block requested or on disk) with their blocks, ascending.
    PieceInfo {
        id: TorrentId,
        have: Bitfield,
        availability: Vec<u32>,
        downloading: Vec<PartialPiece>,
    },
    /// The torrent's web seeds, banned ones left out (libtorrent `url_seeds()` and
    /// `http_seeds()`).
    WebSeeds {
        id: TorrentId,
        url_seeds: Vec<String>,
        http_seeds: Vec<String>,
    },
    /// The torrent as a `.torrent` file (libtorrent `write_torrent_file` of `torrent_file()`):
    /// its info dictionary verbatim plus the trackers and web seeds in use. `None` while the
    /// metadata is unknown (a magnet still fetching it).
    TorrentFile {
        id: TorrentId,
        data: Option<Arc<[u8]>>,
    },

    /// Free-form log line for the Logger tab (libtorrent `log_alert` / `torrent_log_alert`).
    Log {
        id: Option<TorrentId>,
        message: String,
    },
    /// The queue overflowed and this many alerts were lost.
    AlertsDropped {
        count: u64,
    },
    /// A remote session ([`crate::remote`]) lost its connection to the server: nothing more
    /// arrives, and commands go nowhere. PaTorrent's own.
    RemoteDisconnected {
        message: String,
    },
}

impl Alert {
    /// Delivery priority, matching libtorrent's per-alert `priority`.
    pub fn priority(&self) -> AlertPriority {
        use Alert::*;
        match self {
            TorrentAdded { .. }
            | AddTorrentFailed { .. }
            | TorrentRemoved { .. }
            | TorrentDeleted { .. }
            | TorrentDeleteFailed { .. }
            | StorageMoved { .. }
            | StorageMovedFailed { .. }
            | FileRenamed { .. }
            | FileRenameFailed { .. }
            | ScrapeReply { .. }
            | ScrapeFailed { .. }
            | ListenSucceeded { .. }
            | ListenFailed { .. }
            | SessionStats(_)
            | ResumeDataSaved
            | PeerInfo { .. }
            | FileList { .. }
            | FileProgress { .. }
            | TrackerList { .. }
            | PieceInfo { .. }
            | WebSeeds { .. }
            | TorrentFile { .. }
            | AlertsDropped { .. }
            | RemoteDisconnected { .. } => AlertPriority::Critical,
            StateChanged { .. }
            | TorrentsRestored { .. }
            | TorrentPaused { .. }
            | TorrentResumed { .. }
            | TorrentChecked { .. }
            | TorrentFinished { .. }
            | TorrentError { .. }
            | FileError { .. }
            | TrackerError { .. }
            | StateUpdate(_) => AlertPriority::High,
            MetadataReceived { .. }
            | MetadataFailed { .. }
            | TrackerReply { .. }
            | TrackerWarning { .. }
            | PortmapSucceeded { .. }
            | PortmapFailed { .. }
            | PortmapLog { .. }
            | ExternalIp { .. }
            | DhtReply { .. }
            | DhtBootstrap
            | LsdPeer { .. }
            | LsdError { .. }
            | PeerBanned { .. }
            | PeerBlocked { .. }
            | PeerDisconnected { .. }
            | UrlSeed { .. }
            | Proxy { .. }
            | Log { .. } => AlertPriority::Normal,
        }
    }

    /// libtorrent's `static_category` of the alert (of the alert it stands for). Empty for the
    /// alerts the mask never holds back ([`Alert::masked_by`]).
    pub fn category(&self) -> AlertCategory {
        use Alert::*;
        type C = AlertCategory;
        match self {
            TorrentAdded { .. }
            | AddTorrentFailed { .. }
            | TorrentRemoved { .. }
            | TorrentsRestored { .. }
            | StateChanged { .. }
            | TorrentPaused { .. }
            | TorrentResumed { .. }
            | TorrentChecked { .. }
            | TorrentFinished { .. }
            | MetadataReceived { .. }
            | ListenSucceeded { .. }
            | ExternalIp { .. } => C::STATUS,
            TorrentDeleted { .. } | StorageMoved { .. } | FileRenamed { .. } => C::STORAGE,
            TorrentDeleteFailed { .. } | StorageMovedFailed { .. } | FileRenameFailed { .. } => {
                C::STORAGE | C::ERROR
            }
            FileError { .. } => C::STATUS | C::STORAGE,
            MetadataFailed { .. } | LsdError { .. } | Proxy { .. } => C::ERROR,
            TorrentError { .. } | ListenFailed { .. } => C::STATUS | C::ERROR,
            TrackerReply { .. } | ScrapeReply { .. } => C::TRACKER,
            TrackerWarning { .. } | TrackerError { .. } | ScrapeFailed { .. } => C::TRACKER | C::ERROR,
            PortmapSucceeded { .. } => C::PORT_MAPPING,
            PortmapFailed { .. } => C::PORT_MAPPING | C::ERROR,
            PortmapLog { .. } => C::PORT_MAPPING_LOG,
            DhtReply { .. } => C::DHT | C::TRACKER,
            DhtBootstrap => C::DHT,
            LsdPeer { .. } | PeerBanned { .. } => C::PEER,
            PeerBlocked { .. } => C::IP_BLOCK,
            PeerDisconnected { .. } => C::CONNECT,
            UrlSeed { .. } => C::PEER | C::ERROR,
            Log { id: Some(_), .. } => C::TORRENT_LOG,
            Log { id: None, .. } => C::SESSION_LOG,
            // replies to requests, and the queue's and the remote session's own news
            StateUpdate(_)
            | SessionStats(_)
            | ResumeDataSaved
            | PeerInfo { .. }
            | FileList { .. }
            | FileProgress { .. }
            | TrackerList { .. }
            | PieceInfo { .. }
            | WebSeeds { .. }
            | TorrentFile { .. }
            | AlertsDropped { .. }
            | RemoteDisconnected { .. } => C::empty(),
        }
    }

    /// Whether `mask` (`Settings::alert_mask`) holds the alert back: none of its categories is
    /// in the mask. Replies to requests ([`Alert::category`] empty) always get through.
    pub fn masked_by(&self, mask: AlertCategory) -> bool {
        let c = self.category();
        !c.is_empty() && !c.intersects(mask)
    }

    /// The torrent this alert is about, if any.
    pub fn torrent(&self) -> Option<TorrentId> {
        use Alert::*;
        match *self {
            TorrentAdded { id, .. }
            | TorrentRemoved { id, .. }
            | TorrentDeleted { id, .. }
            | TorrentDeleteFailed { id, .. }
            | StorageMoved { id, .. }
            | StorageMovedFailed { id, .. }
            | FileRenamed { id, .. }
            | FileRenameFailed { id, .. }
            | StateChanged { id, .. }
            | TorrentPaused { id }
            | TorrentResumed { id }
            | TorrentChecked { id }
            | TorrentFinished { id }
            | MetadataReceived { id }
            | MetadataFailed { id, .. }
            | TorrentError { id, .. }
            | FileError { id, .. }
            | TrackerReply { id, .. }
            | DhtReply { id, .. }
            | LsdPeer { id, .. }
            | PeerBanned { id, .. }
            | TrackerWarning { id, .. }
            | TrackerError { id, .. }
            | ScrapeReply { id, .. }
            | ScrapeFailed { id, .. }
            | PeerInfo { id, .. }
            | FileList { id, .. }
            | FileProgress { id, .. }
            | TrackerList { id, .. }
            | PieceInfo { id, .. }
            | UrlSeed { id, .. }
            | WebSeeds { id, .. }
            | TorrentFile { id, .. } => Some(id),
            Log { id, .. } | PeerBlocked { id, .. } | PeerDisconnected { id, .. } => id,
            _ => None,
        }
    }

    /// One human-readable line, for logs (libtorrent `alert::message()`).
    pub fn message(&self) -> String {
        use Alert::*;
        let mut s = String::new();
        let _ = match self {
            TorrentAdded { name, .. } => write!(s, "{name}: added"),
            AddTorrentFailed { name, error } => write!(s, "{name}: failed to add: {error}"),
            TorrentRemoved { info_hashes, .. } => write!(s, "{}: removed", info_hashes.key_hex()),
            TorrentsRestored { count, dir } => write!(s, "restored {count} torrents from {dir}"),
            TorrentDeleted { info_hashes, .. } => write!(s, "{}: files deleted", info_hashes.key_hex()),
            TorrentDeleteFailed { info_hashes, error, .. } => {
                write!(s, "{}: failed to delete files: {error}", info_hashes.key_hex())
            }
            StorageMoved { old, new, .. } => write!(s, "storage moved from \"{old}\" to \"{new}\""),
            StorageMovedFailed { error, file, operation, .. } => {
                write!(s, "storage move failed. {operation:?} ({file}): {error}")
            }
            FileRenamed { index, old_name, new_name, .. } => {
                write!(s, "file {index} renamed from \"{old_name}\" to \"{new_name}\"")
            }
            FileRenameFailed { index, error, .. } => write!(s, "failed to rename file {index}: {error}"),
            StateChanged { state, prev_state, .. } => write!(s, "state changed: {prev_state:?} -> {state:?}"),
            TorrentPaused { .. } => write!(s, "paused"),
            TorrentResumed { .. } => write!(s, "resumed"),
            TorrentChecked { .. } => write!(s, "checked"),
            TorrentFinished { .. } => write!(s, "finished downloading"),
            MetadataReceived { .. } => write!(s, "metadata received"),
            MetadataFailed { error, .. } => write!(s, "metadata failed: {error}"),
            TorrentError { error, .. } => write!(s, "error: {error}"),
            FileError { file, error, .. } => write!(s, "file error ({file}): {error}"),
            TrackerReply { url, num_peers, .. } => write!(s, "{url}: received {num_peers} peers"),
            TrackerWarning { url, message, .. } => write!(s, "{url}: warning: {message}"),
            TrackerError { url, times_in_row, message, .. } => {
                write!(s, "{url}: error ({times_in_row} in a row): {message}")
            }
            ScrapeReply { url, complete, incomplete, .. } => {
                write!(s, "{url}: scrape: {complete} seeds, {incomplete} peers")
            }
            ScrapeFailed { url, message, .. } => write!(s, "{url}: scrape failed: {message}"),
            ListenSucceeded { address, protocol } => write!(s, "listening on {address} ({protocol:?})"),
            ListenFailed { address, protocol, error } => {
                write!(s, "failed to listen on {address} ({protocol:?}): {error}")
            }
            PortmapSucceeded { transport, protocol, external_port } => {
                write!(s, "{transport:?}: mapped {protocol:?} port {external_port}")
            }
            PortmapFailed { transport, message } => write!(s, "{transport:?}: mapping failed: {message}"),
            PortmapLog { transport, message } => write!(s, "{transport:?}: {message}"),
            ExternalIp { ip } => write!(s, "external IP: {ip}"),
            DhtReply { num_peers, .. } => write!(s, "DHT: received {num_peers} peers"),
            DhtBootstrap => write!(s, "DHT bootstrapped"),
            LsdPeer { address, .. } => write!(s, "LSD: local peer {address}"),
            LsdError { address, error } => write!(s, "LSD failed on {address}: {error}"),
            PeerBanned { addr, .. } => write!(s, "peer {addr} banned"),
            PeerBlocked { addr, reason, .. } => write!(s, "peer {addr} blocked ({reason:?})"),
            PeerDisconnected { addr, utp, reason, error, .. } => {
                let (over, how) =
                    (if *utp { "uTP" } else { "TCP" }, if *error { "failed" } else { "disconnected" });
                write!(s, "peer {addr} ({over}) {how}: {reason}")
            }
            UrlSeed { url, message, .. } => write!(s, "web seed {url}: {message}"),
            Proxy { proxy, operation, error } => {
                write!(s, "proxy error. op: {operation:?} ec: {error} ep: {proxy}")
            }
            WebSeeds { url_seeds, http_seeds, .. } => {
                write!(s, "{} web seeds", url_seeds.len() + http_seeds.len())
            }
            TorrentFile { data, .. } => match data {
                Some(d) => write!(s, ".torrent file of {} bytes", d.len()),
                None => write!(s, "no metadata for a .torrent file"),
            },
            StateUpdate(v) => write!(s, "state update for {} torrents", v.len()),
            SessionStats(_) => write!(s, "session stats"),
            ResumeDataSaved => write!(s, "resume data saved"),
            PeerInfo { peers, .. } => write!(s, "{} peers", peers.len()),
            FileList { files, .. } => write!(s, "{} files", files.len()),
            FileProgress { done, .. } => write!(s, "progress for {} files", done.len()),
            TrackerList { trackers, .. } => write!(s, "{} trackers", trackers.len()),
            PieceInfo { have, .. } => write!(s, "{} of {} pieces", have.count(), have.len()),
            Log { message, .. } => write!(s, "{message}"),
            AlertsDropped { count } => write!(s, "{count} alerts dropped (queue full)"),
            RemoteDisconnected { message } => write!(s, "disconnected from the remote session: {message}"),
        };
        s
    }
}

type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Queue {
    alerts: Vec<Alert>,
    dropped: u64,
}

/// Bounded, thread-safe alert queue with an empty→non-empty notification.
pub struct AlertQueue {
    queue: Mutex<Queue>,
    notify: Mutex<Option<Notify>>,
    limit: AtomicUsize,
}

impl AlertQueue {
    pub fn new(limit: usize) -> AlertQueue {
        AlertQueue {
            queue: Mutex::default(),
            notify: Mutex::new(None),
            limit: AtomicUsize::new(limit.max(1)),
        }
    }

    pub fn set_limit(&self, limit: usize) {
        self.limit.store(limit.max(1), Ordering::Relaxed);
    }

    /// `f` runs on the engine thread whenever the queue goes from empty to non-empty. It must be
    /// cheap and must not call back into the session. Typically it posts a wake-up message to the
    /// frontend's event loop, which then calls [`AlertQueue::pop_all`].
    pub fn set_notify(&self, f: Option<Notify>) {
        *lock(&self.notify) = f;
    }

    /// Queues `alert`. Returns `false` if it was dropped because the queue is full.
    pub fn post(&self, alert: Alert) -> bool {
        let limit = self.limit.load(Ordering::Relaxed);
        let was_empty = {
            let mut q = lock(&self.queue);
            if q.alerts.len() / (1 + alert.priority() as usize) >= limit {
                q.dropped += 1;
                return false;
            }
            let was_empty = q.alerts.is_empty();
            q.alerts.push(alert);
            was_empty
        };
        if was_empty {
            let notify = lock(&self.notify).clone();
            if let Some(f) = notify {
                f();
            }
        }
        true
    }

    /// Moves every queued alert into `out` (which is cleared first), preceded by
    /// [`Alert::AlertsDropped`] if anything was lost since the last call.
    pub fn pop_all(&self, out: &mut Vec<Alert>) {
        out.clear();
        let mut q = lock(&self.queue);
        if q.dropped > 0 {
            out.push(Alert::AlertsDropped { count: q.dropped });
            q.dropped = 0;
        }
        if out.is_empty() {
            std::mem::swap(out, &mut q.alerts);
        } else {
            out.append(&mut q.alerts);
        }
    }
}

/// A panic while holding these locks cannot leave the data inconsistent, so poisoning is ignored.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn log(i: u32) -> Alert {
        Alert::Log { id: None, message: i.to_string() }
    }

    #[test]
    fn notify_fires_on_empty_to_non_empty_only() {
        let q = AlertQueue::new(100);
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        q.set_notify(Some(Arc::new(move || {
            h.fetch_add(1, Ordering::Relaxed);
        })));
        q.post(log(1));
        q.post(log(2));
        assert_eq!(hits.load(Ordering::Relaxed), 1);
        let mut out = Vec::new();
        q.pop_all(&mut out);
        assert_eq!(out.len(), 2);
        q.post(log(3));
        assert_eq!(hits.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn the_mask_holds_back_its_categories_but_never_replies() {
        let addr: SocketAddr = "10.0.0.1:6881".parse().unwrap();
        let disconnected = Alert::PeerDisconnected {
            id: None,
            addr,
            peer_id: [0; 20],
            utp: false,
            reason: "connect timed out".into(),
            error: true,
        };
        assert!(disconnected.masked_by(AlertCategory::DEFAULT), "connect is off by default");
        assert!(!disconnected.masked_by(AlertCategory::DEFAULT | AlertCategory::CONNECT));
        assert_eq!(disconnected.message(), "peer 10.0.0.1:6881 (TCP) failed: connect timed out");
        // an alert in two categories gets through on either
        let tracker_error = Alert::TrackerError {
            id: TorrentId::default(),
            url: "u".into(),
            times_in_row: 1,
            message: "m".into(),
        };
        assert!(!tracker_error.masked_by(AlertCategory::ERROR));
        assert!(!tracker_error.masked_by(AlertCategory::TRACKER));
        assert!(tracker_error.masked_by(AlertCategory::STATUS));
        // the default holds back nothing PaTorrent posted before the mask
        assert!(!Alert::TorrentFinished { id: TorrentId::default() }.masked_by(AlertCategory::DEFAULT));
        assert!(!log(1).masked_by(AlertCategory::DEFAULT));
        // a request's reply ignores the mask
        assert!(!Alert::SessionStats(SessionStats::default()).masked_by(AlertCategory::empty()));
        assert!(!Alert::StateUpdate(Vec::new()).masked_by(AlertCategory::empty()));
        assert!(log(1).masked_by(AlertCategory::empty()));
        // libtorrent's bit positions
        assert_eq!(AlertCategory::CONNECT.bits(), 1 << 5);
        assert_eq!(AlertCategory::PORT_MAPPING_LOG.bits(), 1 << 19);
    }

    #[test]
    fn limits_scale_with_priority_and_drops_are_reported() {
        let q = AlertQueue::new(2);
        assert!(q.post(log(1)));
        assert!(q.post(log(2)));
        assert!(!q.post(log(3)), "normal alerts stop at the limit");
        assert!(q.post(Alert::TorrentFinished { id: TorrentId::default() }), "high gets 2x");
        assert!(q.post(Alert::TorrentFinished { id: TorrentId::default() }));
        assert!(!q.post(Alert::TorrentFinished { id: TorrentId::default() }));
        assert!(q.post(Alert::SessionStats(SessionStats::default())), "critical gets 3x");
        assert!(q.post(Alert::SessionStats(SessionStats::default())));
        assert!(!q.post(Alert::SessionStats(SessionStats::default())));

        let mut out = Vec::new();
        q.pop_all(&mut out);
        assert!(matches!(out[0], Alert::AlertsDropped { count: 3 }));
        assert_eq!(out.len(), 7);
        q.pop_all(&mut out);
        assert!(out.is_empty());
    }
}
