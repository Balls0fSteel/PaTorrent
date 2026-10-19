// Ported from libtorrent-rasterbar 2.0.11: src/session_impl.cpp (network loop, listen sockets,
// tick, auto-management, choker, connection pacing), src/torrent.cpp (start/stop/check/finish
// transitions), src/choker.cpp, src/alert_manager.cpp usage
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: one mio poll loop owns everything (no asio strands or shared_ptr webs); torrents and
// peers live in slabs addressed by generational ids. Only running torrents are ticked. The
// choker is libtorrent's fixed-slots choker with rate-based ranking (download rate from the
// peer for downloading torrents, the `fastest_upload` seed policy for finished ones) plus one
// optimistic slot. Each family's UDP listen socket sits beside a TCP listener on the same port,
// shared by uTP and the DHT (libtorrent's `listen_socket_t`; see engine::listen for how listen
// sockets follow `listen_interfaces` and network changes); uTP sockets are driven by the
// manager's ready list instead of asio handlers.

//! The real network engine (PLAN.md Phase 2).

mod dht;
mod external_ip;
mod ip_filter;
mod listen;
mod lsd;
mod metadata;
mod natpmp;
mod pe;
pub(crate) mod peer;
pub(crate) mod peer_list;
mod pex;
pub(crate) mod picker;
mod portmap;
pub(crate) mod proxy;
mod smart_ban;
mod super_seed;
pub(crate) mod torrent;
pub(crate) mod tracker;
mod upnp;
mod web_seed;
mod wire;

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use mio::net::{TcpStream, UdpSocket};
use mio::{Events, Interest, Poll, Token, Waker};

use crate::alert::{Alert, FileOperation};
use crate::crypto::mse;
use crate::disk::{
    BLOCK_SIZE, CheckMode, CheckResult, Completion, DiskIo, FenceAction, MoveError, MoveStatus,
};
use crate::error::Error;
use crate::metainfo::{LoadLimits, MagnetUri, TorrentInfo, WebSeedKind, parse_magnet_uri};
use crate::net::ip_voter::{ExternalIp, SessionVoter, source};
use crate::net::resolver::{Resolved, Resolver};
use crate::net::{Bucket, sys};
use crate::params::{
    AddTorrentParams, DownloadPriority, MoveFlags, QueueMove, RemoveFlags, TorrentFlags, TorrentSource,
};
use crate::proto::peer_wire::{self, msg};
use crate::proto::tracker::Event;
use crate::resume::ResumeData;
use crate::session::{Command, Shared};
use crate::settings::Settings;
use crate::status::{PeerFlags, PeerInfo, PeerSource, SessionStats, TorrentState};
use crate::store::Store;
use crate::types::{Sha1Hash, TorrentId};
use crate::util::bitfield::Bitfield;
use crate::util::stat::{Stat, now_unix};
use crate::utp::{self, SendOutcome, UtpIo, UtpManager, UtpSettings};
use peer::{PeerConn, PeerState, Transport};
use picker::PiecePicker;
use torrent::{Torrent, piece_priorities};
use tracker::{HTTP_TOKEN_BASE, JobKind, TrackerJob, TrackerManager, TrackerOutcome, UDP4_TOKEN, UDP6_TOKEN};

pub(crate) const WAKER_TOKEN: Token = Token(0);
/// The UDP listen sockets (one per address family) that uTP and the DHT share.
const UDP4_LISTEN_TOKEN: Token = Token(11);
const UDP6_LISTEN_TOKEN: Token = Token(12);
/// Largest UDP datagram we read (jumbo frames included).
const UDP_RECV_BUFFER: usize = 64 * 1024;
const PEER_TOKEN_BASE: usize = 0x1000_0000;

const BANDWIDTH_TICK: Duration = Duration::from_millis(100);
const SAVE_INTERVAL: Duration = Duration::from_secs(30);
/// Adds and removals reach `resume.db` within this delay, so bulk adds rewrite it once.
const STORE_FLUSH_DELAY: Duration = Duration::from_secs(2);
/// libtorrent `piece_timeout`: no block from a peer with requests outstanding this long marks
/// it snubbed.
const PIECE_TIMEOUT: Duration = Duration::from_secs(20);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(90);
const MAX_READS_IN_FLIGHT: usize = 8;

/// A slab with stable indices (freed slots are reused).
pub(crate) struct Slab<T> {
    entries: Vec<Option<T>>,
    free: Vec<usize>,
    len: usize,
}

impl<T> Default for Slab<T> {
    fn default() -> Slab<T> {
        Slab { entries: Vec::new(), free: Vec::new(), len: 0 }
    }
}

impl<T> Slab<T> {
    pub fn insert(&mut self, v: T) -> usize {
        self.len += 1;
        match self.free.pop() {
            Some(i) => {
                self.entries[i] = Some(v);
                i
            }
            None => {
                self.entries.push(Some(v));
                self.entries.len() - 1
            }
        }
    }

    pub fn get(&self, i: usize) -> Option<&T> {
        self.entries.get(i).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        self.entries.get_mut(i).and_then(Option::as_mut)
    }

    pub fn remove(&mut self, i: usize) -> Option<T> {
        let v = self.entries.get_mut(i)?.take()?;
        self.free.push(i);
        self.len -= 1;
        Some(v)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, &T)> {
        self.entries.iter().enumerate().filter_map(|(i, e)| e.as_ref().map(|v| (i, v)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (usize, &mut T)> {
        self.entries.iter_mut().enumerate().filter_map(|(i, e)| e.as_mut().map(|v| (i, v)))
    }
}

struct TorrentSlot {
    generation: u32,
    torrent: Option<Torrent>,
}

/// A UDP listen socket, bound beside a TCP listener on the same port.
pub(super) struct UdpListen {
    pub v4: bool,
    pub sock: UdpSocket,
    pub local: SocketAddr,
    /// How datagrams go while a proxy is set.
    pub proxy: proxy::UdpProxy,
}

/// The uTP manager's way out: the UDP listen socket of the destination's family (libtorrent
/// `session_impl::send_udp_packet`).
struct UdpIo<'a> {
    socks: &'a [UdpListen],
}

impl UtpIo for UdpIo<'_> {
    fn send(&mut self, to: SocketAddr, data: &[u8], dont_fragment: bool) -> SendOutcome {
        match proxy::udp_route(proxy::udp_policy(self.socks), self.socks, proxy::UdpKind::Peer) {
            proxy::UdpRoute::Direct => {}
            proxy::UdpRoute::Relay(i, relay) => {
                let target = crate::proto::proxy::Target::Addr(to);
                return match proxy::send_relayed(&self.socks[i], relay, &target, data, dont_fragment) {
                    Ok(_) => SendOutcome::Sent,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => SendOutcome::WouldBlock,
                    Err(e) if is_message_size(&e) => SendOutcome::MessageSize,
                    Err(e) => SendOutcome::Failed(e.kind()),
                };
            }
            // the associate is still being made: as if the datagram was lost
            proxy::UdpRoute::Drop => return SendOutcome::Sent,
            proxy::UdpRoute::Refuse => return SendOutcome::Failed(io::ErrorKind::PermissionDenied),
        }
        let Some(u) = self.socks.iter().find(|u| u.v4 == to.is_ipv4()) else {
            return SendOutcome::Failed(io::ErrorKind::AddrNotAvailable);
        };
        // the DF bit around MTU probes only, and only for IPv4 (libtorrent `set_dont_frag`)
        let df = dont_fragment && to.is_ipv4();
        if df {
            let _ = sys::set_dont_fragment(&u.sock, true);
        }
        let r = u.sock.send_to(data, to);
        if df {
            let _ = sys::set_dont_fragment(&u.sock, false);
        }
        match r {
            Ok(_) => SendOutcome::Sent,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => SendOutcome::WouldBlock,
            Err(e) if is_message_size(&e) => SendOutcome::MessageSize,
            Err(e) => SendOutcome::Failed(e.kind()),
        }
    }
}

/// `EMSGSIZE` / `WSAEMSGSIZE`: the datagram does not fit the interface (or known path) MTU.
fn is_message_size(e: &io::Error) -> bool {
    #[cfg(windows)]
    const EMSGSIZE: i32 = 10040;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const EMSGSIZE: i32 = 90;
    #[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
    const EMSGSIZE: i32 = 40;
    e.raw_os_error() == Some(EMSGSIZE)
}

/// IPv4-mapped IPv6 senders are IPv4 peers.
fn normalize_addr(a: SocketAddr) -> SocketAddr {
    match a {
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::new(v4.into(), v6.port()),
            None => a,
        },
        SocketAddr::V4(_) => a,
    }
}

pub(crate) struct Engine {
    poll: Poll,
    shared: Arc<Shared>,
    rx: mpsc::Receiver<Command>,
    settings: Settings,
    out: Vec<Alert>,

    slots: Vec<TorrentSlot>,
    free_slots: Vec<u32>,
    by_hash: HashMap<Sha1Hash, TorrentId>,
    /// `SHA-1("req2", info-hash)` of every torrent, which incoming encrypted handshakes name
    /// (libtorrent `torrent_list::find_obfuscated`).
    by_obfuscated: HashMap<Sha1Hash, Sha1Hash>,
    /// Download queue: unfinished torrents in queue order.
    queue: Vec<TorrentId>,
    /// Storage id → torrent, until the storage is released.
    storages: HashMap<u64, TorrentId>,
    /// Storages being deleted for removed torrents.
    deleting: HashMap<u64, (TorrentId, crate::types::InfoHashes)>,

    peers: Slab<PeerConn>,
    next_peer_gen: u32,
    /// The bound listen sockets (libtorrent `m_listen_sockets`).
    listeners: Vec<listen::ListenSocket>,
    /// The UDP side of the listen sockets, at most one per address family.
    udp: Vec<UdpListen>,
    udp_buf: Vec<u8>,
    utp: UtpManager,
    listen_port: u16,
    to_flush: Vec<usize>,

    disk: DiskIo,
    disk_rx: mpsc::Receiver<Completion>,
    resolver: Resolver,
    resolve_rx: mpsc::Receiver<Resolved>,
    trackers: TrackerManager,
    store: Option<Store>,

    peer_id: [u8; 20],
    tracker_key: u32,
    session_paused: bool,
    stat: Stat,
    has_incoming: bool,
    download_bucket: Bucket,
    upload_bucket: Bucket,
    /// The session's external-address votes, one voter per family.
    ip_voter: SessionVoter,
    /// The voted addresses plus the listen addresses (libtorrent `external_address()`).
    external_ip: ExternalIp,
    needs_auto_manage: bool,
    connect_budget: f64,

    last_tick: Instant,
    last_bandwidth: Instant,
    last_unchoke: Instant,
    last_optimistic: Instant,
    last_save: Instant,
    last_auto_manage: Instant,
    store_flush_due: Option<Instant>,
    dht: dht::DhtSession,
    lsd: lsd::LsdSession,
    portmap: portmap::PortmapSession,
    filters: ip_filter::Filters,
    network: listen::NetworkState,
    /// Web seed connections (BEP 19 / BEP 17).
    web: web_seed::WebConns,
    /// The proxy server and its SOCKS5 UDP associate.
    proxy: proxy::ProxyState,
}

/// The network thread's entry point.
pub(crate) fn run(
    poll: Poll,
    waker: Arc<Waker>,
    rx: mpsc::Receiver<Command>,
    shared: Arc<Shared>,
    settings: Settings,
    state_dir: Option<PathBuf>,
) {
    let mut e = Engine::new(poll, waker, rx, shared, settings);
    e.start(state_dir);
    e.flush_alerts();
    let mut events = Events::with_capacity(1024);
    loop {
        let now = Instant::now();
        let tick = Duration::from_millis(e.settings.tick_interval.max(10) as u64);
        let mut next = e.last_tick + tick;
        if e.bandwidth_tick_needed() {
            next = next.min(e.last_bandwidth + BANDWIDTH_TICK);
        }
        if let Some(d) = e.dht.deadline() {
            next = next.min(d);
        }
        if let Some(d) = e.lsd.deadline() {
            next = next.min(d);
        }
        if let Some(d) = e.portmap.deadline() {
            next = next.min(d);
        }
        if let Some(d) = e.network.deadline() {
            next = next.min(d);
        }
        if let Some(d) = e.proxy.deadline() {
            next = next.min(d);
        }
        let timeout = next.saturating_duration_since(now);
        if let Err(err) = e.poll.poll(&mut events, Some(timeout)) {
            if err.kind() != io::ErrorKind::Interrupted {
                e.out.push(Alert::Log { id: None, message: format!("poll failed: {err}") });
            }
        }
        for ev in events.iter() {
            e.on_event(
                ev.token(),
                ev.is_readable() || ev.is_read_closed() || ev.is_error(),
                ev.is_writable(),
            );
        }
        if !e.drain_commands() {
            break;
        }
        e.drain_completions();
        e.flush_peers();
        let now = Instant::now();
        e.dht_service(now);
        e.lsd_service(now);
        e.portmap_service(now);
        e.network_service(now);
        e.proxy_service(now);
        e.flush_tracker_udp();
        if e.bandwidth_tick_needed() && now >= e.last_bandwidth + BANDWIDTH_TICK {
            e.bandwidth_tick(now);
        }
        if now >= e.last_tick + tick {
            e.tick(now);
        }
        e.reap_peers();
        e.flush_peers();
        e.flush_alerts();
    }
    e.shutdown(&mut events);
    e.flush_alerts();
}

