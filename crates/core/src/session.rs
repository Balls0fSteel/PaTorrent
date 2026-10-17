//! Session and handles (libtorrent `session`, `session_handle`, `torrent_handle`).
//!
//! Handles never touch engine state. They send [`Command`]s over a channel and wake the engine
//! thread; results come back as alerts. This is libtorrent's model (`async_call` + alerts) and is
//! what lets the engine own all torrent state on one thread without locks.

use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use crate::alert::{Alert, AlertQueue};
use crate::net::ip_filter::{IpFilter, PortFilter};
use crate::params::{AddTorrentParams, DownloadPriority, QueueMove, RemoveFlags, TorrentFlags};
use crate::settings::Settings;
use crate::types::TorrentId;

/// Requests from handles to the engine thread.
#[derive(Debug)]
pub(crate) enum Command {
    AddTorrent(Box<AddTorrentParams>),
    RemoveTorrent(TorrentId, RemoveFlags),
    Pause(TorrentId),
    Resume(TorrentId),
    /// libtorrent `set_flags(flags, mask)`: bits in `mask` are set to their value in `flags`.
    SetFlags {
        id: TorrentId,
        flags: TorrentFlags,
        mask: TorrentFlags,
    },
    Queue(TorrentId, QueueMove),
    ForceRecheck(TorrentId),
    ForceReannounce(TorrentId),
    SetFilePriority {
        id: TorrentId,
        file: u32,
        priority: DownloadPriority,
    },
    PrioritizeFiles(TorrentId, Vec<DownloadPriority>),
    SetDownloadLimit(TorrentId, u32),
    SetUploadLimit(TorrentId, u32),
    MoveStorage(TorrentId, String),
    ConnectPeer(TorrentId, std::net::SocketAddr),
    AddWebSeed(TorrentId, String, crate::metainfo::WebSeedKind),
    RemoveWebSeed(TorrentId, String, crate::metainfo::WebSeedKind),
    PostWebSeeds(TorrentId),
    PostTorrentUpdates,
    PostSessionStats,
    PostPeerInfo(TorrentId),
    PostFileList(TorrentId),
    PostFileProgress(TorrentId),
    PostTrackers(TorrentId),
    PostPieceInfo(TorrentId),
    ApplySettings(Box<Settings>),
    SetIpFilter(Arc<IpFilter>),
    SetPortFilter(PortFilter),
    PauseSession,
    ResumeSession,
    /// libtorrent `reopen_network_sockets`.
    ReopenNetworkSockets,
    /// Tests: the engine sees these interfaces and routes instead of the OS's (`None`: the
    /// OS's again) and reacts as to a network change.
    #[cfg(test)]
    SetFakeNetwork(Option<(Vec<crate::net::enum_net::IpInterface>, Vec<crate::net::enum_net::IpRoute>)>),
    Shutdown,
}

/// State shared between handles and the engine thread.
pub(crate) struct Shared {
    tx: mpsc::Sender<Command>,
    /// Wakes the engine thread after a command is queued (a `mio::Waker` for the real engine).
    wake: Box<dyn Fn() + Send + Sync>,
    pub(crate) alerts: AlertQueue,
}

/// How to start a [`Session`] (libtorrent `session_params`).
#[derive(Clone, Debug, Default)]
pub struct SessionParams {
    pub settings: Settings,
    /// Directory for `resume.db` and `metadata.pack`; `None` keeps nothing between runs.
    pub state_dir: Option<std::path::PathBuf>,
}

/// Owns the engine thread. Dropping it shuts the engine down and waits for it, like
/// libtorrent's `session` destructor.
pub struct Session {
    handle: SessionHandle,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts the BitTorrent engine on its own network thread. With `state_dir`, torrents and
    /// their progress are saved there and restored on the next start.
    pub fn start(params: SessionParams) -> Result<Session, crate::Error> {
        let poll = mio::Poll::new()?;
        let waker = Arc::new(mio::Waker::new(poll.registry(), crate::engine::WAKER_TOKEN)?);
        let wake = waker.clone();
        let state_dir = params.state_dir;
        Ok(Session::spawn(
            params.settings,
            Box::new(move || {
                let _ = wake.wake();
            }),
            move |rx, shared, settings| crate::engine::run(poll, waker, rx, shared, settings, state_dir),
        ))
    }

    /// Starts a simulated engine behind the real API (see [`crate::MockConfig`]).
    #[cfg(feature = "mock")]
    pub fn start_mock(settings: Settings, config: crate::MockConfig) -> Session {
        Session::spawn(settings, Box::new(|| {}), move |rx, shared, settings| {
            crate::mock::run(rx, shared, settings, config)
        })
    }

