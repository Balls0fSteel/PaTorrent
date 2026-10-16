//! Alerts: the engine's only channel back to frontends (libtorrent `alert` + `alert_manager`).
//!
//! The queue is bounded (`Settings::alert_queue_size`). As in libtorrent, high-priority alerts
//! may use twice the limit and critical ones three times; anything beyond that is dropped and
//! counted, and the next [`AlertQueue::pop_all`] reports the loss as [`Alert::AlertsDropped`].

use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::Error;
use crate::status::{AnnounceEntry, FileEntry, PeerInfo, SessionStats, TorrentState, TorrentStatus};
use crate::types::{InfoHashes, TorrentId};
use crate::util::bitfield::Bitfield;

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
    TorrentDeleted {
        id: TorrentId,
        info_hashes: InfoHashes,
    },
    TorrentDeleteFailed {
        id: TorrentId,
        info_hashes: InfoHashes,
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

    // Replies to `post_*` requests
    /// Torrents whose status changed since the previous `post_torrent_updates()`.
    StateUpdate(Vec<TorrentStatus>),
    SessionStats(SessionStats),
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
    /// Our pieces and the swarm's availability per piece.
    PieceInfo {
        id: TorrentId,
        have: Bitfield,
        availability: Vec<u32>,
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
            | ScrapeReply { .. }
            | ListenSucceeded { .. }
            | ListenFailed { .. }
            | SessionStats(_)
            | PeerInfo { .. }
            | FileList { .. }
            | FileProgress { .. }
            | TrackerList { .. }
            | PieceInfo { .. }
            | AlertsDropped { .. } => AlertPriority::Critical,
            StateChanged { .. }
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
            | Log { .. } => AlertPriority::Normal,
        }
    }

    /// The torrent this alert is about, if any.
    pub fn torrent(&self) -> Option<TorrentId> {
        use Alert::*;
        match *self {
            TorrentAdded { id, .. }
            | TorrentRemoved { id, .. }
            | TorrentDeleted { id, .. }
            | TorrentDeleteFailed { id, .. }
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
            | TrackerWarning { id, .. }
            | TrackerError { id, .. }
            | ScrapeReply { id, .. }
            | PeerInfo { id, .. }
            | FileList { id, .. }
            | FileProgress { id, .. }
            | TrackerList { id, .. }
            | PieceInfo { id, .. } => Some(id),
            Log { id, .. } => id,
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
            TorrentDeleted { info_hashes, .. } => write!(s, "{}: files deleted", info_hashes.key_hex()),
            TorrentDeleteFailed { info_hashes, error, .. } => {
                write!(s, "{}: failed to delete files: {error}", info_hashes.key_hex())
            }
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
            StateUpdate(v) => write!(s, "state update for {} torrents", v.len()),
            SessionStats(_) => write!(s, "session stats"),
            PeerInfo { peers, .. } => write!(s, "{} peers", peers.len()),
            FileList { files, .. } => write!(s, "{} files", files.len()),
            FileProgress { done, .. } => write!(s, "progress for {} files", done.len()),
            TrackerList { trackers, .. } => write!(s, "{} trackers", trackers.len()),
            PieceInfo { have, .. } => write!(s, "{} of {} pieces", have.count(), have.len()),
            Log { message, .. } => write!(s, "{message}"),
            AlertsDropped { count } => write!(s, "{count} alerts dropped (queue full)"),
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