impl Engine {
    fn new(
        poll: Poll,
        waker: Arc<Waker>,
        rx: mpsc::Receiver<Command>,
        shared: Arc<Shared>,
        settings: Settings,
    ) -> Engine {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = waker.wake();
        });
        let (disk_tx, disk_rx) = mpsc::channel();
        let disk = DiskIo::new(
            settings.aio_threads as usize,
            settings.file_pool_size as usize,
            disk_tx,
            wake.clone(),
        );
        let (res_tx, resolve_rx) = mpsc::channel();
        let network = listen::NetworkState::new(wake.clone());
        let resolver = Resolver::new(res_tx, wake);
        let mut trackers = TrackerManager::new(poll.registry(), settings.user_agent.clone());
        trackers.set_options(&settings.user_agent, settings.tracker_completion_timeout);
        let now = Instant::now();
        Engine {
            poll,
            shared,
            rx,
            out: Vec::new(),
            slots: Vec::new(),
            free_slots: Vec::new(),
            by_hash: HashMap::new(),
            by_obfuscated: HashMap::new(),
            queue: Vec::new(),
            storages: HashMap::new(),
            deleting: HashMap::new(),
            peers: Slab::default(),
            next_peer_gen: 1,
            listeners: Vec::new(),
            udp: Vec::new(),
            udp_buf: vec![0; UDP_RECV_BUFFER],
            utp: UtpManager::new(UtpSettings::from(&settings), now),
            listen_port: 0,
            to_flush: Vec::new(),
            disk,
            disk_rx,
            resolver,
            resolve_rx,
            trackers,
            store: None,
            peer_id: peer_wire::generate_peer_id(&settings.peer_fingerprint),
            tracker_key: crate::util::random::random_u64() as u32,
            session_paused: false,
            stat: Stat::default(),
            has_incoming: false,
            download_bucket: Bucket::new(settings.download_rate_limit),
            upload_bucket: Bucket::new(settings.upload_rate_limit),
            ip_voter: SessionVoter::new(now),
            external_ip: ExternalIp::default(),
            needs_auto_manage: true,
            connect_budget: 0.0,
            last_tick: now,
            last_bandwidth: now,
            last_unchoke: now,
            last_optimistic: now,
            last_save: now,
            last_auto_manage: now,
            store_flush_due: None,
            dht: dht::DhtSession::new(now),
            lsd: lsd::LsdSession::new(),
            portmap: portmap::PortmapSession::new(),
            filters: ip_filter::Filters::default(),
            network,
            web: web_seed::WebConns::default(),
            proxy: proxy::ProxyState::default(),
            settings,
        }
    }

    // --- Startup and shutdown -----------------------------------------------------------------

    fn start(&mut self, state_dir: Option<PathBuf>) {
        let now = Instant::now();
        self.update_proxy(now);
        self.open_listen_sockets();
        self.start_ip_notifier(now);
        self.update_external_ip();
        self.start_lsd(Instant::now());
        self.start_dht(state_dir.as_deref());
        // after the DHT, whose UDP port the mappers forward too
        self.start_portmap();
        if let Some(dir) = state_dir {
            match Store::open(&dir) {
                Ok((store, records, errors)) => {
                    for e in errors {
                        self.out
                            .push(Alert::Log { id: None, message: format!("skipped a saved torrent: {e}") });
                    }
                    self.store = Some(store);
                    let mut records = records;
                    records.sort_by_key(|r| r.queue_position.unwrap_or(u32::MAX));
                    let n = records.len();
                    for r in records {
                        self.restore(r);
                    }
                    self.out.push(Alert::Log {
                        id: None,
                        message: format!("restored {n} torrents from {}", dir.display()),
                    });
                }
                Err(e) => self
                    .out
                    .push(Alert::Log { id: None, message: format!("cannot open the session store: {e}") }),
            }
        }
        self.auto_manage();
        for id in self.torrent_ids() {
            self.update_running(id);
        }
    }

    fn shutdown(&mut self, events: &mut Events) {
        self.stop_ip_notifier();
        self.save_dht_state();
        let ids: Vec<TorrentId> = self.torrent_ids();
        for id in ids {
            self.stop_torrent(id, true);
        }
        self.reap_peers();
        // A move in flight changes the save path saved below: wait for it (the disk threads
        // finish it anyway).
        while self
            .slots
            .iter()
            .any(|s| s.torrent.as_ref().is_some_and(|t| t.moves_pending > 0 || t.renames_pending > 0))
        {
            match self.disk_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Completion::Moved { storage, status, save_path, error }) => {
                    self.on_storage_moved(storage, status, save_path, error, false)
                }
                Ok(Completion::Renamed { storage, file, name, error }) => {
                    self.on_file_renamed(storage, file, name, error, false)
                }
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        self.save_resume(true);
        // the router forgets our ports (libtorrent `session_impl::abort` closes the mappers)
        self.close_portmap();
        // Give "stopped" announces and UPnP's delete requests a few seconds (libtorrent
        // `stop_tracker_timeout`).
        let deadline = Instant::now() + Duration::from_secs(self.settings.stop_tracker_timeout as u64);
        while (self.trackers.pending() > 0 || self.portmap.pending()) && Instant::now() < deadline {
            let _ = self.poll.poll(events, Some(Duration::from_millis(100)));
            for ev in events.iter() {
                self.on_event(ev.token(), true, ev.is_writable());
            }
            self.portmap_service(Instant::now());
            self.flush_tracker_udp();
            while let Ok(r) = self.resolve_rx.try_recv() {
                if r.cookie & dht::DHT_COOKIE != 0 {
                    continue;
                }
                self.trackers.on_resolved(r, self.poll.registry(), Instant::now());
            }
            self.trackers.tick(self.poll.registry(), Instant::now());
            self.trackers.take_results();
        }
    }

    // --- Helpers ------------------------------------------------------------------------------

    fn torrent_ids(&self) -> Vec<TorrentId> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.torrent.as_ref().map(|_| TorrentId::new(i as u32, s.generation)))
            .collect()
    }

    pub(super) fn torrent(&mut self, id: TorrentId) -> Option<&mut Torrent> {
        let s = self.slots.get_mut(id.index() as usize)?;
        if s.generation != id.generation() {
            return None;
        }
        s.torrent.as_mut()
    }

    fn torrent_ref(&self, id: TorrentId) -> Option<&Torrent> {
        let s = self.slots.get(id.index() as usize)?;
        if s.generation != id.generation() {
            return None;
        }
        s.torrent.as_ref()
    }

    /// Disjoint borrows of a peer, its torrent, the uTP manager and the UDP sockets: what a
    /// read or write on the connection needs.
    fn peer_io(
        &mut self,
        slot: usize,
    ) -> Option<(&mut PeerConn, Option<&mut Torrent>, &mut UtpManager, &[UdpListen])> {
        let Engine { peers, slots, utp, udp, .. } = self;
        let p = peers.get_mut(slot)?;
        let t = p.torrent.and_then(|id| {
            let s = slots.get_mut(id.index() as usize)?;
            if s.generation != id.generation() {
                return None;
            }
            s.torrent.as_mut()
        });
        Some((p, t, utp, udp.as_slice()))
    }

    /// Disjoint borrows of a peer and its torrent.
    pub(super) fn peer_and_torrent(&mut self, slot: usize) -> Option<(&mut PeerConn, Option<&mut Torrent>)> {
        let p = self.peers.get_mut(slot)?;
        let t = p.torrent.and_then(|id| {
            let s = self.slots.get_mut(id.index() as usize)?;
            if s.generation != id.generation() {
                return None;
            }
            s.torrent.as_mut()
        });
        Some((p, t))
    }

    fn alert(&mut self, a: Alert) {
        self.out.push(a);
    }

    fn flush_alerts(&mut self) {
        let mut out = std::mem::take(&mut self.out);
        let mut lost = Vec::new();
        for a in out.drain(..) {
            // A dropped state update must not lose its torrents: `post_torrent_updates` reports
            // only changed torrents, so mark them changed again for the next request.
            let ids: Vec<TorrentId> = match &a {
                Alert::StateUpdate(v) => v.iter().map(|st| st.id).collect(),
                _ => Vec::new(),
            };
            if !self.shared.alerts.post(a) {
                lost.extend(ids);
            }
        }
        self.out = out;
        for id in lost {
            if let Some(t) = self.torrent(id) {
                t.status_dirty = true;
            }
        }
    }

    fn is_running(&self, t: &Torrent) -> bool {
        !t.is_paused() && !self.session_paused && t.error.is_none()
    }

    fn set_state(&mut self, id: TorrentId, state: TorrentState) {
        let Some(t) = self.torrent(id) else { return };
        if t.state != state {
            let prev_state = t.state;
            t.state = state;
            t.status_dirty = true;
            self.alert(Alert::StateChanged { id, state, prev_state });
        }
    }

    // --- Events -------------------------------------------------------------------------------

    fn on_event(&mut self, token: Token, readable: bool, writable: bool) {
        match token.0 {
            0 => {}
            _ if token == listen::IP_NOTIFIER_TOKEN => self.on_ip_notifier_readable(),
            _ if token == proxy::PROXY_UDP_TOKEN => self.on_proxy_udp_event(),
            t if (listen::LISTEN_TOKEN_BASE..PEER_TOKEN_BASE).contains(&t) => self.on_accept(token),
            _ if token == UDP4_TOKEN => self.trackers.on_udp_readable(false, Instant::now()),
            _ if token == UDP6_TOKEN => self.trackers.on_udp_readable(true, Instant::now()),
            _ if token == UDP4_LISTEN_TOKEN || token == UDP6_LISTEN_TOKEN => {
                if readable {
                    self.on_udp_readable(token == UDP4_LISTEN_TOKEN);
                }
                if writable {
                    self.on_udp_writable();
                }
            }
            t if (lsd::LSD_TOKEN_BASE..lsd::LSD_TOKEN_BASE + lsd::MAX_LSD_SOCKETS).contains(&t) => {
                self.on_lsd_readable(t - lsd::LSD_TOKEN_BASE)
            }
            t if t >= web_seed::WEB_TOKEN_BASE => self.on_web_event(token),
            t if t >= portmap::UPNP_HTTP_TOKEN_BASE => self.on_portmap_event(t),
            t if t >= HTTP_TOKEN_BASE => {
                self.trackers.on_http_event(token, &self.resolver, self.poll.registry(), Instant::now())
            }
            t if t >= PEER_TOKEN_BASE => self.on_peer_event(t - PEER_TOKEN_BASE, readable, writable),
            _ => {}
        }
    }

    fn on_accept(&mut self, token: Token) {
        loop {
            let Some(l) = self.listeners.iter().find(|l| l.token == token).and_then(|l| l.tcp.as_ref())
            else {
                return;
            };
            let (sock, addr) = match l.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.alert(Alert::Log { id: None, message: format!("accept failed: {e}") });
                    return;
                }
            };
            let limit = self.settings.connections_limit;
            if limit >= 0 && self.peers.len() >= limit as usize {
                continue; // dropped: closes the socket
            }
            // libtorrent: with a proxy, peers are only accepted through it, never directly
            if !self.settings.enable_incoming_tcp
                || self.session_paused
                || self.proxy.for_peers().is_some()
                || self.incoming_blocked(addr)
            {
                continue;
            }
            self.add_peer_conn(Transport::Tcp(sock), addr, false, PeerSource::INCOMING, None);
        }
    }

    /// Datagrams on the UDP listen socket of one family (libtorrent `session_impl::on_udp_packet`):
    /// uTP gets first dibs, since most packets are uTP; what it declines goes to the DHT.
    fn on_udp_readable(&mut self, v4: bool) {
        let now = Instant::now();
        let Some(index) = self.udp.iter().position(|u| u.v4 == v4) else { return };
        loop {
            let (n, from) = match self.udp[index].sock.recv_from(&mut self.udp_buf) {
                Ok(r) => r,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                // Windows reports ICMP port-unreachable as a receive error (without the address)
                Err(e) if matches!(e.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::Interrupted) => {
                    continue;
                }
                Err(_) => break,
            };
            let mut from = normalize_addr(from);
            let mut buf = &self.udp_buf[..n];
            let mut node_v4 = v4;
            // libtorrent `udp_socket::read`: through a proxy, only the relay's datagrams count
            match proxy::udp_incoming(&self.udp[index].proxy, from, buf) {
                proxy::UdpIn::Direct => {}
                proxy::UdpIn::Drop => continue,
                proxy::UdpIn::Proxied(src, data) => {
                    let addr = match src {
                        crate::proto::proxy::UdpSource::Addr(a) => Some(normalize_addr(a)),
                        crate::proto::proxy::UdpSource::Host(..) => None,
                    };
                    if self.trackers.on_proxied_packet(addr, data, now) {
                        continue;
                    }
                    let Some(addr) = addr else { continue };
                    (from, buf, node_v4) = (addr, data, addr.is_ipv4());
                }
            }
            let n = buf.len();
            let mut io = UdpIo { socks: &self.udp };
            if self.utp.incoming_packet(from, buf, &mut io, now) {
                continue;
            }
            if let Some(t) = self.dht.tracker.as_mut() {
                if n > 20 && buf[0] == b'd' && buf[n - 1] == b'e' {
                    t.incoming_packet(node_v4, from, buf, now);
                }
            }
        }
        let mut io = UdpIo { socks: &self.udp };
        self.utp.socket_drained(&mut io, now);
        self.dht_service(now);
        self.utp_dispatch(now);
    }

    /// A UDP listen socket's send buffer has room again: stalled uTP sockets go on.
    fn on_udp_writable(&mut self) {
        let now = Instant::now();
        let mut io = UdpIo { socks: &self.udp };
        self.utp.writable(&mut io, now);
        self.utp_dispatch(now);
    }

    /// Hands new incoming uTP connections to the peer layer (libtorrent's
    /// `incoming_utp_callback_t` and `incoming_connection`) and wakes the peers whose uTP
    /// sockets have news (its read, write and connect handlers).
    fn utp_dispatch(&mut self, now: Instant) {
        for id in self.utp.take_accepted() {
            let Some(addr) = self.utp.get(id).map(|s| s.remote()) else { continue };
            let limit = self.settings.connections_limit;
            let refuse = (limit >= 0 && self.peers.len() >= limit as usize)
                || !self.settings.enable_incoming_utp
                || self.session_paused
                || self.incoming_blocked(addr);
            if refuse {
                let mut io = UdpIo { socks: &self.udp };
                self.utp.detach(id, &mut io, now);
                continue;
            }
            self.add_peer_conn(Transport::Utp(id), addr, false, PeerSource::INCOMING, None);
        }
        for (id, slot) in self.utp.take_ready() {
            if self.peers.get(slot).is_some_and(|p| matches!(p.sock, Transport::Utp(s) if s == id)) {
                self.on_peer_event(slot, true, true);
            }
        }
    }

    fn add_peer_conn(
        &mut self,
        sock: Transport,
        addr: SocketAddr,
        outgoing: bool,
        source: PeerSource,
        torrent: Option<TorrentId>,
    ) -> Option<usize> {
        if let Transport::Tcp(s) = &sock {
            let _ = s.set_nodelay(true);
        }
        let generation = self.next_peer_gen;
        // The top bit of a connection id stays clear: the disk read cookies of smart_ban use it.
        self.next_peer_gen = (self.next_peer_gen.wrapping_add(1) & 0x7fff_ffff).max(1);
        let now = Instant::now();
        let slot = self.peers.insert(PeerConn::new(0, sock, addr, outgoing, source, now));
        let p = self.peers.get_mut(slot).expect("just inserted");
        p.id = ((generation as u64) << 32) | slot as u64;
        p.torrent = torrent;
        let registered = match &mut p.sock {
            Transport::Tcp(s) => self.poll.registry().register(
                s,
                Token(PEER_TOKEN_BASE + slot),
                Interest::READABLE | Interest::WRITABLE,
            ),
            Transport::Utp(id) => {
                // uTP sockets have no reactor token: the manager's ready list names the slot
                self.utp.set_user(*id, slot);
                Ok(())
            }
        };
        if let Err(e) = registered {
            self.alert(Alert::Log { id: torrent, message: format!("{addr}: register failed: {e}") });
            self.peers.remove(slot);
            return None;
        }
        Some(slot)
    }

    /// Peer slot from a disk read cookie, if that connection still exists.
    fn peer_by_id(&self, id: u64) -> Option<usize> {
        let slot = (id & 0xffff_ffff) as usize;
        self.peers.get(slot).filter(|p| p.id == id).map(|_| slot)
    }

    fn on_peer_event(&mut self, slot: usize, readable: bool, writable: bool) {
        let Some(p) = self.peers.get_mut(slot) else { return };
        if p.state == PeerState::Connecting {
            if !(readable || writable) {
                return;
            }
            match &mut p.sock {
                Transport::Tcp(sock) => match (sock.take_error(), sock.peer_addr()) {
                    (Ok(None), Ok(_)) => {
                        // through a proxy: its handshake first
                        if let Some(t) = p.proxy.as_mut() {
                            match t.drive(sock) {
                                Ok(None) => return,
                                Ok(Some(_)) => p.proxy = None,
                                Err(e) => {
                                    p.close(format!("connect failed: {e}"), true);
                                    return self.proxy_connection_failed(&e);
                                }
                            }
                        }
                    }
                    (Ok(None), Err(e)) if e.kind() == io::ErrorKind::NotConnected => return,
                    (Ok(Some(e)), _) | (Err(e), _) | (_, Err(e)) => {
                        p.close(format!("connect failed: {e}"), true);
                        return;
                    }
                },
                Transport::Utp(id) => match self.utp.get(*id).map(|s| (s.state(), s.error())) {
                    Some((_, Some(e))) => {
                        p.close(format!("connect failed: {e}"), true);
                        return;
                    }
                    Some((utp::State::Connected | utp::State::FinSent, None)) => {}
                    Some(_) => return,
                    None => {
                        p.close("connect failed: uTP socket gone", true);
                        return;
                    }
                },
            }
            p.state = PeerState::Handshake;
            p.connected_at = Instant::now();
            p.last_receive = p.connected_at;
            let (addr, utp) = (p.addr, p.sock.is_utp());
            if utp {
                // libtorrent `on_connection_complete`: this peer is known to speak uTP now
                if let Some((_, Some(t))) = self.peer_and_torrent(slot) {
                    t.peer_list.utp_confirmed(&addr);
                }
            }
            self.start_outgoing_handshake(slot);
        }
        if readable {
            self.do_read(slot);
        }
        if writable {
            self.flush_peer(slot);
        } else if self.peers.get(slot).is_some_and(|p| p.send_buffer_len() > 0) {
            self.to_flush.push(slot);
        }
    }

    /// Writes our plain BitTorrent handshake (libtorrent `write_handshake`); encrypted when the
    /// connection's RC4 stream is on.
    pub(super) fn send_handshake(&mut self, slot: usize) {
        let peer_id = self.peer_id;
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        let ih = t.info_hashes.best_v1_compatible();
        peer_wire::encode_handshake(p.out(), &ih, &peer_id);
        p.sent_handshake = true;
        self.to_flush.push(slot);
    }

    fn disk_backlogged(&self) -> bool {
        self.disk.queued_write_bytes() > self.settings.max_queued_disk_bytes.max(4 * 1024 * 1024) as u64
    }

    fn do_read(&mut self, slot: usize) {
        self.do_read_capped(slot, usize::MAX);
    }

    /// Reads at most `share` bytes: the peer's share when several wait for quota.
    fn do_read_capped(&mut self, slot: usize, mut share: usize) {
        loop {
            let backlogged = self.disk_backlogged();
            let session_quota = self.download_bucket.available();
            let Some((p, t, utp, _)) = self.peer_io(slot) else { return };
            if p.closing.is_some() || p.state == PeerState::Connecting {
                return;
            }
            let quota = session_quota.min(t.map_or(usize::MAX, |t| t.download_limit.available())).min(share);
            let quota = if backlogged && p.state == PeerState::Active { 0 } else { quota };
            if quota == 0 {
                p.read_blocked = true;
                return;
            }
            p.read_blocked = false;
            let max = quota.min(256 * 1024);
            let (n, more) = match p.read_some(max, utp) {
                peer::ReadOutcome::Data(n) => (n, true),
                peer::ReadOutcome::WouldBlock(n) => (n, false),
                peer::ReadOutcome::Closed => {
                    p.close("connection closed by peer", false);
                    (0, false)
                }
                peer::ReadOutcome::Error(e) => {
                    p.close(format!("read error: {e}"), true);
                    (0, false)
                }
            };
            share = share.saturating_sub(n);
            if n > 0 {
                p.last_receive = Instant::now();
                self.download_bucket.consume(n);
                if let Some((_, Some(t))) = self.peer_and_torrent(slot) {
                    t.download_limit.consume(n);
                }
                let payload = self.process_messages(slot);
                let protocol = (n as u64).saturating_sub(payload);
                self.stat.received(0, protocol);
                if let Some((p, t)) = self.peer_and_torrent(slot) {
                    p.stat.received(0, protocol);
                    if let Some(t) = t {
                        t.stat.received(0, protocol);
                    }
                }
            }
            if !more || n == 0 {
                return;
            }
        }
    }

    pub(super) fn flush_peer(&mut self, slot: usize) {
        self.flush_peer_capped(slot, usize::MAX);
    }

    /// Writes at most `share` bytes: the peer's share when several wait for quota.
    fn flush_peer_capped(&mut self, slot: usize, share: usize) {
        let session_quota = self.upload_bucket.available();
        let now = Instant::now();
        let Some((p, t, utp, udp)) = self.peer_io(slot) else { return };
        if p.closing.is_some() || p.state == PeerState::Connecting || p.send_buffer_len() == 0 {
            return;
        }
        let quota =
            session_quota.min(t.as_ref().map_or(usize::MAX, |t| t.upload_limit.available())).min(share);
        if quota == 0 {
            p.write_blocked = true;
            return;
        }
        let mut io = UdpIo { socks: udp };
        match p.write_some(quota, utp, &mut io, now) {
            Ok((raw, payload, _)) => {
                if raw > 0 {
                    p.last_sent = Instant::now();
                }
                let protocol = raw.saturating_sub(payload) as u64;
                p.stat.sent(payload as u64, protocol);
                if let Some(t) = t {
                    t.upload_limit.consume(raw);
                    t.stat.sent(payload as u64, protocol);
                }
                self.upload_bucket.consume(raw);
                self.stat.sent(payload as u64, protocol);
            }
            Err(e) => p.close(format!("write error: {e}"), false),
        }
        self.pump_uploads(slot);
    }

    fn flush_peers(&mut self) {
        let mut list = std::mem::take(&mut self.to_flush);
        list.sort_unstable();
        list.dedup();
        for slot in list.drain(..) {
            self.flush_peer(slot);
        }
        if self.to_flush.is_empty() {
            self.to_flush = list;
        }
    }

    fn bandwidth_tick_needed(&self) -> bool {
        self.download_bucket.limit > 0
            || self.upload_bucket.limit > 0
            || self.peers.iter().any(|(_, p)| p.read_blocked || (p.write_blocked && p.send_buffer_len() > 0))
            || self.web.any_read_blocked()
    }

    fn bandwidth_tick(&mut self, now: Instant) {
        let dt = now.duration_since(self.last_bandwidth).as_secs_f64().min(1.0);
        self.last_bandwidth = now;
        self.download_bucket.refill(dt);
        self.upload_bucket.refill(dt);
        for s in &mut self.slots {
            if let Some(t) = s.torrent.as_mut() {
                t.download_limit.refill(dt);
                t.upload_limit.refill(dt);
            }
        }
        let writes: Vec<usize> =
            self.peers.iter().filter(|(_, p)| p.send_buffer_len() > 0).map(|(i, _)| i).collect();
        for (slot, share) in self.quota_shares(&writes, false) {
            self.flush_peer_capped(slot, share);
        }
        let reads: Vec<usize> = self.peers.iter().filter(|(_, p)| p.read_blocked).map(|(i, _)| i).collect();
        for (slot, share) in self.quota_shares(&reads, true) {
            self.do_read_capped(slot, share);
        }
        self.web_unblock_reads();
    }

    /// libtorrent `bandwidth_manager::update_quotas` / `bw_request::assign_bandwidth`: the
    /// quota of each channel (the session's and each torrent's) is split evenly over the peers
    /// waiting for it (every peer has priority 1). Serving them in slot order with all the quota
    /// let the first, busy peers take everything, and a new peer never read its handshake.
    fn quota_shares(&self, slots: &[usize], download: bool) -> Vec<(usize, usize)> {
        fn share(available: usize, waiting: usize) -> usize {
            if available == usize::MAX { usize::MAX } else { available / waiting.max(1) }
        }
        let mut per_torrent: HashMap<TorrentId, usize> = HashMap::new();
        for id in slots.iter().filter_map(|&s| self.peers.get(s).and_then(|p| p.torrent)) {
            *per_torrent.entry(id).or_default() += 1;
        }
        let session = if download { &self.download_bucket } else { &self.upload_bucket };
        let session_share = share(session.available(), slots.len());
        slots
            .iter()
            .map(|&slot| {
                let id = self.peers.get(slot).and_then(|p| p.torrent);
                let torrent_share =
                    id.and_then(|id| self.torrent_ref(id).map(|t| (id, t))).map_or(usize::MAX, |(id, t)| {
                        let bucket = if download { &t.download_limit } else { &t.upload_limit };
                        share(bucket.available(), per_torrent[&id])
                    });
                (slot, session_share.min(torrent_share))
            })
            .collect()
    }

    /// Removes peers marked for closing.
    fn reap_peers(&mut self) {
        let closing: Vec<usize> =
            self.peers.iter().filter(|(_, p)| p.closing.is_some()).map(|(i, _)| i).collect();
        for slot in closing {
            self.remove_peer(slot);
        }
    }

    fn remove_peer(&mut self, slot: usize) {
        let Some(mut p) = self.peers.remove(slot) else { return };
        let now = Instant::now();
        match &mut p.sock {
            Transport::Tcp(sock) => {
                let _ = self.poll.registry().deregister(sock);
            }
            // the uTP socket sends its FIN and lives on in the manager until it is acknowledged
            Transport::Utp(id) => {
                let mut io = UdpIo { socks: &self.udp };
                self.utp.detach(*id, &mut io, now);
            }
        }
        let (reason, failed) = p.closing.take().unwrap_or_default();
        let Some(id) = p.torrent else { return };
        let Some(t) = self.torrent(id) else { return };
        t.peers.retain(|&s| s != slot);
        if p.state == PeerState::Active {
            if let Some(picker) = t.picker.as_mut() {
                if p.peer_seed {
                    picker.dec_seed();
                } else if !p.peer_has.is_empty() {
                    picker.dec_refcount_bitfield(&p.peer_has);
                }
                for r in &p.requests {
                    picker.abort_download(r.piece, r.block);
                }
            }
        }
        let seed = p.is_seed();
        t.status_dirty = true;
        // libtorrent `on_connection_failed`: a uTP connect that never got through means the peer
        // probably doesn't speak uTP (or its UDP port is closed): don't assume it again, and
        // reconnect over TCP at once.
        let utp_retry = p.sock.is_utp()
            && p.outgoing
            && p.state == PeerState::Connecting
            && failed
            && t.peer_list.utp_failed(&p.addr);
        if utp_retry {
            t.peer_list.disconnected(&p.addr, false, seed, true);
            self.connect_to_peer(id, p.addr, now);
        } else {
            t.peer_list.disconnected(&p.addr, failed && p.state != PeerState::Active, seed, p.fast_reconnect);
        }
        let _ = reason;
    }

    // --- Commands -----------------------------------------------------------------------------

    /// Handles queued commands. Returns false on shutdown.
    fn drain_commands(&mut self) -> bool {
        let mut any = false;
        while let Ok(cmd) = self.rx.try_recv() {
            any = true;
            if matches!(cmd, Command::Shutdown) {
                return false;
            }
            self.handle(cmd);
        }
        if any && self.needs_auto_manage {
            self.auto_manage();
        }
        true
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::AddTorrent(p) => self.add_torrent(*p),
            Command::RemoveTorrent(id, flags) => self.remove_torrent(id, flags),
            Command::Pause(id) => self.set_flags(id, TorrentFlags::PAUSED, TorrentFlags::PAUSED),
            Command::Resume(id) => self.set_flags(id, TorrentFlags::empty(), TorrentFlags::PAUSED),
            Command::SetFlags { id, flags, mask } => self.set_flags(id, flags, mask),
            Command::Queue(id, m) => self.move_in_queue(id, m),
            Command::ForceRecheck(id) => self.force_recheck(id),
            Command::ForceReannounce(id) => {
                if let Some(t) = self.torrent(id) {
                    for tr in &mut t.trackers {
                        tr.next_announce = None;
                        tr.fails = 0;
                    }
                }
            }
            Command::SetFilePriority { id, file, priority } => {
                if let Some(t) = self.torrent(id) {
                    let mut prios = t.file_priority.clone();
                    if (file as usize) < prios.len() {
                        prios[file as usize] = priority.0.min(7);
                        self.set_file_priorities(id, prios);
                    }
                }
            }
            Command::PrioritizeFiles(id, prios) => {
                if let Some(t) = self.torrent(id) {
                    let mut v = t.file_priority.clone();
                    for (i, p) in prios.iter().enumerate().take(v.len()) {
                        v[i] = p.0.min(7);
                    }
                    self.set_file_priorities(id, v);
                }
            }
            Command::SetDownloadLimit(id, l) => {
                if let Some(t) = self.torrent(id) {
                    t.download_limit.set_limit(l);
                    t.resume_dirty = true;
                    t.status_dirty = true;
                }
            }
            Command::SetUploadLimit(id, l) => {
                if let Some(t) = self.torrent(id) {
                    t.upload_limit.set_limit(l);
                    t.resume_dirty = true;
                    t.status_dirty = true;
                }
            }
            Command::SetMaxConnections(id, l) => self.set_max_connections(id, l),
            Command::SetMaxUploads(id, l) => {
                if let Some(t) = self.torrent(id) {
                    // libtorrent `set_max_uploads`: 0 (or less) is no limit
                    let limit = (l > 0).then_some(l);
                    if t.max_uploads != limit {
                        t.max_uploads = limit;
                        t.resume_dirty = true;
                        t.status_dirty = true;
                    }
                }
            }
            Command::ReplaceTrackers(id, list) => {
                let list: Vec<(String, u32)> = list.into_iter().map(|e| (e.url, e.tier)).collect();
                if let Some(t) = self.torrent(id) {
                    t.replace_trackers(&list);
                    t.resume_dirty = true;
                    t.status_dirty = true;
                }
            }
            Command::AddTracker(id, e) => {
                if let Some(t) = self.torrent(id) {
                    let before = t.trackers.len();
                    t.add_tracker(&e.url, e.tier);
                    if t.trackers.len() != before {
                        t.resume_dirty = true;
                        t.status_dirty = true;
                    }
                }
            }
            Command::MoveStorage(id, path, flags) => self.move_storage(id, path, flags),
            Command::RenameFile(id, file, name) => self.rename_file(id, file, name),
            Command::ConnectPeer(id, addr) => {
                if self.add_peer(id, addr, PeerSource::RESUME_DATA) {
                    self.connect_peers_for(id, usize::MAX);
                }
            }
            Command::AddWebSeed(id, url, kind) => self.add_web_seed_cmd(id, url, kind),
            Command::RemoveWebSeed(id, url, kind) => self.remove_web_seed_cmd(id, &url, kind),
            Command::PostWebSeeds(id) => self.post_web_seeds(id),
            Command::PostTorrentUpdates => self.post_torrent_updates(),
            Command::PostSessionStats => self.post_session_stats(),
            Command::SaveResumeData => {
                self.save_resume(true);
                self.alert(Alert::ResumeDataSaved);
            }
            Command::PostPeerInfo(id) => self.post_peer_info(id),
            Command::PostFileList(id) => {
                self.ensure_metadata(id);
                if let Some(t) = self.torrent_ref(id) {
                    let files = t.file_entries().into();
                    self.alert(Alert::FileList { id, files });
                }
            }
            Command::PostFileProgress(id) => {
                self.ensure_metadata(id);
                if let Some(t) = self.torrent_ref(id) {
                    let done = t.file_progress();
                    self.alert(Alert::FileProgress { id, done });
                }
            }
            Command::PostTrackers(id) => {
                let now = Instant::now();
                if let Some(t) = self.torrent_ref(id) {
                    let trackers = t.trackers.iter().map(|tr| tr.to_status(now)).collect();
                    self.alert(Alert::TrackerList { id, trackers });
                }
            }
            Command::PostPieceInfo(id) => {
                if let Some(t) = self.torrent_ref(id) {
                    let have =
                        t.have_bits().cloned().unwrap_or_else(|| Bitfield::new(t.num_pieces as usize, false));
                    let (availability, downloading) = match &t.picker {
                        Some(p) => (
                            (0..t.num_pieces).map(|i| p.availability(i)).collect(),
                            p.partial_pieces(|i| t.piece_size(i) as u32),
                        ),
                        None => (vec![0; t.num_pieces as usize], Vec::new()),
                    };
                    self.alert(Alert::PieceInfo { id, have, availability, downloading });
                }
            }
            Command::ApplySettings(s) => self.apply_settings(*s),
            Command::PauseSession => {
                if !self.session_paused {
                    self.session_paused = true;
                    for id in self.torrent_ids() {
                        self.update_running(id);
                    }
                }
            }
            Command::ResumeSession => {
                if self.session_paused {
                    self.session_paused = false;
                    for id in self.torrent_ids() {
                        self.update_running(id);
                    }
                }
            }
            Command::SetIpFilter(f) => self.set_ip_filter(f),
            Command::SetPortFilter(f) => self.set_port_filter(f),
            Command::ReopenNetworkSockets => self.on_ip_change(Instant::now(), true),
            #[cfg(test)]
            Command::SetFakeNetwork(net) => {
                self.network.fake = net;
                self.on_ip_change(Instant::now(), false);
            }
            Command::Shutdown => {}
        }
    }

    fn apply_settings(&mut self, s: Settings) {
        self.download_bucket.set_limit(s.download_rate_limit);
        self.upload_bucket.set_limit(s.upload_rate_limit);
        self.utp.set_settings(UtpSettings::from(&s));
        self.disk.set_file_pool_size(s.file_pool_size as usize);
        self.trackers.set_options(&s.user_agent, s.tracker_completion_timeout);
        if let Some(t) = self.dht.tracker.as_mut() {
            t.set_settings(crate::dht::DhtSettings::from(&s));
        }
        let queue_changed = s.active_downloads != self.settings.active_downloads
            || s.active_seeds != self.settings.active_seeds
            || s.active_limit != self.settings.active_limit;
        let lsd_changed = s.enable_lsd != self.settings.enable_lsd;
        let natpmp_changed = s.enable_natpmp != self.settings.enable_natpmp;
        let upnp_changed = s.enable_upnp != self.settings.enable_upnp;
        let listen_changed = listen::listen_settings_changed(&self.settings, &s);
        self.settings = s;
        self.update_proxy(Instant::now());
        if let Some(close_all) = listen_changed {
            self.on_listen_settings_changed(close_all);
        }
        if queue_changed {
            self.needs_auto_manage = true;
        }
        // libtorrent `update_lsd`
        if lsd_changed {
            if self.settings.enable_lsd {
                self.start_lsd(Instant::now());
            } else {
                self.stop_lsd();
            }
        }
        // libtorrent `update_natpmp` / `update_upnp`
        if natpmp_changed && !self.settings.enable_natpmp {
            self.stop_natpmp();
        }
        if upnp_changed && !self.settings.enable_upnp {
            self.stop_upnp();
        }
        if (natpmp_changed && self.settings.enable_natpmp) || (upnp_changed && self.settings.enable_upnp) {
            self.start_portmap();
        }
        self.update_upnp_settings();
    }

    // --- Adding and removing ------------------------------------------------------------------

    fn insert_torrent(&mut self, mut t: Torrent) -> TorrentId {
        let index = match self.free_slots.pop() {
            Some(i) => i,
            None => {
                self.slots.push(TorrentSlot { generation: 0, torrent: None });
                (self.slots.len() - 1) as u32
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.generation += 1;
        let id = TorrentId::new(index, slot.generation);
        t.id = id;
        let ih = t.info_hashes.best_v1_compatible();
        self.by_hash.insert(ih, id);
        self.by_obfuscated.insert(mse::stream_key_hash(&ih), ih);
        if !t.is_finished() {
            t.queue_position = Some(self.queue.len() as u32);
            self.queue.push(id);
        }
        slot.torrent = Some(t);
        self.needs_auto_manage = true;
        id
    }

    fn add_torrent(&mut self, p: AddTorrentParams) {
        let fallback_name = p.name.clone().unwrap_or_default();
        let fail =
            |e: &mut Engine, name: String, error: Error| e.alert(Alert::AddTorrentFailed { name, error });
        let mut resume: Option<ResumeData> = None;
        let (ti, mut trackers) = match &p.source {
            TorrentSource::TorrentFile(bytes) => match TorrentInfo::from_bytes(bytes) {
                Ok(ti) => (ti, Vec::new()),
                Err(e) => return fail(self, fallback_name, e.into()),
            },
            TorrentSource::Resume(bytes) => {
                let r = match ResumeData::decode(bytes) {
                    Ok(r) => r,
                    Err(e) => return fail(self, fallback_name, e),
                };
                let Some(info) = r.info.clone() else {
                    return fail(self, r.name, Error::Unsupported("resume data without the info dictionary"));
                };
                let ti = match TorrentInfo::from_info_section(&info, LoadLimits::default()) {
                    Ok(ti) => ti,
                    Err(e) => return fail(self, r.name, e.into()),
                };
                let tr: Vec<(String, u32)> = r
                    .trackers
                    .iter()
                    .enumerate()
                    .flat_map(|(tier, urls)| urls.iter().map(move |u| (u.clone(), tier as u32)))
                    .collect();
                resume = Some(r);
                (ti, tr)
            }
            TorrentSource::Magnet(_) | TorrentSource::InfoHash(_) => return self.add_magnet(p),
        };
        if ti.v2() && !ti.v1() {
            return fail(
                self,
                ti.name().to_owned(),
                Error::Unsupported("v2-only torrents (PLAN.md Phase 8)"),
            );
        }
        let info_hashes = ti.info_hashes();
        if let Some(&existing) = self.by_hash.get(&info_hashes.best_v1_compatible()) {
            if p.flags.contains(TorrentFlags::DUPLICATE_IS_ERROR) {
                return fail(self, ti.name().to_owned(), Error::DuplicateTorrent);
            }
            let name = self.torrent_ref(existing).map(|t| t.name.clone()).unwrap_or_default();
            self.alert(Alert::TorrentAdded { id: existing, info_hashes, name });
            return;
        }
        for t in ti.trackers() {
            trackers.push((t.url.clone(), t.tier as u32));
        }
        for (i, url) in p.trackers.iter().enumerate() {
            trackers.push((url.clone(), p.tracker_tiers.get(i).copied().unwrap_or(0)));
        }

        let save_path = resume
            .as_ref()
            .map(|r| r.save_path.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or(p.save_path.clone());
        let mut flags = p.flags;
        flags.remove(TorrentFlags::DUPLICATE_IS_ERROR);
        let mut t = Torrent::new(
            TorrentId::default(),
            info_hashes,
            p.name.clone().unwrap_or_default(),
            save_path,
            flags,
        );
        let info_bytes = ti.info_section().to_vec();
        let ti = Arc::new(ti);
        t.renamed_files = p.renamed_files.clone();
        t.set_metadata(ti.clone());
        if flags.contains(TorrentFlags::DEFAULT_DONT_DOWNLOAD) {
            t.file_priority.fill(0);
        }
        for (i, prio) in p.file_priorities.iter().enumerate().take(t.file_priority.len()) {
            t.file_priority[i] = prio.0.min(7);
        }
        for (url, tier) in trackers {
            t.add_tracker(&url, tier);
        }
        for ws in ti.web_seeds() {
            t.web_seeds.add(&ws.url, ws.kind, &ws.auth, &ws.extra_headers, false, false);
        }
        if let Some(r) = &resume {
            t.web_seeds.add_urls(&r.url_seeds, WebSeedKind::UrlSeed);
            t.web_seeds.add_urls(&r.http_seeds, WebSeedKind::HttpSeed);
        }
        t.web_seeds.add_urls(&p.url_seeds, WebSeedKind::UrlSeed);
        t.web_seeds.add_urls(&p.http_seeds, WebSeedKind::HttpSeed);
        t.download_limit.set_limit(p.download_limit);
        t.upload_limit.set_limit(p.upload_limit);
        t.max_connections = p.max_connections;
        t.max_uploads = p.max_uploads;
        if let Some(r) = resume {
            apply_resume(&mut t, &r);
        }
        if flags.contains(TorrentFlags::SEED_MODE) {
            t.have = Some(Bitfield::new(t.num_pieces as usize, true));
        }
        let name = t.name.clone();
        let key = t.key.clone();
        let id = self.insert_torrent(t);
        if let Some(store) = self.store.as_mut() {
            let rec = self.slots[id.index() as usize].torrent.as_ref().expect("inserted").resume_data();
            if let Err(e) = store.put(&key, &rec, Some(&info_bytes)) {
                self.out.push(Alert::Log { id: Some(id), message: format!("cannot save the torrent: {e}") });
            }
            self.store_flush_due.get_or_insert(Instant::now() + STORE_FLUSH_DELAY);
        }
        if let Some(t) = self.torrent(id) {
            t.resume_dirty = false;
        }
        self.alert(Alert::TorrentAdded { id, info_hashes, name });
        self.update_running(id);
    }

    /// A magnet link or a bare info-hash: the torrent starts without metadata and fetches it
    /// from its peers (BEP 9) when it runs.
    fn add_magnet(&mut self, p: AddTorrentParams) {
        let fallback_name = p.name.clone().unwrap_or_default();
        let m = match &p.source {
            TorrentSource::Magnet(uri) => match parse_magnet_uri(uri) {
                Ok(m) => m,
                Err(e) => {
                    return self.alert(Alert::AddTorrentFailed { name: fallback_name, error: e.into() });
                }
            },
            TorrentSource::InfoHash(ih) => MagnetUri { info_hashes: *ih, ..MagnetUri::default() },
            _ => unreachable!("only magnets and info-hashes come here"),
        };
        let info_hashes = m.info_hashes;
        let user_named = p.name.as_deref().is_some_and(|n| !n.is_empty());
        let name = if user_named {
            fallback_name
        } else if !m.name.is_empty() {
            m.name.clone()
        } else {
            info_hashes.key_hex()
        };
        if !info_hashes.has_v1() {
            let error = Error::Unsupported("v2-only torrents (PLAN.md Phase 8)");
            return self.alert(Alert::AddTorrentFailed { name, error });
        }
        if let Some(&existing) = self.by_hash.get(&info_hashes.best_v1_compatible()) {
            if p.flags.contains(TorrentFlags::DUPLICATE_IS_ERROR) {
                return self.alert(Alert::AddTorrentFailed { name, error: Error::DuplicateTorrent });
            }
            let name = self.torrent_ref(existing).map(|t| t.name.clone()).unwrap_or_default();
            self.alert(Alert::TorrentAdded { id: existing, info_hashes, name });
            return;
        }
        let mut flags = p.flags;
        flags.remove(TorrentFlags::DUPLICATE_IS_ERROR);
        if m.default_dont_download {
            flags.insert(TorrentFlags::DEFAULT_DONT_DOWNLOAD);
        }
        let mut t = Torrent::new(TorrentId::default(), info_hashes, name, p.save_path.clone(), flags);
        t.user_named = user_named;
        t.renamed_files = p.renamed_files.clone();
        for (i, url) in m.trackers.iter().enumerate() {
            t.add_tracker(url, m.tracker_tiers.get(i).copied().unwrap_or(0));
        }
        for (i, url) in p.trackers.iter().enumerate() {
            t.add_tracker(url, p.tracker_tiers.get(i).copied().unwrap_or(0));
        }
        // BEP 19 `ws=`: used once the metadata is in
        t.web_seeds.add_urls(&m.url_seeds, WebSeedKind::UrlSeed);
        t.web_seeds.add_urls(&p.url_seeds, WebSeedKind::UrlSeed);
        t.web_seeds.add_urls(&p.http_seeds, WebSeedKind::HttpSeed);
        // File priorities apply once the file list is known (the caller's win over `so=`).
        let n = m.file_priorities.len().max(p.file_priorities.len());
        t.file_priority = (0..n)
            .map(|i| p.file_priorities.get(i).or(m.file_priorities.get(i)).map_or(4, |d| d.0.min(7)))
            .collect();
        for addr in &m.peers {
            t.peer_list.add(*addr, PeerSource::RESUME_DATA, ExternalIp::default(), usize::MAX);
        }
        for (host, port) in &m.dht_nodes {
            self.add_dht_node(host, *port);
        }
        t.download_limit.set_limit(p.download_limit);
        t.upload_limit.set_limit(p.upload_limit);
        t.max_connections = p.max_connections;
        t.max_uploads = p.max_uploads;
        t.state = TorrentState::DownloadingMetadata;
        let name = t.name.clone();
        let key = t.key.clone();
        let rec = t.resume_data();
        let id = self.insert_torrent(t);
        if let Some(store) = self.store.as_mut() {
            if let Err(e) = store.put(&key, &rec, None) {
                self.out.push(Alert::Log { id: Some(id), message: format!("cannot save the torrent: {e}") });
            }
            self.store_flush_due.get_or_insert(Instant::now() + STORE_FLUSH_DELAY);
        }
        if let Some(t) = self.torrent(id) {
            t.resume_dirty = false;
        }
        self.alert(Alert::TorrentAdded { id, info_hashes, name });
        self.update_running(id);
    }

    /// Restores a torrent saved in the store (metadata stays in the pack until it starts).
    fn restore(&mut self, r: ResumeData) {
        let mut t = Torrent::new(
            TorrentId::default(),
            r.info_hashes,
            r.name.clone(),
            r.save_path.clone(),
            TorrentFlags::empty(),
        );
        t.total_size = r.total_size;
        t.num_pieces = r.num_pieces;
        t.piece_length = r.piece_length;
        for (tier, urls) in r.trackers.iter().enumerate() {
            for u in urls {
                t.add_tracker(u, tier as u32);
            }
        }
        t.web_seeds.add_urls(&r.url_seeds, WebSeedKind::UrlSeed);
        t.web_seeds.add_urls(&r.http_seeds, WebSeedKind::HttpSeed);
        apply_resume(&mut t, &r);
        let has_meta = self.store.as_ref().is_some_and(|s| s.has_metadata(&t.key));
        if !has_meta && t.num_pieces == 0 {
            // A magnet still waiting for its metadata.
        } else if !has_meta || t.num_pieces == 0 || t.piece_length == 0 {
            // Records from before metadata was stored, or a damaged pack: load it now.
            let key = t.key.clone();
            match self.load_info(&key) {
                Some(ti) => t.set_metadata(ti),
                None => t.error = Some("metadata missing from the session store".into()),
            }
        }
        t.state = t.settled_state();
        t.resume_dirty = false;
        let id = self.insert_torrent(t);
        let name = self.torrent_ref(id).map(|t| t.name.clone()).unwrap_or_default();
        self.alert(Alert::TorrentAdded { id, info_hashes: r.info_hashes, name });
    }

    fn load_info(&mut self, key: &str) -> Option<Arc<TorrentInfo>> {
        let bytes = self.store.as_mut()?.load_metadata(key).ok()??;
        TorrentInfo::from_info_section(&bytes, LoadLimits::default()).ok().map(Arc::new)
    }

    /// Loads metadata from the store if the torrent does not hold it.
    fn ensure_metadata(&mut self, id: TorrentId) -> bool {
        let Some(t) = self.torrent_ref(id) else { return false };
        if t.ti.is_some() {
            return true;
        }
        let key = t.key.clone();
        match self.load_info(&key) {
            Some(ti) => {
                self.torrent(id).expect("exists").set_metadata(ti);
                true
            }
            None => false,
        }
    }

    fn remove_torrent(&mut self, id: TorrentId, flags: RemoveFlags) {
        if self.torrent_ref(id).is_none() {
            return;
        }
        // libtorrent `delete_files` / `delete_partfile`; deleting the files takes the part file.
        let delete = if flags.contains(RemoveFlags::DELETE_FILES) {
            Some(FenceAction::Delete)
        } else if flags.contains(RemoveFlags::DELETE_PARTFILE) {
            Some(FenceAction::DeletePartFile)
        } else {
            None
        };
        if delete.is_some() {
            self.ensure_metadata(id);
        }
        // A storage still moving files: deleting (or releasing) waits behind the move on it.
        let moving = self.torrent(id).and_then(|t| t.moving.take());
        self.stop_torrent(id, true);
        self.reap_peers();
        let slot = &mut self.slots[id.index() as usize];
        let t = slot.torrent.take().expect("checked");
        self.free_slots.push(id.index());
        let ih = t.info_hashes.best_v1_compatible();
        self.by_hash.remove(&ih);
        self.by_obfuscated.remove(&mse::stream_key_hash(&ih));
        if let Some(pos) = self.queue.iter().position(|&q| q == id) {
            self.queue.remove(pos);
            self.renumber_queue(pos);
        }
        if let Some(store) = self.store.as_mut() {
            store.remove(&t.key);
            self.store_flush_due.get_or_insert(Instant::now() + STORE_FLUSH_DELAY);
        }
        self.alert(Alert::TorrentRemoved { id, info_hashes: t.info_hashes });
        if let Some((storage, temporary)) = &moving {
            match delete {
                Some(action) => {
                    self.deleting.insert(storage.id, (id, t.info_hashes));
                    self.disk.async_fence(storage, action);
                }
                None if *temporary => self.disk.async_fence(storage, FenceAction::Release),
                None => {}
            }
        } else if let Some(action) = delete {
            match t.ti.clone() {
                Some(ti) => {
                    let storage = self.disk.new_storage(t.save_path.clone(), ti, Vec::new());
                    self.deleting.insert(storage.id, (id, t.info_hashes));
                    self.disk.async_fence(&storage, action);
                }
                // A magnet without metadata never wrote anything.
                None if !t.has_metadata() => {
                    self.alert(Alert::TorrentDeleted { id, info_hashes: t.info_hashes })
                }
                None => self.alert(Alert::TorrentDeleteFailed {
                    id,
                    info_hashes: t.info_hashes,
                    error: Error::Unsupported("deleting files without metadata"),
                }),
            }
        }
        self.needs_auto_manage = true;
    }

    // --- Moving storage -----------------------------------------------------------------------

    /// libtorrent `torrent::move_storage`. A running torrent's storage moves its files behind
    /// the jobs already queued, holding later ones back. A stopped torrent gets a storage just
    /// for the move (libtorrent keeps its storage while paused, so the files move as well).
    fn move_storage(&mut self, id: TorrentId, path: String, flags: MoveFlags) {
        let Some(t) = self.torrent(id) else { return };
        let storage = match (&t.storage, &t.moving) {
            (Some(s), _) => Some((s.clone(), false)),
            (None, Some(m)) => Some(m.clone()),
            (None, None) => None,
        };
        if let Some((storage, temporary)) = storage {
            t.moving = Some((storage.clone(), temporary));
            t.moves_pending += 1;
            t.status_dirty = true;
            self.disk.async_move_storage(&storage, path, flags);
            return;
        }
        // Without metadata nothing is on disk yet: just use the new path.
        if !t.has_metadata() {
            let new = crate::disk::complete(&path);
            let old = std::mem::replace(&mut t.save_path, new.clone());
            t.resume_dirty = true;
            t.status_dirty = true;
            self.alert(Alert::StorageMoved { id, old, new });
            return;
        }
        if !self.ensure_metadata(id) {
            let error = Error::Unsupported("moving files without metadata");
            self.alert(Alert::StorageMovedFailed {
                id,
                error,
                file: String::new(),
                operation: FileOperation::Unknown,
            });
            return;
        }
        let t = self.torrent(id).expect("exists");
        let (save_path, ti, prio) =
            (t.save_path.clone(), t.ti.clone().expect("loaded"), t.file_priority.clone());
        let storage = self.disk.new_storage(save_path, ti, prio);
        self.storages.insert(storage.id, id);
        let t = self.torrent(id).expect("exists");
        t.moving = Some((storage.clone(), true));
        t.moves_pending += 1;
        t.status_dirty = true;
        self.disk.async_move_storage(&storage, path, flags);
    }

    /// libtorrent `torrent::on_storage_moved`. `resume` starts a torrent whose start waited
    /// for the move (not while shutting down).
    fn on_storage_moved(
        &mut self,
        storage: u64,
        status: MoveStatus,
        save_path: String,
        error: Option<MoveError>,
        resume: bool,
    ) {
        let Some(&id) = self.storages.get(&storage) else { return };
        let keep_metadata = self.store.is_none();
        let Some(t) = self.torrent(id) else { return };
        t.moves_pending = t.moves_pending.saturating_sub(1);
        t.status_dirty = true;
        match status {
            MoveStatus::NoError | MoveStatus::NeedFullCheck => {
                let old = std::mem::replace(&mut t.save_path, save_path.clone());
                t.resume_dirty = true;
                self.alert(Alert::StorageMoved { id, old, new: save_path });
            }
            MoveStatus::FileExist | MoveStatus::FatalDiskError => {
                let e = error.unwrap_or(MoveError {
                    file: String::new(),
                    operation: FileOperation::Unknown,
                    error: Error::Unsupported("move failed"),
                });
                self.alert(Alert::StorageMovedFailed {
                    id,
                    error: e.error,
                    file: e.file,
                    operation: e.operation,
                });
            }
        }
        self.release_disk_op_storage(id, keep_metadata);
        if status == MoveStatus::NeedFullCheck {
            self.force_recheck(id);
        }
        if resume {
            self.update_running(id);
        }
    }

    /// Once no move or rename is pending: forgets the storage they ran on, and releases it if
    /// it was made for them (a stopped torrent's).
    fn release_disk_op_storage(&mut self, id: TorrentId, keep_metadata: bool) {
        let Some(t) = self.torrent(id) else { return };
        if t.moves_pending > 0 || t.renames_pending > 0 {
            return;
        }
        if let Some((s, true)) = t.moving.take() {
            if !keep_metadata && !t.is_active() {
                t.ti = None;
            }
            self.disk.async_fence(&s, FenceAction::Release);
        }
    }

    /// libtorrent `torrent::rename_file`. A running torrent's storage renames the file behind
    /// the jobs already queued, holding later ones back; a stopped torrent gets a storage just
    /// for it, as for a move.
    fn rename_file(&mut self, id: TorrentId, file: u32, name: String) {
        let fail = |e: &mut Engine, error| e.alert(Alert::FileRenameFailed { id, index: file, error });
        let Some(t) = self.torrent_ref(id) else { return };
        if !t.has_metadata() {
            return fail(self, Error::Unsupported("renaming files before the metadata is known"));
        }
        if name.is_empty() {
            let e = std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty file name");
            return fail(self, e.into());
        }
        if !self.ensure_metadata(id) {
            return fail(self, Error::Unsupported("renaming files without metadata"));
        }
        let t = self.torrent(id).expect("exists");
        let ti = t.ti.clone().expect("loaded");
        if file as usize >= ti.num_files() {
            let e = std::io::Error::new(std::io::ErrorKind::InvalidInput, "no such file in the torrent");
            return fail(self, e.into());
        }
        let storage = match (&t.storage, &t.moving) {
            (Some(s), _) | (None, Some((s, _))) => s.clone(),
            (None, None) => {
                let (save_path, prio) = (t.save_path.clone(), t.file_priority.clone());
                let s = self.disk.new_storage(save_path, ti, prio);
                self.storages.insert(s.id, id);
                self.torrent(id).expect("exists").moving = Some((s.clone(), true));
                s
            }
        };
        self.torrent(id).expect("exists").renames_pending += 1;
        self.disk.async_rename_file(&storage, file, name);
    }

    /// libtorrent `torrent::on_file_renamed`. `resume` starts a torrent whose start waited for
    /// the rename (not while shutting down).
    fn on_file_renamed(
        &mut self,
        storage: u64,
        file: u32,
        name: String,
        error: Option<(String, Error)>,
        resume: bool,
    ) {
        let Some(&id) = self.storages.get(&storage) else { return };
        let keep_metadata = self.store.is_none();
        let Some(t) = self.torrent(id) else { return };
        t.renames_pending = t.renames_pending.saturating_sub(1);
        match error {
            None => {
                let old_name =
                    t.ti.as_ref().map(|ti| ti.files().file_path(file as usize, "")).unwrap_or_default();
                t.file_renamed(file, &name);
                t.resume_dirty = true;
                t.status_dirty = true;
                self.alert(Alert::FileRenamed { id, index: file, old_name, new_name: name });
            }
            Some((path, error)) => {
                let message = format!("cannot rename \"{path}\": {error}");
                self.out.push(Alert::Log { id: Some(id), message });
                self.alert(Alert::FileRenameFailed { id, index: file, error });
            }
        }
        self.release_disk_op_storage(id, keep_metadata);
        if resume {
            self.update_running(id);
        }
    }

    // --- Running, pausing, checking -----------------------------------------------------------

    fn set_flags(&mut self, id: TorrentId, flags: TorrentFlags, mask: TorrentFlags) {
        let Some(t) = self.torrent(id) else { return };
        let old = t.flags;
        t.flags = (t.flags & !mask) | (flags & mask);
        // Super seeding is for seeds only (µTorrent greys "Initial Seeding" out otherwise);
        // without a known piece set the check waits for `on_checked`.
        if t.flags.contains(TorrentFlags::SUPER_SEEDING)
            && !old.contains(TorrentFlags::SUPER_SEEDING)
            && t.have_bits().is_some()
            && !t.checking
            && !t.is_seed()
        {
            t.flags.remove(TorrentFlags::SUPER_SEEDING);
        }
        let new = t.flags;
        if old == new {
            return;
        }
        t.status_dirty = true;
        t.resume_dirty = true;
        if let Some(p) = t.picker.as_mut() {
            p.set_sequential(new.contains(TorrentFlags::SEQUENTIAL_DOWNLOAD));
        }
        // A user pause/resume clears a torrent's error, as libtorrent's `clear_error` + resume.
        if old.contains(TorrentFlags::PAUSED) && !new.contains(TorrentFlags::PAUSED) {
            t.error = None;
        }
        match (old.contains(TorrentFlags::PAUSED), new.contains(TorrentFlags::PAUSED)) {
            (false, true) => self.alert(Alert::TorrentPaused { id }),
            (true, false) => self.alert(Alert::TorrentResumed { id }),
            _ => {}
        }
        if old.contains(TorrentFlags::AUTO_MANAGED) != new.contains(TorrentFlags::AUTO_MANAGED) {
            self.needs_auto_manage = true;
        }
        if !old.contains(TorrentFlags::APPLY_IP_FILTER) && new.contains(TorrentFlags::APPLY_IP_FILTER) {
            self.ip_filter_updated(id);
        }
        if old.contains(TorrentFlags::SUPER_SEEDING) && !new.contains(TorrentFlags::SUPER_SEEDING) {
            self.superseed_end(id);
        }
        // libtorrent `torrent::set_upload_mode`: tell every peer we are (not) interested.
        if old.contains(TorrentFlags::UPLOAD_MODE) != new.contains(TorrentFlags::UPLOAD_MODE) {
            if let Some(t) = self.torrent_ref(id) {
                for slot in t.peers.clone() {
                    if self.peers.get(slot).is_some_and(|p| p.state == PeerState::Active) {
                        self.update_interest(slot);
                    }
                }
            }
        }
        self.update_running(id);
    }

    /// Starts or stops the torrent to match its flags and the session state.
    fn update_running(&mut self, id: TorrentId) {
        let Some(t) = self.torrent_ref(id) else { return };
        let want = self.is_running(t);
        let running = t.is_active();
        if want && !running {
            self.start_torrent(id);
        } else if !want && running {
            self.stop_torrent(id, false);
        }
    }

    fn start_torrent(&mut self, id: TorrentId) {
        // A new storage would see the files mid-move: start once the move is done.
        if self.torrent_ref(id).is_some_and(|t| t.moves_pending > 0 || t.renames_pending > 0) {
            return;
        }
        if !self.ensure_metadata(id) {
            let Some(t) = self.torrent(id) else { return };
            t.status_dirty = true;
            if t.has_metadata() {
                t.error = Some("metadata missing from the session store".into());
                return;
            }
            // A magnet: fetch the info dictionary from peers first (BEP 9).
            t.fetch = Some(crate::proto::ut_metadata::MetadataFetch::new());
            t.started_at = Some(Instant::now());
            self.set_state(id, TorrentState::DownloadingMetadata);
            self.connect_peers_for(id, 10);
            self.prioritize_dht(id);
            self.lsd_announce(id, Instant::now());
            return;
        }
        let t = self.torrent(id).expect("exists");
        let ti = t.ti.clone().expect("loaded");
        let save_path = t.save_path.clone();
        let peer_slots = t.peers.clone();
        let file_priority = t.file_priority.clone();
        let storage = self.disk.new_storage(save_path, ti.clone(), file_priority);
        self.storages.insert(storage.id, id);
        let mut picker = PiecePicker::new(
            ti.num_pieces(),
            ti.piece_length(),
            ti.piece_size(ti.num_pieces().saturating_sub(1)),
            BLOCK_SIZE,
        );
        // Peers that stayed connected while the metadata was fetched count toward availability.
        for slot in peer_slots {
            let Some(p) = self.peers.get(slot).filter(|p| p.state == PeerState::Active) else { continue };
            if p.peer_seed {
                picker.inc_seed();
            } else if p.peer_has.len() == ti.num_pieces() as usize {
                picker.inc_refcount_bitfield(&p.peer_has);
            }
        }
        let t = self.torrent(id).expect("exists");
        for (i, &p) in piece_priorities(&ti, &t.file_priority).iter().enumerate() {
            picker.set_piece_priority(i as u32, p);
        }
        picker.set_sequential(t.flags.contains(TorrentFlags::SEQUENTIAL_DOWNLOAD));
        let mode = match t.have.take() {
            Some(have) if have.len() == ti.num_pieces() as usize => CheckMode::Resume(have),
            _ => CheckMode::Full,
        };
        if matches!(mode, CheckMode::Full) {
            // A full check finds only whole pieces; partial blocks are not trusted.
            t.unfinished.clear();
        }
        t.picker = Some(picker);
        t.storage = Some(storage.clone());
        t.checking = true;
        t.checked_pieces = 0;
        t.started_at = Some(Instant::now());
        t.status_dirty = true;
        let state = if matches!(mode, CheckMode::Resume(_)) {
            TorrentState::CheckingResumeData
        } else {
            TorrentState::CheckingFiles
        };
        self.disk.async_check(&storage, mode);
        self.set_state(id, state);
    }

    /// libtorrent `torrent::stop` / `pause`: disconnects peers, tells trackers, releases files.
    fn stop_torrent(&mut self, id: TorrentId, shutting_down: bool) {
        let keep_metadata = self.store.is_none();
        let peer_id = self.peer_id;
        let key = self.tracker_key;
        let port = self.tracker_port();
        let filter = self.tracker_filter(id);
        let Some(t) = self.torrent(id) else { return };
        let peers = t.peers.clone();
        for slot in peers {
            if let Some(p) = self.peers.get_mut(slot) {
                p.close("torrent stopped", false);
            }
        }
        self.close_web_seeds(id, "torrent stopped");
        let t = self.torrent(id).expect("exists");
        let mut jobs = Vec::new();
        for tr in &mut t.trackers {
            if tr.start_sent {
                let req = tracker::make_announce(
                    t.info_hashes.best_v1_compatible(),
                    peer_id,
                    port,
                    t.stat.upload_payload.total,
                    t.stat.download_payload.total,
                    0,
                    t.failed_bytes,
                    Event::Stopped,
                    key,
                    0,
                    tr.tracker_id.clone(),
                );
                jobs.push(TrackerJob {
                    torrent: id,
                    tracker: None,
                    url: tr.url.clone(),
                    kind: JobKind::Announce(req),
                    filter: filter.clone(),
                });
            }
            tr.reset();
        }
        t.fetch = None;
        if let Some(storage) = t.storage.take() {
            self.disk.async_fence(&storage, FenceAction::Release);
        }
        let t = self.torrent(id).expect("exists");
        if let Some(mut p) = t.picker.take() {
            if t.checked || !t.checking {
                t.have = Some(p.have().clone());
                t.unfinished = p.unfinished_pieces();
            }
            p.clear_downloading();
        }
        t.checking = false;
        t.started_at = None;
        t.status_dirty = true;
        t.resume_dirty = true;
        let max_paused = self.settings.max_paused_peerlist_size.max(0) as usize;
        let t = self.torrent(id).expect("exists");
        t.peer_list.trim(max_paused);
        if !keep_metadata && !shutting_down {
            t.ti = None;
        }
        let state = t.settled_state();
        let now = Instant::now();
        for job in jobs {
            self.trackers.start(job, &self.resolver, self.poll.registry(), now);
        }
        if !shutting_down {
            self.set_state(id, state);
        }
    }

    fn force_recheck(&mut self, id: TorrentId) {
        let Some(t) = self.torrent(id) else { return };
        t.have = None;
        t.unfinished.clear();
        t.checked = false;
        t.resume_dirty = true;
        if t.storage.is_some() {
            self.stop_torrent(id, false);
            self.reap_peers();
            if let Some(t) = self.torrent(id) {
                t.have = None;
            }
            self.start_torrent(id);
        }
    }

    fn on_checked(&mut self, id: TorrentId, result: Result<CheckResult, (String, Error)>) {
        let Some(t) = self.torrent(id) else { return };
        t.checking = false;
        match result {
            Err((file, error)) => {
                t.error = Some(format!("{file}: {error}"));
                t.status_dirty = true;
                self.alert(Alert::FileError { id, file, error: error.clone() });
                self.alert(Alert::TorrentError { id, error });
                self.update_running(id);
            }
            Ok(res) => {
                let was_finished_before = t.completed_time != 0;
                let unfinished = std::mem::take(&mut t.unfinished);
                let mut restored = Vec::new();
                if let Some(p) = t.picker.as_mut() {
                    p.set_have(&res.have);
                    // libtorrent `on_resume_data_checked`: partial pieces are kept only when
                    // the resume data matched the files.
                    if !res.resume_failed {
                        for (piece, blocks) in &unfinished {
                            if let Some(mask) = p.restore_partial(*piece, blocks) {
                                restored.push((*piece, mask));
                            }
                        }
                    }
                }
                if let Some(storage) = t.storage.clone() {
                    for (piece, mask) in restored {
                        self.disk.async_restore_partial(&storage, piece, mask);
                    }
                }
                let t = self.torrent(id).expect("exists");
                t.checked = true;
                t.checked_pieces = t.num_pieces;
                // libtorrent `files_checked`: super seeding is turned off for a non-seed.
                if t.flags.contains(TorrentFlags::SUPER_SEEDING) && !t.is_seed() {
                    t.flags.remove(TorrentFlags::SUPER_SEEDING);
                }
                // Peers kept from the metadata download may be interesting now.
                t.interest_dirty = true;
                t.resume_dirty = true;
                t.status_dirty = true;
                let finished = t.is_finished();
                if finished && t.completed_time == 0 {
                    t.completed_time = now_unix();
                }
                if res.resume_failed {
                    self.alert(Alert::Log {
                        id: Some(id),
                        message: "resume data did not match the files; rechecked".into(),
                    });
                }
                let state = self.torrent_ref(id).expect("exists").settled_state();
                self.alert(Alert::TorrentChecked { id });
                self.set_state(id, state);
                if finished {
                    self.remove_from_queue(id);
                    if !was_finished_before {
                        self.alert(Alert::TorrentFinished { id });
                    }
                } else {
                    self.add_to_queue(id);
                }
                self.needs_auto_manage = true;
                self.connect_peers_for(id, 10);
                self.prioritize_dht(id);
                self.lsd_announce(id, Instant::now());
            }
        }
    }

    fn set_file_priorities(&mut self, id: TorrentId, prios: Vec<u8>) {
        let Some(t) = self.torrent(id) else { return };
        let was_finished = t.is_finished();
        t.file_priority = prios;
        t.resume_dirty = true;
        t.status_dirty = true;
        t.interest_dirty = true;
        if let (Some(ti), Some(p)) = (t.ti.clone(), t.picker.as_mut()) {
            for (i, &pr) in piece_priorities(&ti, &t.file_priority).iter().enumerate() {
                p.set_piece_priority(i as u32, pr);
            }
        }
        // The storage moves data in or out of the part file (libtorrent `prioritize_files`).
        if let Some(storage) = t.storage.clone() {
            let prios = t.file_priority.clone();
            self.disk.async_set_file_priority(&storage, prios);
        }
        let t = self.torrent(id).expect("exists");
        let finished = t.is_finished();
        if t.checking || t.storage.is_none() {
            if finished != was_finished {
                if finished { self.remove_from_queue(id) } else { self.add_to_queue(id) }
            }
            return;
        }
        if finished && !was_finished {
            self.on_finished(id);
        } else if !finished && was_finished {
            self.set_state(id, TorrentState::Downloading);
            self.add_to_queue(id);
            self.needs_auto_manage = true;
        }
    }

    /// Every wanted piece is done (libtorrent `torrent::finished`).
    fn on_finished(&mut self, id: TorrentId) {
        let close_redundant = self.settings.close_redundant_connections;
        let Some(t) = self.torrent(id) else { return };
        t.completed_time = now_unix();
        t.resume_dirty = true;
        if t.downloaded_in_session {
            for tr in &mut t.trackers {
                if tr.start_sent && !tr.complete_sent && !tr.updating {
                    tr.next_announce = None;
                }
            }
        }
        let peers = t.peers.clone();
        let state = t.settled_state();
        for slot in peers {
            let Some(p) = self.peers.get_mut(slot) else { continue };
            if p.state != PeerState::Active {
                continue;
            }
            if close_redundant && p.is_seed() {
                p.close("both sides are seeds", false);
                continue;
            }
            if p.am_interested {
                p.am_interested = false;
                peer_wire::encode_simple(p.out(), msg::NOT_INTERESTED);
                self.to_flush.push(slot);
            }
        }
        self.set_state(id, state);
        self.alert(Alert::TorrentFinished { id });
        self.remove_from_queue(id);
        self.needs_auto_manage = true;
        self.save_resume(true);
    }

    // --- Queue --------------------------------------------------------------------------------

    fn remove_from_queue(&mut self, id: TorrentId) {
        if let Some(pos) = self.queue.iter().position(|&q| q == id) {
            self.queue.remove(pos);
            if let Some(t) = self.torrent(id) {
                t.queue_position = None;
                t.status_dirty = true;
            }
            self.renumber_queue(pos);
        }
    }

    fn add_to_queue(&mut self, id: TorrentId) {
        if !self.queue.contains(&id) {
            let pos = self.queue.len() as u32;
            self.queue.push(id);
            if let Some(t) = self.torrent(id) {
                t.queue_position = Some(pos);
                t.status_dirty = true;
            }
        }
    }

    fn renumber_queue(&mut self, from: usize) {
        for pos in from..self.queue.len() {
            let qid = self.queue[pos];
            if let Some(t) = self.torrent(qid) {
                t.queue_position = Some(pos as u32);
                t.status_dirty = true;
                t.resume_dirty = true;
            }
        }
    }

    fn move_in_queue(&mut self, id: TorrentId, m: QueueMove) {
        let Some(pos) = self.queue.iter().position(|&q| q == id) else { return };
        let last = self.queue.len() - 1;
        let target = match m {
            QueueMove::Up => pos.saturating_sub(1),
            QueueMove::Down => (pos + 1).min(last),
            QueueMove::Top => 0,
            QueueMove::Bottom => last,
        };
        if target != pos {
            let item = self.queue.remove(pos);
            self.queue.insert(target, item);
            self.renumber_queue(pos.min(target));
            self.needs_auto_manage = true;
        }
    }

    /// libtorrent `session_impl::recalculate_auto_managed_torrents`, simplified: auto-managed
    /// torrents run in queue order up to `active_downloads` / `active_seeds` / `active_limit`
    /// and are paused beyond that. Errored torrents don't take a slot.
    fn auto_manage(&mut self) {
        self.needs_auto_manage = false;
        self.last_auto_manage = Instant::now();
        let limit = |v: i32| if v < 0 { usize::MAX } else { v as usize };
        let (dl_limit, seed_limit, total_limit) = (
            limit(self.settings.active_downloads),
            limit(self.settings.active_seeds),
            limit(self.settings.active_limit),
        );
        let mut decisions: Vec<(TorrentId, bool)> = Vec::new();
        let mut active_total = 0;
        let mut active_dl = 0;
        for &id in &self.queue {
            let Some(t) = self.torrent_ref(id) else { continue };
            if !t.flags.contains(TorrentFlags::AUTO_MANAGED) || t.error.is_some() {
                continue;
            }
            let run = active_dl < dl_limit && active_total < total_limit;
            if run {
                active_dl += 1;
                active_total += 1;
            }
            decisions.push((id, run));
        }
        let mut active_seeds = 0;
        for id in self.torrent_ids() {
            let t = self.torrent_ref(id).expect("listed");
            if self.queue.contains(&id) || !t.flags.contains(TorrentFlags::AUTO_MANAGED) || t.error.is_some()
            {
                continue;
            }
            let run = active_seeds < seed_limit && active_total < total_limit;
            if run {
                active_seeds += 1;
                active_total += 1;
            }
            decisions.push((id, run));
        }
        for (id, run) in decisions {
            let paused = self.torrent_ref(id).is_some_and(|t| t.is_paused());
            if paused == run {
                let flags = if run { TorrentFlags::empty() } else { TorrentFlags::PAUSED };
                self.set_flags(id, flags, TorrentFlags::PAUSED);
            }
        }
    }

    // --- Disk and resolver completions --------------------------------------------------------

    fn drain_completions(&mut self) {
        while let Ok(c) = self.disk_rx.try_recv() {
            self.on_disk(c);
        }
        let now = Instant::now();
        while let Ok(r) = self.resolve_rx.try_recv() {
            if r.cookie & dht::DHT_COOKIE != 0 {
                self.on_dht_resolved(r);
                continue;
            }
            if r.cookie & web_seed::WEB_COOKIE != 0 {
                self.on_web_resolved(r);
                continue;
            }
            if r.cookie & proxy::PROXY_COOKIE != 0 {
                self.on_proxy_resolved(r);
                continue;
            }
            self.trackers.on_resolved(r, self.poll.registry(), now);
        }
        self.apply_tracker_results();
    }

    fn on_disk(&mut self, c: Completion) {
        match c {
            Completion::Write { storage, piece, offset, error } => {
                let Some(&id) = self.storages.get(&storage) else { return };
                let current =
                    self.torrent_ref(id).and_then(|t| t.storage.as_ref()).is_some_and(|s| s.id == storage);
                if !current {
                    return;
                }
                let block = offset / BLOCK_SIZE;
                match error {
                    None => {
                        if let Some(p) = self.torrent(id).and_then(|t| t.picker.as_mut()) {
                            p.mark_as_finished(piece, block);
                        }
                    }
                    Some((file, error)) => {
                        if let Some(t) = self.torrent(id) {
                            if let Some(p) = t.picker.as_mut() {
                                p.write_failed(piece, block);
                            }
                            // libtorrent stops the torrent with the error (disk full and the like).
                            t.error = Some(format!("{file}: {error}"));
                            t.status_dirty = true;
                        }
                        self.alert(Alert::FileError { id, file, error: error.clone() });
                        self.alert(Alert::TorrentError { id, error });
                        self.update_running(id);
                        self.needs_auto_manage = true;
                    }
                }
                // Writes drained: peers held back by the disk queue may read again.
                if !self.disk_backlogged() {
                    let blocked: Vec<usize> =
                        self.peers.iter().filter(|(_, p)| p.read_blocked).map(|(i, _)| i).collect();
                    for (slot, share) in self.quota_shares(&blocked, true) {
                        self.do_read_capped(slot, share);
                    }
                    self.web_unblock_reads();
                }
            }
            Completion::Hashed { storage, piece, passed } => {
                let Some(&id) = self.storages.get(&storage) else { return };
                let Some(t) = self.torrent(id) else { return };
                let current = t.storage.as_ref().is_some_and(|s| s.id == storage);
                if !current {
                    // The torrent stopped meanwhile; a verified piece is still ours.
                    if passed {
                        if let Some(h) = t.have.as_mut().filter(|h| (piece as usize) < h.len()) {
                            h.set(piece as usize, true);
                        }
                    }
                    return;
                }
                if passed {
                    self.piece_passed(id, piece);
                } else {
                    // libtorrent `torrent::piece_failed`: who sent each block (for blame),
                    // then restore the piece. A piece we had announced (libtorrent's
                    // predictive pieces) is taken back with DONT_HAVE.
                    let (downloaders, announced) = match t.picker.as_mut() {
                        Some(p) => {
                            let d = p.get_downloaders(piece);
                            let announced = p.have_piece(piece);
                            p.we_dont_have(piece);
                            p.piece_failed(piece);
                            (d, announced)
                        }
                        None => (Vec::new(), false),
                    };
                    if announced {
                        self.lost_piece(id, piece);
                    }
                    self.alert(Alert::Log {
                        id: Some(id),
                        message: format!("piece {piece} failed the hash check"),
                    });
                    self.on_piece_failed(id, piece, downloaders);
                }
            }
            Completion::Read { storage, cookie, piece, offset, result } => {
                self.on_read(storage, cookie, piece, offset, result)
            }
            Completion::CheckProgress { storage, checked } => {
                if let Some(&id) = self.storages.get(&storage) {
                    if let Some(t) = self.torrent(id) {
                        t.checked_pieces = checked;
                        t.status_dirty = true;
                    }
                }
            }
            Completion::Checked { storage, result } => {
                let Some(&id) = self.storages.get(&storage) else { return };
                let current =
                    self.torrent_ref(id).and_then(|t| t.storage.as_ref()).is_some_and(|s| s.id == storage);
                if current {
                    self.on_checked(id, result);
                }
            }
            Completion::FilePriority { storage, error: Some((file, error)) } => {
                // libtorrent `on_file_priority`: a failed move out of the part file stops the
                // torrent with the error.
                let Some(&id) = self.storages.get(&storage) else { return };
                if let Some(t) =
                    self.torrent(id).filter(|t| t.storage.as_ref().is_some_and(|s| s.id == storage))
                {
                    t.error = Some(format!("{file}: {error}"));
                    t.status_dirty = true;
                    self.alert(Alert::FileError { id, file, error: error.clone() });
                    self.alert(Alert::TorrentError { id, error });
                    self.update_running(id);
                }
            }
            Completion::FilePriority { error: None, .. } => {}
            Completion::Moved { storage, status, save_path, error } => {
                self.on_storage_moved(storage, status, save_path, error, true)
            }
            Completion::Renamed { storage, file, name, error } => {
                self.on_file_renamed(storage, file, name, error, true)
            }
            Completion::Fenced { storage, action, error } => {
                self.storages.remove(&storage);
                if action != FenceAction::Release {
                    if let Some((id, info_hashes)) = self.deleting.remove(&storage) {
                        match error {
                            None => self.alert(Alert::TorrentDeleted { id, info_hashes }),
                            Some(error) => self.alert(Alert::TorrentDeleteFailed { id, info_hashes, error }),
                        }
                    }
                }
            }
        }
    }

    fn piece_passed(&mut self, id: TorrentId, piece: u32) {
        let Some(t) = self.torrent(id) else { return };
        let Some(p) = t.picker.as_mut() else { return };
        let downloaders = p.get_downloaders(piece);
        p.piece_passed(piece);
        t.interest_dirty = true;
        t.resume_dirty = true;
        t.status_dirty = true;
        let finished = p.is_finished();
        let peers = t.peers.clone();
        for slot in peers {
            if let Some(p) = self.peers.get_mut(slot).filter(|p| p.state == PeerState::Active) {
                peer_wire::encode_u32(p.out(), msg::HAVE, piece);
                self.to_flush.push(slot);
            }
        }
        self.on_piece_pass(id, piece, downloaders);
        if finished {
            self.on_finished(id);
        }
    }

    /// A piece we announced is gone (libtorrent `piece_failed` on a predictive piece): every
    /// peer gets a DONT_HAVE, and its outstanding requests for the piece are rejected.
    fn lost_piece(&mut self, id: TorrentId, piece: u32) {
        // No longer a seed: super-seeded peers get our real piece set.
        self.superseed_end(id);
        let Some(t) = self.torrent(id) else { return };
        t.interest_dirty = true;
        t.resume_dirty = true;
        t.status_dirty = true;
        for slot in t.peers.clone() {
            let Some(p) = self.peers.get_mut(slot).filter(|p| p.state == PeerState::Active) else { continue };
            let queued: Vec<(u32, u32, u32)> =
                p.upload_queue.iter().copied().filter(|r| r.0 == piece).collect();
            p.upload_queue.retain(|r| r.0 != piece);
            if p.supports_fast {
                for r in queued {
                    peer_wire::encode_block_msg(p.out(), msg::REJECT, r.0, r.1, r.2);
                }
            }
            self.write_dont_have(slot, piece);
            self.to_flush.push(slot);
        }
    }

    fn apply_tracker_results(&mut self) {
        let now = Instant::now();
        let min_interval = self.settings.min_announce_interval;
        for (job, outcome) in self.trackers.take_results() {
            let Some(index) = job.tracker else { continue };
            let id = job.torrent;
            let Some(t) = self.torrent(id) else { continue };
            let ih = t.info_hashes.best_v1_compatible();
            // the list may have been replaced while the request was out: find the URL again
            let index = if t.trackers.get(index).is_some_and(|tr| tr.url == job.url) {
                index
            } else {
                let Some(i) = t.trackers.iter().position(|tr| tr.url == job.url) else { continue };
                i
            };
            let tr = &mut t.trackers[index];
            let url = tr.url.clone();
            match outcome {
                TrackerOutcome::Announce(r, tracker_ip) => {
                    tr.updating = false;
                    tr.fails = 0;
                    tr.working = true;
                    tr.message = r.warning.clone().unwrap_or_default();
                    if let JobKind::Announce(req) = &job.kind {
                        match req.event {
                            Event::Started => tr.start_sent = true,
                            Event::Completed => tr.complete_sent = true,
                            _ => {}
                        }
                    }
                    if let Some(tid) = &r.tracker_id {
                        tr.tracker_id = tid.clone();
                    }
                    let interval = r.interval.max(min_interval.min(r.interval.max(60)));
                    tr.next_announce = Some(now + Duration::from_secs(interval as u64));
                    tr.min_announce = r.min_interval.map(|m| now + Duration::from_secs(m as u64));
                    if r.complete.is_some() || r.incomplete.is_some() {
                        let s = tr.scrape.get_or_insert_default();
                        s.complete = r.complete.unwrap_or(s.complete);
                        s.incomplete = r.incomplete.unwrap_or(s.incomplete);
                        t.num_complete = r.complete;
                        t.num_incomplete = r.incomplete;
                    }
                    self.add_peers(id, &r.peers, PeerSource::TRACKER);
                    let num_peers = r.peers.len() as u32;
                    if let Some(w) = r.warning {
                        self.alert(Alert::TrackerWarning { id, url: url.clone(), message: w });
                    }
                    // libtorrent `tracker_response`: the tracker's `external ip` is its vote
                    if let (Some(ip), Some(from)) =
                        (r.external_ip, tracker_ip.filter(|a| !a.is_unspecified()))
                    {
                        self.set_external_address(ip, source::TRACKER, from);
                    }
                    let needs_scrape = r.complete.is_none() && r.incomplete.is_none();
                    self.alert(Alert::TrackerReply { id, url: url.clone(), num_peers });
                    if needs_scrape {
                        let job = TrackerJob {
                            torrent: id,
                            tracker: Some(index),
                            url,
                            kind: JobKind::Scrape(ih),
                            filter: self.tracker_filter(id),
                        };
                        self.trackers.start(job, &self.resolver, self.poll.registry(), now);
                    }
                    self.connect_peers_for(id, 10);
                }
                TrackerOutcome::Scrape(s) => {
                    tr.scrape = Some(s);
                    t.num_complete = Some(s.complete);
                    t.num_incomplete = Some(s.incomplete);
                    t.status_dirty = true;
                    self.alert(Alert::ScrapeReply {
                        id,
                        url,
                        complete: s.complete,
                        incomplete: s.incomplete,
                    });
                }
                // libtorrent `tracker_request_error`: a failed scrape is only reported; a
                // failed announce puts the tracker into its retry backoff
                TrackerOutcome::Failed(message) if matches!(job.kind, JobKind::Scrape(_)) => {
                    self.alert(Alert::ScrapeFailed { id, url, message });
                }
                TrackerOutcome::Failed(message) => {
                    tr.failed(message.clone(), now);
                    let times_in_row = tr.fails;
                    t.status_dirty = true;
                    self.alert(Alert::TrackerError { id, url, times_in_row, message });
                }
            }
        }
    }

    // --- The tick -----------------------------------------------------------------------------

    fn tick(&mut self, now: Instant) {
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        self.stat.tick(dt);
        self.trackers.tick(self.poll.registry(), now);
        self.apply_tracker_results();
        {
            let mut io = UdpIo { socks: &self.udp };
            self.utp.tick(&mut io, now);
        }
        self.utp_dispatch(now);

        let mut running = Vec::new();
        for id in self.torrent_ids() {
            let t = self.torrent_ref(id).expect("listed");
            if t.is_active() {
                running.push(id);
            }
        }
        for (_, p) in self.peers.iter_mut() {
            p.stat.tick(dt);
        }
        for &id in &running {
            self.tick_torrent(id, now, dt);
        }
        self.web_tick(now, dt, &running);
        // Connection pacing (libtorrent `connection_speed` per second, shared by torrents).
        self.connect_budget = (self.connect_budget + self.settings.connection_speed.max(0) as f64 * dt)
            .min(self.settings.connection_speed.max(1) as f64);
        for &id in &running {
            let n = self.connect_budget as usize;
            if n == 0 {
                break;
            }
            let made = self.connect_peers_for(id, n);
            self.connect_budget -= made as f64;
        }
        if now.duration_since(self.last_unchoke)
            >= Duration::from_secs(self.settings.unchoke_interval.max(1) as u64)
        {
            let optimistic = now.duration_since(self.last_optimistic)
                >= Duration::from_secs(self.settings.optimistic_unchoke_interval.max(1) as u64);
            self.run_choker(now, optimistic);
        }
        if self.needs_auto_manage
            || now.duration_since(self.last_auto_manage)
                >= Duration::from_secs(self.settings.auto_manage_interval.max(1) as u64)
        {
            self.auto_manage();
        }
        if now.duration_since(self.last_save) >= SAVE_INTERVAL
            || self.store_flush_due.is_some_and(|d| now >= d)
        {
            self.save_resume(false);
        }
    }

    fn tick_torrent(&mut self, id: TorrentId, now: Instant, dt: f64) {
        let settings = self.settings.clone();
        let Some(t) = self.torrent(id) else { return };
        t.stat.tick(dt);
        if !t.checking {
            t.active_secs += dt;
            if t.is_finished() {
                t.finished_secs += dt;
            }
            if t.is_seed() {
                t.seeding_secs += dt;
            }
        }
        if t.stat.download_rate() > 0 || t.stat.upload_rate() > 0 || !t.peers.is_empty() || t.checking {
            t.status_dirty = true;
        }
        let interest_dirty = std::mem::take(&mut t.interest_dirty);
        let peers = t.peers.clone();
        let checking = t.checking;

        for slot in peers {
            let Some(p) = self.peers.get_mut(slot) else { continue };
            let age = now.duration_since(p.connected_at);
            match p.state {
                PeerState::Connecting if age > Duration::from_secs(settings.peer_connect_timeout as u64) => {
                    p.close("connect timed out", true);
                }
                PeerState::Handshake if age > Duration::from_secs(settings.handshake_timeout as u64) => {
                    p.close("handshake timed out", true);
                }
                PeerState::Active => {
                    if now.duration_since(p.last_receive) > Duration::from_secs(settings.peer_timeout as u64)
                    {
                        p.close("inactivity timeout", false);
                        continue;
                    }
                    if now.duration_since(p.last_sent) > KEEPALIVE_INTERVAL && p.send_buffer_len() == 0 {
                        peer_wire::encode_keepalive(p.out());
                        self.to_flush.push(slot);
                    }
                    if !p.requests.is_empty() && now.duration_since(p.last_piece) > PIECE_TIMEOUT {
                        self.snub(slot);
                    }
                    self.metadata_tick(slot, now);
                    self.superseed_tick(slot, now);
                    if !checking {
                        if interest_dirty {
                            self.update_interest(slot);
                        }
                        self.request_blocks(slot);
                    }
                }
                _ => {}
            }
        }
        self.pex_tick(id, now);
        // Announce.
        let all_tiers = settings.announce_to_all_tiers;
        let all_trackers = settings.announce_to_all_trackers;
        let Some(t) = self.torrent(id) else { return };
        if t.checking {
            return;
        }
        let due = tracker::trackers_to_announce(&t.trackers, now, all_tiers, all_trackers);
        for index in due {
            self.announce(id, index, now);
        }
    }

    fn announce(&mut self, id: TorrentId, index: usize, now: Instant) {
        let (peer_id, key, port, num_want) =
            (self.peer_id, self.tracker_key, self.tracker_port(), self.settings.num_want);
        let filter = self.tracker_filter(id);
        let Some(t) = self.torrent(id) else { return };
        let finished = t.is_finished();
        let left = t.bytes_left();
        let (uploaded, downloaded, corrupt) =
            (t.stat.upload_payload.total, t.stat.download_payload.total, t.failed_bytes);
        let in_session = t.downloaded_in_session;
        let ih = t.info_hashes.best_v1_compatible();
        let tr = &mut t.trackers[index];
        let event = if !tr.start_sent {
            Event::Started
        } else if finished && in_session && !tr.complete_sent {
            Event::Completed
        } else {
            Event::None
        };
        let want = if finished && t.peers.len() > 50 { 0 } else { num_want };
        let tr = &mut t.trackers[index];
        tr.updating = true;
        tr.next_announce = Some(now + Duration::from_secs(60));
        let req = tracker::make_announce(
            ih,
            peer_id,
            port,
            uploaded,
            downloaded,
            left,
            corrupt,
            event,
            key,
            want,
            tr.tracker_id.clone(),
        );
        let job = TrackerJob {
            torrent: id,
            tracker: Some(index),
            url: tr.url.clone(),
            kind: JobKind::Announce(req),
            filter,
        };
        t.status_dirty = true;
        self.trackers.start(job, &self.resolver, self.poll.registry(), now);
    }

    /// Opens up to `max` new outgoing connections for a torrent. Returns how many were started.
    fn connect_peers_for(&mut self, id: TorrentId, max: usize) -> usize {
        if !self.settings.enable_outgoing_tcp && !self.settings.enable_outgoing_utp {
            return 0;
        }
        let global_limit = self.settings.connections_limit;
        let now = Instant::now();
        let Some(t) = self.torrent_ref(id) else { return 0 };
        if !t.is_active() || t.checking || t.error.is_some() {
            return 0;
        }
        let per_torrent = t.max_connections.map_or(usize::MAX, |m| m as usize);
        let room_global = if global_limit < 0 {
            usize::MAX
        } else {
            (global_limit as usize).saturating_sub(self.peers.len())
        };
        let room = per_torrent.saturating_sub(t.peers.len()).min(room_global).min(max);
        if room == 0 {
            return 0;
        }
        let candidates = t.peer_list.connect_candidates(room, now);
        let finished = t.is_finished();
        let mut made = 0;
        for addr in candidates {
            if finished && t_is_seed(self.torrent_ref(id), &addr) {
                continue;
            }
            if self.connect_to_peer(id, addr, now) {
                made += 1;
            }
        }
        made
    }

    /// libtorrent `torrent::set_max_connections`: 0 is no limit. Peers beyond a lowered limit
    /// are disconnected (`disconnect_peers` with `too_many_connections`), the least useful
    /// first: libtorrent's `compare_disconnect_peer` order (peers we are not interested in, then
    /// non-seeds, then the slowest average download, then peers choking us, then the longest
    /// silent).
    fn set_max_connections(&mut self, id: TorrentId, l: u32) {
        let limit = (l > 0).then_some(l);
        let Some(t) = self.torrent(id) else { return };
        if t.max_connections == limit {
            return;
        }
        t.max_connections = limit;
        t.resume_dirty = true;
        t.status_dirty = true;
        let Some(max) = limit else { return };
        let now = Instant::now();
        let slots = t.peers.clone();
        let mut live: Vec<_> = slots
            .into_iter()
            .filter_map(|s| {
                let p = self.peers.get(s).filter(|p| p.closing.is_none())?;
                let secs = now.duration_since(p.connected_at).as_secs();
                let avg = p.stat.download_payload.total / (secs + 1);
                Some((s, (p.am_interested, p.peer_seed, avg, !p.peer_choking, p.last_receive)))
            })
            .collect();
        let excess = live.len().saturating_sub(max as usize);
        if excess == 0 {
            return;
        }
        live.sort_by_key(|&(s, key)| (key, s));
        for &(s, _) in live.iter().take(excess) {
            if let Some(p) = self.peers.get_mut(s) {
                p.close("too many connections", false);
            }
        }
    }

    /// One outgoing connection (libtorrent `torrent::connect_to_peer`): over uTP when it is
    /// enabled and the peer is thought or known to support it (every peer is thought to, until
    /// a uTP connect to it fails), or when outgoing TCP is off; over TCP otherwise. Returns
    /// false if no attempt was made.
    fn connect_to_peer(&mut self, id: TorrentId, addr: SocketAddr, now: Instant) -> bool {
        if self.outgoing_blocked(id, addr) {
            return false;
        }
        let (out_tcp, out_utp) = (self.settings.enable_outgoing_tcp, self.settings.enable_outgoing_utp);
        // through a proxy: TCP to the proxy once its address is known; uTP only if it carries UDP
        let via = match self.peer_proxy() {
            None => None,
            Some(Ok(p)) => Some(p),
            Some(Err(_)) => return false,
        };
        let have_udp = match proxy::udp_route(proxy::udp_policy(&self.udp), &self.udp, proxy::UdpKind::Peer) {
            proxy::UdpRoute::Direct => self.udp.iter().any(|u| u.v4 == addr.is_ipv4()),
            proxy::UdpRoute::Relay(..) | proxy::UdpRoute::Drop => true,
            proxy::UdpRoute::Refuse => false,
        };
        let Some(t) = self.torrent_ref(id) else { return false };
        let source = t.peer_list.get(&addr).map_or(PeerSource::TRACKER, |p| p.source);
        let utp = out_utp && have_udp && (!out_tcp || t.peer_list.wants_utp(&addr));
        if !utp && !out_tcp {
            // libtorrent: "discarding peer: TCP connections disabled"
            return false;
        }
        let mut tunnel = None;
        let transport = if utp {
            let mut io = UdpIo { socks: &self.udp };
            // the user handle is the peer slot, set once it is known
            Ok(Transport::Utp(self.utp.connect(addr, 0, &mut io, now)))
        } else if let Some((cfg, proxy_addr)) = via {
            let target = crate::proto::proxy::Target::Addr(addr);
            cfg.tunnel(&target)
                .map_err(io::Error::other)
                .and_then(|hs| {
                    tunnel = Some(Box::new(proxy::Tunnel::new(hs)));
                    TcpStream::connect(proxy_addr)
                })
                .map(Transport::Tcp)
        } else {
            TcpStream::connect(addr).map(Transport::Tcp)
        };
        match transport {
            Ok(tr) => match self.add_peer_conn(tr, addr, true, source, Some(id)) {
                Some(slot) => {
                    let p = self.peers.get_mut(slot).expect("added");
                    p.proxy = tunnel;
                    let pid = p.id;
                    let t = self.torrent(id).expect("exists");
                    t.peers.push(slot);
                    t.peer_list.connecting(addr, pid, source, now);
                    true
                }
                None => false,
            },
            Err(_) => {
                let t = self.torrent(id).expect("exists");
                t.peer_list.connecting(addr, 0, source, now);
                t.peer_list.disconnected(&addr, true, false, false);
                false
            }
        }
    }

    // --- Choker -------------------------------------------------------------------------------

    /// libtorrent `fixed_slots_choker` (+ `fastest_upload` for finished torrents) with one
    /// optimistic slot rotated every `optimistic_unchoke_interval`.
    fn run_choker(&mut self, now: Instant, rotate_optimistic: bool) {
        self.last_unchoke = now;
        if rotate_optimistic {
            self.last_optimistic = now;
        }
        let slots_limit = self.settings.unchoke_slots_limit;
        let mut candidates: Vec<(usize, u32)> = Vec::new();
        let mut others: Vec<usize> = Vec::new();
        for (slot, p) in self.peers.iter() {
            if p.state != PeerState::Active || p.closing.is_some() {
                continue;
            }
            let Some(t) = p.torrent.and_then(|id| self.torrent_ref(id)) else { continue };
            if !p.peer_interested || t.checking || t.picker.is_none() || !self.is_running(t) {
                others.push(slot);
                continue;
            }
            let rank =
                if t.is_finished() { p.stat.upload_payload.rate() } else { p.stat.download_payload.rate() };
            candidates.push((slot, rank));
        }
        candidates.sort_by_key(|&(slot, rank)| (std::cmp::Reverse(rank), slot));
        let (regular, optimistic) = if slots_limit < 0 {
            (candidates.len(), 0)
        } else {
            let s = slots_limit.max(1) as usize;
            if s >= 2 { (s - 1, 1) } else { (s, 0) }
        };
        // libtorrent `torrent::unchoke_peer`: a torrent at its `max_uploads` unchokes no more
        // regular peers; they stay choked without using up a slot (the optimistic unchoke is
        // exempt)
        let mut unchoke: Vec<usize> = Vec::new();
        let mut rest: Vec<usize> = Vec::new();
        let mut per_torrent: HashMap<TorrentId, u32> = HashMap::new();
        for &(s, _) in &candidates {
            let id = self.peers.get(s).and_then(|p| p.torrent);
            let max = id.and_then(|id| self.torrent_ref(id)).and_then(|t| t.max_uploads);
            let count = id.map_or(0, |id| per_torrent.get(&id).copied().unwrap_or(0));
            if unchoke.len() < regular && max.is_none_or(|m| count < m) {
                unchoke.push(s);
                if let Some(id) = id {
                    *per_torrent.entry(id).or_default() += 1;
                }
            } else {
                rest.push(s);
            }
        }
        if optimistic > 0 && !rest.is_empty() {
            let current = rest.iter().copied().find(|&s| self.peers.get(s).is_some_and(|p| p.optimistic));
            let pick = match current {
                Some(c) if !rotate_optimistic => c,
                _ => *rest
                    .iter()
                    .min_by_key(|&&s| self.peers.get(s).and_then(|p| p.last_optimistic))
                    .expect("non-empty"),
            };
            for &s in &rest {
                if let Some(p) = self.peers.get_mut(s) {
                    p.optimistic = s == pick;
                }
            }
            if let Some(p) = self.peers.get_mut(pick) {
                if current != Some(pick) {
                    p.last_optimistic = Some(now);
                }
            }
            unchoke.push(pick);
        }
        for (slot, _) in &candidates {
            let want = unchoke.contains(slot);
            self.set_choke(*slot, !want, now);
        }
        for slot in others {
            self.set_choke(slot, true, now);
        }
    }

    pub(super) fn set_choke(&mut self, slot: usize, choke: bool, now: Instant) {
        let Some(p) = self.peers.get_mut(slot) else { return };
        if p.am_choking == choke {
            return;
        }
        p.am_choking = choke;
        if choke {
            peer_wire::encode_simple(p.out(), msg::CHOKE);
            p.optimistic = false;
            // Drop their queued requests (rejecting them when the fast extension allows).
            let queued: Vec<_> = p.upload_queue.drain(..).chain(p.reads_in_flight.drain()).collect();
            if p.supports_fast {
                for (piece, start, len) in queued {
                    if !p.fast_set.contains(&piece) {
                        peer_wire::encode_block_msg(p.out(), msg::REJECT, piece, start, len);
                    }
                }
            }
        } else {
            peer_wire::encode_simple(p.out(), msg::UNCHOKE);
            p.last_unchoked = Some(now);
        }
        self.to_flush.push(slot);
    }

    /// Unchokes an interested peer right away when a slot is free (libtorrent does this on
    /// `interested` instead of waiting for the next choker run).
    pub(super) fn maybe_unchoke(&mut self, slot: usize) {
        // Nothing to offer while the metadata or the file check is still pending.
        let ready = self
            .peer_and_torrent(slot)
            .and_then(|(_, t)| t)
            .is_some_and(|t| t.picker.is_some() && !t.checking);
        if !ready {
            return;
        }
        let limit = self.settings.unchoke_slots_limit;
        let unchoked = self.peers.iter().filter(|(_, p)| !p.am_choking).count();
        // and only while its torrent is below `max_uploads` (libtorrent `unchoke_peer`)
        let id = self.peers.get(slot).and_then(|p| p.torrent);
        let torrent_full = id.and_then(|id| self.torrent_ref(id)).is_some_and(|t| {
            t.max_uploads.is_some_and(|m| {
                t.peers.iter().filter(|&&s| self.peers.get(s).is_some_and(|p| !p.am_choking)).count()
                    >= m as usize
            })
        });
        if (limit < 0 || unchoked < limit as usize) && !torrent_full {
            self.set_choke(slot, false, Instant::now());
        }
    }

    // --- Persistence --------------------------------------------------------------------------

    fn save_resume(&mut self, force: bool) {
        self.last_save = Instant::now();
        self.store_flush_due = None;
        let Some(store) = self.store.as_mut() else { return };
        let mut errors = Vec::new();
        for s in &mut self.slots {
            let Some(t) = s.torrent.as_mut() else { continue };
            if t.resume_dirty || force {
                t.resume_dirty = false;
                if let Err(e) = store.put(&t.key, &t.resume_data(), None) {
                    errors.push((t.id, e));
                }
            }
        }
        if let Err(e) = store.flush() {
            errors.push((TorrentId::default(), e));
        }
        for (id, e) in errors {
            let id = (id != TorrentId::default()).then_some(id);
            self.out.push(Alert::Log { id, message: format!("cannot save resume data: {e}") });
        }
    }

    // --- Status -------------------------------------------------------------------------------

    fn post_torrent_updates(&mut self) {
        let now = Instant::now();
        let mut out = Vec::new();
        for i in 0..self.slots.len() {
            let Some(t) = self.slots[i].torrent.as_ref() else { continue };
            if !t.status_dirty {
                continue;
            }
            let (mut peers, mut seeds) = (0u32, 0u32);
            let mut rates = (0u32, 0u32, 0u32, 0u32);
            for &slot in &t.peers {
                if let Some(p) = self.peers.get(slot).filter(|p| p.state == PeerState::Active) {
                    peers += 1;
                    if p.is_seed() {
                        seeds += 1;
                    }
                }
            }
            let (web_peers, web_seeds) = self.web.counts(t.id);
            peers += web_peers;
            seeds += web_seeds;
            rates.0 = t.stat.download_rate();
            rates.1 = t.stat.upload_rate();
            rates.2 = t.stat.download_payload.rate();
            rates.3 = t.stat.upload_payload.rate();
            out.push(t.status(peers, seeds, rates, now));
            self.slots[i].torrent.as_mut().expect("present").status_dirty = false;
        }
        self.alert(Alert::StateUpdate(out));
    }

    fn post_session_stats(&mut self) {
        let mut st = SessionStats {
            num_peers_connected: self.peers.iter().filter(|(_, p)| p.state == PeerState::Active).count()
                as u32,
            recv_bytes: self.stat.total_download(),
            sent_bytes: self.stat.total_upload(),
            recv_payload_bytes: self.stat.download_payload.total,
            sent_payload_bytes: self.stat.upload_payload.total,
            download_rate: self.stat.download_payload.rate(),
            upload_rate: self.stat.upload_payload.rate(),
            has_incoming_connections: self.has_incoming,
            disk_write_queue: (self.disk.queued_write_bytes() / BLOCK_SIZE as u64) as u32,
            dht_nodes: self.dht.num_nodes(),
            utp_sockets: self.utp.num_sockets() as u32,
            utp_packets_in: self.utp.counters().packets_in,
            utp_packets_out: self.utp.counters().packets_out,
            utp_packet_loss: self.utp.counters().packet_loss,
            utp_timeouts: self.utp.counters().timeout,
            utp_resends: self.utp.counters().packet_resend,
            ..SessionStats::default()
        };
        for s in &self.slots {
            if let Some(t) = &s.torrent {
                st.num_torrents += 1;
                if t.is_active() {
                    if t.is_finished() { st.num_seeding += 1 } else { st.num_downloading += 1 }
                }
            }
        }
        self.alert(Alert::SessionStats(st));
    }

    fn post_peer_info(&mut self, id: TorrentId) {
        let Some(t) = self.torrent_ref(id) else { return };
        let num_pieces = t.num_pieces.max(1);
        let endgame = t.picker.as_ref().is_some_and(|p| p.in_endgame());
        let mut peers = Vec::new();
        for &slot in &t.peers {
            let Some(p) = self.peers.get(slot) else { continue };
            let mut flags = PeerFlags::empty();
            flags.set(PeerFlags::INTERESTING, p.am_interested);
            flags.set(PeerFlags::CHOKED, p.am_choking);
            flags.set(PeerFlags::REMOTE_INTERESTED, p.peer_interested);
            flags.set(PeerFlags::REMOTE_CHOKED, p.peer_choking);
            flags.set(PeerFlags::SUPPORTS_EXTENSIONS, p.supports_extensions);
            flags.set(PeerFlags::OUTGOING_CONNECTION, p.outgoing);
            flags.set(PeerFlags::HANDSHAKE, p.state == PeerState::Handshake);
            flags.set(PeerFlags::CONNECTING, p.state == PeerState::Connecting);
            flags.set(PeerFlags::SEED, p.is_seed());
            flags.set(PeerFlags::OPTIMISTIC_UNCHOKE, p.optimistic);
            flags.set(PeerFlags::SNUBBED, p.snubbed);
            flags.set(PeerFlags::ENDGAME_MODE, endgame && p.am_interested);
            flags.set(PeerFlags::RC4_ENCRYPTED, p.rc4.is_some());
            flags.set(PeerFlags::PLAINTEXT_ENCRYPTED, p.encrypted && p.rc4.is_none());
            flags.set(PeerFlags::UTP_SOCKET, p.sock.is_utp());
            let trust = Self::trust_of(Some(t), &p.addr);
            flags.set(PeerFlags::ON_PAROLE, trust.on_parole);
            let have = if p.is_seed() { num_pieces } else { p.peer_has.count() as u32 };
            peers.push(PeerInfo {
                ip: p.addr,
                client: p.ext.as_ref().and_then(|e| e.client.clone()).unwrap_or_else(|| p.client.clone()),
                flags,
                source: p.source,
                progress_ppm: (have as u64 * 1_000_000 / num_pieces as u64) as u32,
                down_speed: p.stat.download_payload.rate(),
                up_speed: p.stat.upload_payload.rate(),
                total_download: p.stat.download_payload.total,
                total_upload: p.stat.upload_payload.total,
                download_queue_length: p.requests.len() as u32,
                upload_queue_length: (p.upload_queue.len() + p.reads_in_flight.len()) as u32,
                num_hashfails: trust.hashfails as u32,
                is_web_seed: false,
            });
        }
        peers.extend(self.web_peer_infos(id));
        self.alert(Alert::PeerInfo { id, peers });
    }
}

fn t_is_seed(t: Option<&Torrent>, addr: &SocketAddr) -> bool {
    t.and_then(|t| t.peer_list.get(addr)).is_some_and(|p| p.seed)
}

/// Copies fast-resume fields onto a torrent.
fn apply_resume(t: &mut Torrent, r: &ResumeData) {
    if !r.name.is_empty() {
        t.name = r.name.clone();
    }
    if let Some(p) = &r.pieces {
        if t.num_pieces == 0 || p.len() == t.num_pieces as usize {
            t.have = Some(p.clone());
            t.unfinished = r.unfinished.clone();
        }
    }
    t.prev_uploaded = r.total_uploaded;
    t.prev_downloaded = r.total_downloaded;
    t.active_secs = r.active_time as f64;
    t.finished_secs = r.finished_time as f64;
    t.seeding_secs = r.seeding_time as f64;
    if r.added_time > 0 {
        t.added_time = r.added_time;
    }
    t.completed_time = r.completed_time;
    let mut flags = TorrentFlags::empty();
    flags.set(TorrentFlags::PAUSED, r.paused);
    flags.set(TorrentFlags::AUTO_MANAGED, r.auto_managed);
    flags.set(TorrentFlags::SEQUENTIAL_DOWNLOAD, r.sequential_download);
    flags.set(TorrentFlags::SUPER_SEEDING, r.super_seeding);
    flags.set(TorrentFlags::DISABLE_DHT, r.disable_dht);
    flags.set(TorrentFlags::DISABLE_LSD, r.disable_lsd);
    flags.set(TorrentFlags::DISABLE_PEX, r.disable_pex);
    flags.insert(TorrentFlags::APPLY_IP_FILTER);
    t.flags = flags;
    if !r.file_priority.is_empty() {
        let n = t.file_priority.len().max(r.file_priority.len());
        t.file_priority = r.file_priority.clone();
        t.file_priority.resize(n, DownloadPriority::NORMAL.0);
    }
    for p in &r.peers {
        t.peer_list.add(*p, PeerSource::RESUME_DATA, ExternalIp::default(), usize::MAX);
    }
    t.download_limit.set_limit(r.download_rate_limit);
    t.upload_limit.set_limit(r.upload_rate_limit);
    t.max_connections = r.max_connections;
    t.max_uploads = r.max_uploads;
    t.renamed_files.extend(r.mapped_files.iter().map(|(&i, n)| (i, n.clone())));
    t.apply_renames();
}