    fn spawn(
        settings: Settings,
        wake: Box<dyn Fn() + Send + Sync>,
        engine: impl FnOnce(mpsc::Receiver<Command>, Arc<Shared>, Settings) + Send + 'static,
    ) -> Session {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared { tx, wake, alerts: AlertQueue::new(settings.alert_queue_size) });
        let engine_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("lt-network".into())
            .spawn(move || engine(rx, engine_shared, settings))
            .expect("failed to spawn the network thread");
        Session { handle: SessionHandle { shared }, thread: Some(thread) }
    }

    pub fn handle(&self) -> SessionHandle {
        self.handle.clone()
    }

    /// Stops the engine and waits for it to finish (also done on drop).
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.handle.send(Command::Shutdown);
            let _ = thread.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Cheap, cloneable, thread-safe handle to a [`Session`]. Calls are asynchronous; after the
/// session has shut down they are silently ignored (alerts stop arriving).
#[derive(Clone)]
pub struct SessionHandle {
    shared: Arc<Shared>,
}

impl SessionHandle {
    fn send(&self, cmd: Command) {
        if self.shared.tx.send(cmd).is_ok() {
            (self.shared.wake)();
        }
    }

    /// Asynchronously adds a torrent. Answered by [`Alert::TorrentAdded`] or
    /// [`Alert::AddTorrentFailed`].
    pub fn add_torrent(&self, params: AddTorrentParams) {
        self.send(Command::AddTorrent(Box::new(params)));
    }

    /// Answered by [`Alert::TorrentRemoved`] (and [`Alert::TorrentDeleted`] /
    /// [`Alert::TorrentDeleteFailed`] when deleting files).
    pub fn remove_torrent(&self, id: TorrentId, flags: RemoveFlags) {
        self.send(Command::RemoveTorrent(id, flags));
    }

    pub fn torrent(&self, id: TorrentId) -> TorrentHandle {
        TorrentHandle { id, session: self.clone() }
    }

    /// Requests an [`Alert::StateUpdate`] with every torrent whose status changed since the last
    /// request. Frontends call this once per refresh (µTorrent refreshes once a second).
    pub fn post_torrent_updates(&self) {
        self.send(Command::PostTorrentUpdates);
    }

    /// Requests an [`Alert::SessionStats`].
    pub fn post_session_stats(&self) {
        self.send(Command::PostSessionStats);
    }

    pub fn apply_settings(&self, settings: Settings) {
        self.shared.alerts.set_limit(settings.alert_queue_size);
        self.send(Command::ApplySettings(Box::new(settings)));
    }

    /// Replaces the session's IP filter (libtorrent `set_ip_filter`). It applies to torrents
    /// with [`TorrentFlags::APPLY_IP_FILTER`]: peers in blocked ranges are not added or
    /// connected, connections to them are closed, and incoming connections from them are
    /// refused, each with an [`Alert::PeerBlocked`].
    pub fn set_ip_filter(&self, filter: IpFilter) {
        self.send(Command::SetIpFilter(Arc::new(filter)));
    }

    /// Reads a block list (`ipfilter.dat` or P2P format, see [`IpFilter::parse`]) on the
    /// calling thread and makes it the session's IP filter. Returns the number of blocking rules.
    pub fn load_ip_filter(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<usize> {
        let l = IpFilter::load(path)?;
        self.set_ip_filter(l.filter);
        Ok(l.rules)
    }

    /// Replaces the session's port filter (libtorrent `set_port_filter`): peers listening on a
    /// blocked port are not added to any torrent.
    pub fn set_port_filter(&self, filter: PortFilter) {
        self.send(Command::SetPortFilter(filter));
    }

    /// Reads the network interfaces again and reopens the listen sockets, LSD and the port
    /// mappers to match, asking the routers for every port mapping again (libtorrent
    /// `reopen_network_sockets` with `reopen_map_ports`). The session does this by itself when
    /// the OS reports a network change; this is for when the caller knows better (a VPN came
    /// up through some other channel).
    pub fn reopen_network_sockets(&self) {
        self.send(Command::ReopenNetworkSockets);
    }

    /// Tests: pretend the OS reports these interfaces and routes (`None`: the real ones).
    #[cfg(test)]
    pub(crate) fn set_fake_network(
        &self,
        net: Option<(Vec<crate::net::enum_net::IpInterface>, Vec<crate::net::enum_net::IpRoute>)>,
    ) {
        self.send(Command::SetFakeNetwork(net));
    }

    /// Pauses every torrent without changing their own paused/auto-managed flags.
    pub fn pause(&self) {
        self.send(Command::PauseSession);
    }

    pub fn resume(&self) {
        self.send(Command::ResumeSession);
    }

    /// See [`AlertQueue::set_notify`].
    pub fn set_alert_notify(&self, f: impl Fn() + Send + Sync + 'static) {
        self.shared.alerts.set_notify(Some(Arc::new(f)));
    }

    pub fn clear_alert_notify(&self) {
        self.shared.alerts.set_notify(None);
    }

    /// Takes all pending alerts (see [`AlertQueue::pop_all`]).
    pub fn pop_alerts(&self, out: &mut Vec<Alert>) {
        self.shared.alerts.pop_all(out);
    }
}

/// Handle to one torrent (libtorrent `torrent_handle`). Commands to a removed torrent are
/// ignored.
#[derive(Clone)]
pub struct TorrentHandle {
    id: TorrentId,
    session: SessionHandle,
}

impl TorrentHandle {
    pub fn id(&self) -> TorrentId {
        self.id
    }

    pub fn pause(&self) {
        self.session.send(Command::Pause(self.id));
    }

    pub fn resume(&self) {
        self.session.send(Command::Resume(self.id));
    }

    pub fn set_flags(&self, flags: TorrentFlags) {
        self.session.send(Command::SetFlags { id: self.id, flags, mask: flags });
    }

    pub fn unset_flags(&self, flags: TorrentFlags) {
        self.session.send(Command::SetFlags { id: self.id, flags: TorrentFlags::empty(), mask: flags });
    }

    pub fn queue_position(&self, m: QueueMove) {
        self.session.send(Command::Queue(self.id, m));
    }

    pub fn force_recheck(&self) {
        self.session.send(Command::ForceRecheck(self.id));
    }

    pub fn force_reannounce(&self) {
        self.session.send(Command::ForceReannounce(self.id));
    }

    pub fn set_file_priority(&self, file: u32, priority: DownloadPriority) {
        self.session.send(Command::SetFilePriority { id: self.id, file, priority });
    }

    pub fn prioritize_files(&self, priorities: Vec<DownloadPriority>) {
        self.session.send(Command::PrioritizeFiles(self.id, priorities));
    }

    /// Bytes per second; 0 = unlimited.
    pub fn set_download_limit(&self, limit: u32) {
        self.session.send(Command::SetDownloadLimit(self.id, limit));
    }

    pub fn set_upload_limit(&self, limit: u32) {
        self.session.send(Command::SetUploadLimit(self.id, limit));
    }

    /// Adds `addr` to the torrent's peer list and connects to it (libtorrent `connect_peer`).
    pub fn connect_peer(&self, addr: std::net::SocketAddr) {
        self.session.send(Command::ConnectPeer(self.id, addr));
    }

    /// Adds a BEP 19 web seed (libtorrent `add_url_seed`). Web seeds from the torrent file
    /// and added ones are saved with the torrent.
    pub fn add_url_seed(&self, url: impl Into<String>) {
        self.session.send(Command::AddWebSeed(self.id, url.into(), crate::metainfo::WebSeedKind::UrlSeed));
    }

    /// libtorrent `remove_url_seed`: a connection to it is closed.
    pub fn remove_url_seed(&self, url: impl Into<String>) {
        self.session.send(Command::RemoveWebSeed(self.id, url.into(), crate::metainfo::WebSeedKind::UrlSeed));
    }

    /// Adds a BEP 17 (Hoffman style) HTTP seed (libtorrent `add_http_seed`).
    pub fn add_http_seed(&self, url: impl Into<String>) {
        self.session.send(Command::AddWebSeed(self.id, url.into(), crate::metainfo::WebSeedKind::HttpSeed));
    }

    pub fn remove_http_seed(&self, url: impl Into<String>) {
        self.session.send(Command::RemoveWebSeed(
            self.id,
            url.into(),
            crate::metainfo::WebSeedKind::HttpSeed,
        ));
    }

    /// Answered by [`Alert::WebSeeds`].
    pub fn post_web_seeds(&self) {
        self.session.send(Command::PostWebSeeds(self.id));
    }

    pub fn move_storage(&self, save_path: impl Into<String>) {
        self.session.send(Command::MoveStorage(self.id, save_path.into()));
    }

    /// Answered by [`Alert::PeerInfo`].
    pub fn post_peer_info(&self) {
        self.session.send(Command::PostPeerInfo(self.id));
    }

    /// Answered by [`Alert::FileList`].
    pub fn post_file_list(&self) {
        self.session.send(Command::PostFileList(self.id));
    }

    /// Answered by [`Alert::FileProgress`].
    pub fn post_file_progress(&self) {
        self.session.send(Command::PostFileProgress(self.id));
    }

    /// Answered by [`Alert::TrackerList`].
    pub fn post_trackers(&self) {
        self.session.send(Command::PostTrackers(self.id));
    }

    /// Answered by [`Alert::PieceInfo`].
    pub fn post_piece_info(&self) {
        self.session.send(Command::PostPieceInfo(self.id));
    }
}
