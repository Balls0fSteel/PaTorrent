// Ported from libtorrent-rasterbar 2.0.11: src/proxy_settings.cpp, src/instantiate_connection.cpp
// (which connections use the proxy, and how), src/udp_socket.cpp (the `socks5` class that keeps
// the UDP ASSOCIATE tunnel up, `retry_connection`, and `udp_socket::send` / `read` with a proxy),
// src/session_impl.cpp (`proxy()`, `update_proxy`, the "proxy" listen socket), src/torrent.cpp
// (web seeds through a proxy), src/utp_socket_manager.cpp (`mtu_for_dest` with SOCKS5)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the proxy's host name is resolved once per settings change (and again after a
// failure) instead of by every connection; connections wait while it resolves. One UDP
// associate serves the session, on the UDP listen socket of the proxy's address family
// (libtorrent: one per listen socket that can route to the proxy). A relay address of 0.0.0.0
// in the associate's reply means the proxy's own address (libtorrent sends to 0.0.0.0).
// uTP datagrams sent while the associate is being made are dropped as if lost rather than
// refused; DHT and UDP tracker datagrams are held (a few) and sent once it is up.
// UDP trackers go through the listen socket only when they are proxied (they have their own
// sockets otherwise).

//! Proxy servers: which connections use one, the tunnel handshake on a connected socket, and
//! the SOCKS5 UDP associate that carries uTP, the DHT and UDP trackers.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use mio::net::TcpStream;
use mio::{Interest, Token};

use super::{Engine, UdpListen};
use crate::alert::{Alert, ProxyOperation};
use crate::net::resolver::Resolved;
use crate::net::sys;
use crate::proto::proxy::{
    Bound, Handshake, ProxyError, Socks5Command, Step, Target, UdpSource, udp_unwrap, udp_wrap,
};
use crate::settings::{ProxyType, Settings};

/// Resolver cookies of the proxy's host name (the DHT's carry bit 63, web seeds' bit 62).
pub(super) const PROXY_COOKIE: u64 = 1 << 61;
/// The SOCKS5 UDP associate's TCP connection.
pub(super) const PROXY_UDP_TOKEN: Token = Token(2);
/// libtorrent `socks5::on_connect_timeout`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A failed lookup of the proxy's name is tried again after this.
const LOOKUP_RETRY: Duration = Duration::from_secs(30);
/// The same proxy failure of peer connections is posted at most this often.
const FAULT_ALERT_INTERVAL: Duration = Duration::from_secs(60);
/// DHT and UDP tracker datagrams kept while the UDP associate is being made.
const MAX_HELD: usize = 64;

/// libtorrent `aux::proxy_settings`: the proxy and which connections use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProxyConfig {
    pub kind: ProxyType,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
    pub hostnames: bool,
    pub peers: bool,
    pub trackers: bool,
    pub send_host: bool,
    pub udp_send_local_ep: bool,
}

impl ProxyConfig {
    pub fn from_settings(s: &Settings) -> Option<ProxyConfig> {
        (s.proxy_type != ProxyType::None).then(|| ProxyConfig {
            kind: s.proxy_type,
            host: s.proxy_hostname.clone(),
            port: s.proxy_port,
            user: s.proxy_username.clone(),
            pass: s.proxy_password.clone(),
            hostnames: s.proxy_hostnames,
            peers: s.proxy_peer_connections,
            trackers: s.proxy_tracker_connections,
            send_host: s.proxy_send_host_in_connect,
            udp_send_local_ep: s.socks5_udp_send_local_ep,
        })
    }

    /// The credentials sent to the proxy: only the `_pw` types authenticate (libtorrent
    /// `instantiate_connection`).
    pub fn creds(&self) -> Option<(&str, &str)> {
        matches!(self.kind, ProxyType::Socks5Pw | ProxyType::HttpPw).then_some((&self.user, &self.pass))
    }

    /// Whether a host name goes to the proxy as is (SOCKS5 / SOCKS4a with `proxy_hostnames`, HTTP
    /// CONNECT with `proxy_send_host_in_connect`) instead of being resolved locally first.
    pub fn resolves_names(&self) -> bool {
        match self.kind {
            ProxyType::Socks4 | ProxyType::Socks5 | ProxyType::Socks5Pw => self.hostnames,
            ProxyType::Http | ProxyType::HttpPw => self.send_host,
            ProxyType::None => false,
        }
    }

    /// An HTTP proxy is asked for plain `http://` URLs directly, without CONNECT (libtorrent
    /// `http_connection::get` and `web_peer_connection` with `using_proxy`).
    pub fn plain_http(&self, https: bool) -> bool {
        self.kind.is_http() && !https
    }

    /// The handshake that makes the proxy connect to `target`.
    pub fn tunnel(&self, target: &Target) -> Result<Handshake, ProxyError> {
        match self.kind {
            ProxyType::Socks4 => Handshake::socks4(target, &self.user),
            ProxyType::Socks5 | ProxyType::Socks5Pw => {
                Ok(Handshake::socks5(Socks5Command::Connect, target, self.creds()))
            }
            ProxyType::Http | ProxyType::HttpPw => {
                Ok(Handshake::http_connect(target, self.creds(), self.send_host))
            }
            ProxyType::None => Err(ProxyError::GeneralFailure),
        }
    }

    /// `socks5://host:port`, for messages.
    pub fn label(&self) -> String {
        let scheme = match self.kind {
            ProxyType::Socks4 => "socks4",
            ProxyType::Socks5 | ProxyType::Socks5Pw => "socks5",
            ProxyType::Http | ProxyType::HttpPw => "http",
            ProxyType::None => "none",
        };
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        format!("{scheme}://{host}:{}", self.port)
    }
}

/// Peers go through a proxy: libtorrent then opens one "proxy" listen socket (UDP only, no TCP
/// listener) and refuses incoming TCP connections.
pub(crate) fn peer_mode(s: &Settings) -> bool {
    s.proxy_type != ProxyType::None && s.proxy_peer_connections
}

// --- The tunnel handshake ---------------------------------------------------------------------

#[derive(Debug)]
pub(crate) enum TunnelError {
    Proxy(ProxyError),
    Io(io::Error),
    /// The proxy closed the connection during the handshake.
    Closed,
}

impl fmt::Display for TunnelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TunnelError::Proxy(e) => write!(f, "proxy: {e}"),
            TunnelError::Io(e) => write!(f, "proxy: {e}"),
            TunnelError::Closed => f.write_str("proxy: connection closed during the handshake"),
        }
    }
}

impl TunnelError {
    /// The proxy turned us down (authentication, protocol), not the target.
    pub fn is_proxy_fault(&self) -> bool {
        matches!(self, TunnelError::Proxy(e) if e.is_proxy_fault())
    }
}

/// A proxy handshake on a connected, non-blocking socket. It reads exactly what the handshake
/// needs, so the tunnelled stream starts untouched.
#[derive(Debug)]
pub(crate) struct Tunnel {
    hs: Handshake,
    out: Vec<u8>,
}

impl Tunnel {
    pub fn new(mut hs: Handshake) -> Tunnel {
        let mut out = Vec::new();
        hs.start(&mut out);
        Tunnel { hs, out }
    }

    /// Goes as far as the socket allows. `Ok(None)`: waiting for the socket; `Ok(Some(bound))`:
    /// the tunnel is up.
    pub fn drive<S: Read + Write>(&mut self, s: &mut S) -> Result<Option<Option<Bound>>, TunnelError> {
        let mut buf = [0u8; 512];
        loop {
            while !self.out.is_empty() {
                match s.write(&self.out) {
                    Ok(0) => return Err(TunnelError::Closed),
                    Ok(n) => {
                        self.out.drain(..n);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(TunnelError::Io(e)),
                }
            }
            let want = self.hs.want();
            if want == 0 {
                return if self.hs.is_done() {
                    Ok(Some(None))
                } else {
                    Err(TunnelError::Proxy(ProxyError::GeneralFailure))
                };
            }
            let n = want.min(buf.len());
            match s.read(&mut buf[..n]) {
                Ok(0) => return Err(TunnelError::Closed),
                Ok(k) => match self.hs.feed(&buf[..k], &mut self.out) {
                    Ok((_, Step::Done(bound))) => return Ok(Some(bound)),
                    Ok((_, Step::NeedMore)) => {}
                    Err(e) => return Err(TunnelError::Proxy(e)),
                },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(TunnelError::Io(e)),
            }
        }
    }
}

// --- UDP through the proxy --------------------------------------------------------------------

/// How the UDP listen sockets treat datagrams while a proxy is set (libtorrent
/// `udp_socket::m_proxy_settings` and its `socks5` connection). Every UDP listen socket carries
/// a copy; the relay is set on the one that made the associate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct UdpProxy {
    /// A proxy is set.
    pub on: bool,
    pub peers: bool,
    pub trackers: bool,
    /// A SOCKS5 proxy, which can carry UDP once the associate is made.
    pub socks5: bool,
    /// Where proxied datagrams go (the associate's reply).
    pub relay: Option<SocketAddr>,
}

/// What a datagram is for (libtorrent's `udp_send_flags_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UdpKind {
    /// uTP (`peer_connection`).
    Peer,
    /// UDP trackers (`tracker_connection`).
    Tracker,
    /// The DHT (no flag: proxied whenever a proxy is set).
    Dht,
}

/// Where a datagram goes (libtorrent `udp_socket::send`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UdpRoute {
    /// Not proxied.
    Direct,
    /// Wrapped, through the listen socket at this index to the relay.
    Relay(usize, SocketAddr),
    /// The associate is not up yet: the datagram is lost.
    Drop,
    /// The proxy carries no UDP (libtorrent `permission_denied`).
    Refuse,
}

pub(crate) fn udp_route(policy: UdpProxy, socks: &[UdpListen], kind: UdpKind) -> UdpRoute {
    let use_proxy = policy.on
        && match kind {
            UdpKind::Peer => policy.peers,
            UdpKind::Tracker => policy.trackers,
            UdpKind::Dht => true,
        };
    if !use_proxy {
        return UdpRoute::Direct;
    }
    if let Some((i, relay)) = socks.iter().enumerate().find_map(|(i, u)| u.proxy.relay.map(|r| (i, r))) {
        return UdpRoute::Relay(i, relay);
    }
    if policy.socks5 { UdpRoute::Drop } else { UdpRoute::Refuse }
}

/// The policy as the listen sockets carry it.
pub(crate) fn udp_policy(socks: &[UdpListen]) -> UdpProxy {
    socks.first().map(|u| u.proxy).unwrap_or_default()
}

/// Sends `data` to `to` through the relay, with the SOCKS5 UDP header (libtorrent
/// `udp_socket::wrap`).
pub(crate) fn send_relayed(
    u: &UdpListen,
    relay: SocketAddr,
    to: &Target,
    data: &[u8],
    dont_fragment: bool,
) -> io::Result<usize> {
    let mut pkt = Vec::with_capacity(data.len() + 22);
    udp_wrap(to, data, &mut pkt);
    let df = dont_fragment && relay.is_ipv4();
    if df {
        let _ = sys::set_dont_fragment(&u.sock, true);
    }
    let r = u.sock.send_to(&pkt, relay);
    if df {
        let _ = sys::set_dont_fragment(&u.sock, false);
    }
    r
}

/// A datagram that arrived on a UDP listen socket, as libtorrent `udp_socket::read` passes it on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UdpIn<'a> {
    /// Not proxied: as it came.
    Direct,
    /// From the relay, unwrapped.
    Proxied(UdpSource<'a>, &'a [u8]),
    /// Dropped: not from the relay, not a SOCKS5 datagram, or a direct datagram while
    /// everything is proxied.
    Drop,
}

pub(crate) fn udp_incoming<'a>(p: &UdpProxy, from: SocketAddr, buf: &'a [u8]) -> UdpIn<'a> {
    if let Some(relay) = p.relay {
        if super::normalize_addr(relay) != from {
            return UdpIn::Drop;
        }
        return match udp_unwrap(buf) {
            Some((src, data)) => UdpIn::Proxied(src, data),
            None => UdpIn::Drop,
        };
    }
    // if we proxy everything, block all packets that aren't coming from the proxy
    if p.on && p.peers && p.trackers { UdpIn::Drop } else { UdpIn::Direct }
}

// --- Session state ----------------------------------------------------------------------------

#[derive(Debug)]
enum Lookup {
    Idle,
    Resolving(u64),
    Ready(SocketAddr),
    Failed { retry: Instant, error: String },
}

/// libtorrent `socks5`: the TCP connection that keeps the UDP associate alive.
enum Assoc {
    /// Waiting to (re)connect.
    Waiting(Instant),
    Connecting {
        sock: TcpStream,
        tunnel: Box<Tunnel>,
        connected: bool,
        deadline: Instant,
    },
    Active {
        sock: TcpStream,
    },
    /// The proxy refused us for good (authentication, protocol): no retry until the settings
    /// change (libtorrent closes the socket without `retry_connection`).
    Stopped,
}

/// The session's proxy.
pub(super) struct ProxyState {
    pub cfg: Option<ProxyConfig>,
    lookup: Lookup,
    next_cookie: u64,
    assoc: Option<Assoc>,
    /// The UDP listen socket the associate belongs to (its local address).
    assoc_local: Option<SocketAddr>,
    relay: Option<SocketAddr>,
    /// Datagrams waiting for the associate.
    held: Vec<(Target, Vec<u8>)>,
    failures: u32,
    last_fault: Option<(String, Instant)>,
}

impl Default for ProxyState {
    fn default() -> ProxyState {
        ProxyState {
            cfg: None,
            lookup: Lookup::Idle,
            next_cookie: 0,
            assoc: None,
            assoc_local: None,
            relay: None,
            held: Vec::new(),
            failures: 0,
            last_fault: None,
        }
    }
}

impl ProxyState {
    /// The proxy's address, once resolved.
    pub fn addr(&self) -> Option<SocketAddr> {
        match self.lookup {
            Lookup::Ready(a) => Some(a),
            _ => None,
        }
    }

    /// Why the proxy cannot be used, if its name failed to resolve.
    pub fn lookup_error(&self) -> Option<&str> {
        match &self.lookup {
            Lookup::Failed { error, .. } => Some(error),
            _ => None,
        }
    }

    /// The proxy peers and web seeds go through.
    pub fn for_peers(&self) -> Option<&ProxyConfig> {
        self.cfg.as_ref().filter(|c| c.peers)
    }

    /// The proxy trackers go through.
    pub fn for_trackers(&self) -> Option<&ProxyConfig> {
        self.cfg.as_ref().filter(|c| c.trackers)
    }

    pub fn deadline(&self) -> Option<Instant> {
        let lookup = match self.lookup {
            Lookup::Failed { retry, .. } => Some(retry),
            _ => None,
        };
        let assoc = match &self.assoc {
            Some(Assoc::Waiting(at)) => Some(*at),
            Some(Assoc::Connecting { deadline, .. }) => Some(*deadline),
            _ => None,
        };
        match (lookup, assoc) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn udp_policy(&self) -> UdpProxy {
        let Some(c) = &self.cfg else { return UdpProxy::default() };
        UdpProxy {
            on: true,
            peers: c.peers,
            trackers: c.trackers,
            socks5: c.kind.is_socks5() && !matches!(self.assoc, Some(Assoc::Stopped)),
            relay: None,
        }
    }
}

/// libtorrent `socks5::retry_connection`: 5 s, then longer with every failure, at most 125 s.
fn retry_delay(failures: u32) -> Duration {
    let f = failures.min(200) as u64;
    Duration::from_secs((f * f / 2).min(120) + 5)
}

impl Engine {
    /// The announced listen port: none while peers go through a proxy, since nothing can
    /// connect to us (libtorrent `listen_port` of a proxy listen socket).
    pub(super) fn announce_port(&self) -> u16 {
        if self.proxy.for_peers().is_some() { 0 } else { self.listen_port }
    }

    /// The port sent to trackers: libtorrent's `make_announce_port` turns a proxy's 0 into 1.
    pub(super) fn tracker_port(&self) -> u16 {
        if self.proxy.for_peers().is_some() { 1 } else { self.listen_port }
    }

    /// libtorrent `update_proxy`: called at startup and whenever the settings change. A new
    /// proxy is resolved, the UDP associate is made again, and the trackers and UDP sockets
    /// learn the new policy.
    pub(super) fn update_proxy(&mut self, now: Instant) {
        let cfg = ProxyConfig::from_settings(&self.settings);
        if cfg == self.proxy.cfg {
            return;
        }
        self.close_udp_associate();
        self.proxy = ProxyState { cfg, next_cookie: self.proxy.next_cookie, ..ProxyState::default() };
        let message = match &self.proxy.cfg {
            Some(c) => format!(
                "proxy: {}{}{}",
                c.label(),
                if c.peers { ", peers" } else { "" },
                if c.trackers { ", trackers" } else { "" }
            ),
            None => "proxy: none".to_owned(),
        };
        self.alert(Alert::Log { id: None, message });
        self.proxy_lookup(now);
        self.sync_proxy();
    }

    /// Resolves the proxy's host name (an IP literal or a cached name is ready at once).
    fn proxy_lookup(&mut self, now: Instant) {
        let Some(c) = &self.proxy.cfg else { return };
        let (host, port) = (c.host.clone(), c.port);
        if host.is_empty() {
            let error = "no proxy host name set".to_owned();
            self.proxy.lookup = Lookup::Failed { retry: now + LOOKUP_RETRY, error: error.clone() };
            return self.proxy_alert(ProxyOperation::HostnameLookup, error);
        }
        self.proxy.next_cookie += 1;
        let cookie = PROXY_COOKIE | self.proxy.next_cookie;
        let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(&host).to_owned();
        match self.resolver.resolve(&host, cookie) {
            Some(ips) if !ips.is_empty() => self.proxy.lookup = Lookup::Ready(SocketAddr::new(ips[0], port)),
            Some(_) => {
                self.proxy.lookup =
                    Lookup::Failed { retry: now + LOOKUP_RETRY, error: "host not found".into() }
            }
            None => self.proxy.lookup = Lookup::Resolving(cookie),
        }
    }

    /// The proxy's host name resolved (or failed).
    pub(super) fn on_proxy_resolved(&mut self, r: Resolved) {
        if !matches!(self.proxy.lookup, Lookup::Resolving(c) if c == r.cookie) {
            return;
        }
        let now = Instant::now();
        let port = self.proxy.cfg.as_ref().map_or(0, |c| c.port);
        match r.result {
            Ok(ips) if !ips.is_empty() => self.proxy.lookup = Lookup::Ready(SocketAddr::new(ips[0], port)),
            res => {
                let error = format!("{}: {}", r.host, res.err().unwrap_or_else(|| "host not found".into()));
                self.proxy.lookup = Lookup::Failed { retry: now + LOOKUP_RETRY, error: error.clone() };
                self.proxy_alert(ProxyOperation::HostnameLookup, error);
            }
        }
        self.sync_proxy();
    }

    /// Hands the proxy's state to its users: the trackers (which may have requests waiting for
    /// the proxy's address), the UDP listen sockets, and the UDP associate.
    pub(super) fn sync_proxy(&mut self) {
        let now = Instant::now();
        let tracker_proxy = self.proxy.for_trackers().cloned();
        let addr = self.proxy.addr();
        let error = self.proxy.lookup_error().map(str::to_owned);
        self.trackers.set_proxy(tracker_proxy, addr, error, &self.resolver, self.poll.registry(), now);
        self.sync_udp_proxy();
        let wants_assoc = self.proxy.cfg.as_ref().is_some_and(|c| c.kind.is_socks5()) && addr.is_some();
        if wants_assoc && self.proxy.assoc.is_none() {
            self.start_udp_associate(now);
        }
    }

    /// Copies the UDP policy onto the listen sockets; the relay goes on the associate's socket.
    fn sync_udp_proxy(&mut self) {
        let policy = self.proxy.udp_policy();
        let (relay, local) = (self.proxy.relay, self.proxy.assoc_local);
        for u in &mut self.udp {
            u.proxy = policy;
            if relay.is_some() && Some(u.local) == local {
                u.proxy.relay = relay;
            }
        }
        self.trackers.set_udp_proxied(
            policy.on && policy.trackers,
            matches!(udp_route(policy, &self.udp, UdpKind::Tracker), UdpRoute::Refuse),
        );
    }

    /// The UDP listen sockets were reopened: they get the policy, and the associate starts over
    /// if its socket went away.
    pub(super) fn proxy_udp_sockets_changed(&mut self) {
        if let Some(local) = self.proxy.assoc_local {
            if !self.udp.iter().any(|u| u.local == local) {
                self.close_udp_associate();
            }
        }
        self.sync_proxy();
    }

    /// The policy for a UDP listen socket opened now.
    pub(super) fn udp_proxy_policy(&self) -> UdpProxy {
        self.proxy.udp_policy()
    }

    /// libtorrent `socks5::start` / `on_name_lookup`: connects to the proxy from the address of
    /// the UDP listen socket of its family.
    fn start_udp_associate(&mut self, now: Instant) {
        let Some(c) = self.proxy.cfg.clone().filter(|c| c.kind.is_socks5()) else { return };
        let Some(proxy) = self.proxy.addr() else { return };
        let Some(u) = self.udp.iter().find(|u| u.v4 == proxy.is_ipv4()) else {
            // only set up a SOCKS5 tunnel for sockets with the same address family as the proxy
            if !self.udp.is_empty() {
                self.proxy_alert(ProxyOperation::HostnameLookup, "No route to host".into());
            }
            self.proxy.assoc = None;
            return;
        };
        let local = u.local;
        let target = if c.udp_send_local_ep {
            Target::Addr(local)
        } else {
            Target::Addr(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0))
        };
        let tunnel =
            Box::new(Tunnel::new(Handshake::socks5(Socks5Command::UdpAssociate, &target, c.creds())));
        let res = TcpStream::connect(proxy).and_then(|mut sock| {
            self.poll.registry().register(
                &mut sock,
                PROXY_UDP_TOKEN,
                Interest::READABLE | Interest::WRITABLE,
            )?;
            Ok(sock)
        });
        self.proxy.assoc_local = Some(local);
        match res {
            Ok(sock) => {
                self.proxy.assoc = Some(Assoc::Connecting {
                    sock,
                    tunnel,
                    connected: false,
                    deadline: now + CONNECT_TIMEOUT,
                });
            }
            Err(e) => self.udp_associate_failed(ProxyOperation::Connect, e.to_string(), true, now),
        }
    }

    /// Events on the associate's TCP connection.
    pub(super) fn on_proxy_udp_event(&mut self) {
        let now = Instant::now();
        match &mut self.proxy.assoc {
            Some(Assoc::Connecting { sock, tunnel, connected, .. }) => {
                if !*connected {
                    match (sock.take_error(), sock.peer_addr()) {
                        (Ok(None), Ok(_)) => *connected = true,
                        (Ok(None), Err(e)) if e.kind() == io::ErrorKind::NotConnected => return,
                        (Ok(Some(e)), _) | (Err(e), _) | (_, Err(e)) => {
                            return self.udp_associate_failed(
                                ProxyOperation::Connect,
                                e.to_string(),
                                true,
                                now,
                            );
                        }
                    }
                }
                match tunnel.drive(sock) {
                    Ok(None) => {}
                    Ok(Some(bound)) => self.udp_associate_up(bound),
                    Err(e) => {
                        let retry = !e.is_proxy_fault();
                        let msg = match e {
                            TunnelError::Proxy(p) => p.to_string(),
                            TunnelError::Io(e) => e.to_string(),
                            TunnelError::Closed => "connection closed".into(),
                        };
                        self.udp_associate_failed(ProxyOperation::Handshake, msg, retry, now);
                    }
                }
            }
            Some(Assoc::Active { sock }) => {
                // libtorrent `hung_up`: the associate lives as long as this connection; anything
                // on it, or its end, means it is over
                let mut buf = [0u8; 64];
                let error = match sock.read(&mut buf) {
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                    Ok(0) => "connection closed".to_owned(),
                    Ok(_) => "unexpected data".to_owned(),
                    Err(e) => e.to_string(),
                };
                self.udp_associate_failed(ProxyOperation::SockRead, error, true, now);
            }
            _ => {}
        }
    }

    fn udp_associate_up(&mut self, bound: Option<Bound>) {
        let Some(proxy) = self.proxy.addr() else { return };
        let Some(Assoc::Connecting { sock, .. }) = self.proxy.assoc.take() else { return };
        let relay = match bound {
            Some(Bound::Addr(a)) if !a.ip().is_unspecified() => a,
            Some(Bound::Addr(a)) => SocketAddr::new(proxy.ip(), a.port()),
            // libtorrent `read_domainname`: the proxy's address with the given port
            Some(Bound::Host(_, port)) => SocketAddr::new(proxy.ip(), port),
            None => proxy,
        };
        self.proxy.assoc = Some(Assoc::Active { sock });
        self.proxy.relay = Some(relay);
        self.proxy.failures = 0;
        let label = self.proxy.cfg.as_ref().map(ProxyConfig::label).unwrap_or_default();
        self.alert(Alert::Log {
            id: None,
            message: format!("proxy: {label}: UDP associate up, relay {relay}"),
        });
        self.sync_udp_proxy();
        if let Some(i) = self.udp.iter().position(|u| u.proxy.relay.is_some()) {
            for (to, data) in std::mem::take(&mut self.proxy.held) {
                let _ = send_relayed(&self.udp[i], relay, &to, &data, false);
            }
        }
    }

    fn udp_associate_failed(&mut self, op: ProxyOperation, error: String, retry: bool, now: Instant) {
        self.close_udp_associate();
        self.proxy_alert(op, error);
        if retry {
            self.proxy.failures += 1;
            self.proxy.assoc = Some(Assoc::Waiting(now + retry_delay(self.proxy.failures)));
        } else {
            self.proxy.assoc = Some(Assoc::Stopped);
            self.proxy.held.clear();
        }
        self.sync_udp_proxy();
    }

    fn close_udp_associate(&mut self) {
        match self.proxy.assoc.take() {
            Some(Assoc::Connecting { mut sock, .. }) | Some(Assoc::Active { mut sock }) => {
                let _ = self.poll.registry().deregister(&mut sock);
            }
            _ => {}
        }
        if self.proxy.relay.take().is_some() {
            self.sync_udp_proxy();
        }
        self.proxy.assoc_local = None;
    }

    /// Sends the UDP tracker datagrams queued for the proxy. While the associate is not up they
    /// are lost, and the tracker retransmits.
    pub(super) fn flush_tracker_udp(&mut self) {
        let out = self.trackers.take_proxied_out();
        if out.is_empty() {
            return;
        }
        match udp_route(self.proxy.udp_policy(), &self.udp, UdpKind::Tracker) {
            UdpRoute::Relay(i, relay) => {
                for (to, data) in out {
                    let _ = send_relayed(&self.udp[i], relay, &to, &data, false);
                }
            }
            UdpRoute::Drop => {
                for (to, data) in out {
                    self.hold_udp(to, data);
                }
            }
            UdpRoute::Direct | UdpRoute::Refuse => {}
        }
    }

    /// Keeps a datagram for the UDP associate that is being made (the oldest go first).
    pub(super) fn hold_udp(&mut self, to: Target, data: Vec<u8>) {
        if self.proxy.held.len() >= MAX_HELD {
            self.proxy.held.remove(0);
        }
        self.proxy.held.push((to, data));
    }

    /// Timers: a failed lookup is retried, the associate reconnects or times out.
    pub(super) fn proxy_service(&mut self, now: Instant) {
        if matches!(self.proxy.lookup, Lookup::Failed { retry, .. } if retry <= now) {
            self.proxy.lookup = Lookup::Idle;
            self.proxy_lookup(now);
            if self.proxy.addr().is_some() {
                self.sync_proxy();
            }
        }
        match &self.proxy.assoc {
            Some(Assoc::Waiting(at)) if *at <= now => {
                self.proxy.assoc = None;
                self.start_udp_associate(now);
            }
            Some(Assoc::Connecting { deadline, .. }) if *deadline <= now => {
                self.udp_associate_failed(ProxyOperation::Connect, "Connection timed out".into(), true, now);
            }
            _ => {}
        }
    }

    /// libtorrent `socks5_alert`, for the proxy's lookup, the UDP associate and, at most once a
    /// minute per error, peer connections the proxy refused.
    pub(super) fn proxy_alert(&mut self, operation: ProxyOperation, error: String) {
        let proxy = self.proxy.cfg.as_ref().map(ProxyConfig::label).unwrap_or_default();
        self.alert(Alert::Proxy { proxy, operation, error });
    }

    /// A peer or web seed connection failed in the proxy handshake. Failures of the proxy itself
    /// (authentication, protocol) are posted, rate-limited; failures to reach the target are
    /// ordinary connect failures.
    pub(super) fn proxy_connection_failed(&mut self, e: &TunnelError) {
        if !e.is_proxy_fault() {
            return;
        }
        let now = Instant::now();
        let error = match e {
            TunnelError::Proxy(p) => p.to_string(),
            other => other.to_string(),
        };
        if self
            .proxy
            .last_fault
            .as_ref()
            .is_some_and(|(m, at)| *m == error && now < *at + FAULT_ALERT_INTERVAL)
        {
            return;
        }
        self.proxy.last_fault = Some((error.clone(), now));
        self.proxy_alert(ProxyOperation::Handshake, error);
    }

    /// The peer and web seed proxy, if it is usable now. While its name resolves, `Err(None)`;
    /// after the lookup failed, `Err(Some(error))`.
    pub(super) fn peer_proxy(&self) -> Option<Result<(ProxyConfig, SocketAddr), Option<String>>> {
        let c = self.proxy.for_peers()?;
        Some(match (self.proxy.addr(), self.proxy.lookup_error()) {
            (Some(a), _) => Ok((c.clone(), a)),
            (None, e) => Err(e.map(str::to_owned)),
        })
    }

    /// A placeholder address for a connection whose target the proxy resolves: web seeds are
    /// told apart by it (`100::/64` is the discard prefix, never a real peer).
    pub(super) fn proxied_placeholder(seed: u32, port: u16) -> SocketAddr {
        let ip = std::net::Ipv6Addr::new(0x0100, 0, 0, 0, 0, 0, (seed >> 16) as u16, seed as u16);
        SocketAddr::new(IpAddr::V6(ip), port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proxy::UDP_HEADER_V4;

    fn cfg(kind: ProxyType) -> ProxyConfig {
        let s = Settings {
            proxy_type: kind,
            proxy_hostname: "proxy.example".into(),
            proxy_port: 1080,
            proxy_username: "u".into(),
            proxy_password: "p".into(),
            ..Settings::default()
        };
        ProxyConfig::from_settings(&s).unwrap()
    }

    #[test]
    fn config_from_settings() {
        assert_eq!(ProxyConfig::from_settings(&Settings::default()), None);
        assert!(!peer_mode(&Settings::default()));
        let c = cfg(ProxyType::Socks5);
        assert_eq!(c.creds(), None, "plain SOCKS5 never authenticates");
        assert_eq!(cfg(ProxyType::Socks5Pw).creds(), Some(("u", "p")));
        assert_eq!(cfg(ProxyType::HttpPw).creds(), Some(("u", "p")));
        assert_eq!(cfg(ProxyType::Http).creds(), None);
        assert_eq!(c.label(), "socks5://proxy.example:1080");
        assert!(c.resolves_names() && c.peers && c.trackers);
        assert!(cfg(ProxyType::Socks4).resolves_names());
        assert!(!cfg(ProxyType::Http).resolves_names(), "HTTP CONNECT names an address by default");
        assert!(cfg(ProxyType::Http).plain_http(false) && !cfg(ProxyType::Http).plain_http(true));
        assert!(!cfg(ProxyType::Socks5).plain_http(false));
        let mut out = Vec::new();
        cfg(ProxyType::Socks4).tunnel(&Target::new("1.2.3.4", 1)).unwrap().start(&mut out);
        assert_eq!(out[..2], [4, 1]);
        assert!(peer_mode(&Settings { proxy_type: ProxyType::Http, ..Settings::default() }));
        assert!(!peer_mode(&Settings {
            proxy_type: ProxyType::Http,
            proxy_peer_connections: false,
            ..Settings::default()
        }));
    }

    #[test]
    fn retry_backoff() {
        assert_eq!(retry_delay(0), Duration::from_secs(5));
        assert_eq!(retry_delay(1), Duration::from_secs(5));
        assert_eq!(retry_delay(4), Duration::from_secs(13));
        assert_eq!(retry_delay(1000), Duration::from_secs(125));
    }

    /// A blocking-free in-memory socket: reads from `input`, records writes.
    struct Pipe {
        input: Vec<u8>,
        pos: usize,
        written: Vec<u8>,
    }

    impl Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pos == self.input.len() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let n = buf.len().min(self.input.len() - self.pos);
            buf[..n].copy_from_slice(&self.input[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn tunnel_leaves_the_stream_alone() {
        let hs = cfg(ProxyType::Socks5).tunnel(&Target::new("1.2.3.4", 6881)).unwrap();
        let mut t = Tunnel::new(hs);
        let mut p = Pipe { input: vec![5, 0], pos: 0, written: Vec::new() };
        assert!(t.drive(&mut p).unwrap().is_none());
        assert_eq!(p.written, [5, 1, 0, 5, 1, 0, 1, 1, 2, 3, 4, 0x1a, 0xe1]);
        p.input.extend_from_slice(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        p.input.extend_from_slice(b"\x13BitTorrent protocol");
        assert_eq!(t.drive(&mut p).unwrap(), Some(Some(Bound::Addr("0.0.0.0:0".parse().unwrap()))));
        assert_eq!(&p.input[p.pos..p.pos + 1], b"\x13", "the peer's handshake is still unread");

        let mut t = Tunnel::new(cfg(ProxyType::Socks5Pw).tunnel(&Target::new("1.2.3.4", 1)).unwrap());
        let mut p = Pipe { input: vec![5, 2, 1, 1], pos: 0, written: Vec::new() };
        let e = t.drive(&mut p).unwrap_err();
        assert!(e.is_proxy_fault());
        assert_eq!(e.to_string(), "proxy: SOCKS authentication error");
    }

    #[test]
    fn udp_routes() {
        let none = UdpProxy::default();
        assert_eq!(udp_route(none, &[], UdpKind::Dht), UdpRoute::Direct);
        let socks = UdpProxy { on: true, peers: true, trackers: false, socks5: true, relay: None };
        assert_eq!(udp_route(socks, &[], UdpKind::Peer), UdpRoute::Drop);
        assert_eq!(udp_route(socks, &[], UdpKind::Dht), UdpRoute::Drop);
        assert_eq!(udp_route(socks, &[], UdpKind::Tracker), UdpRoute::Direct);
        let http = UdpProxy { socks5: false, ..socks };
        assert_eq!(udp_route(http, &[], UdpKind::Peer), UdpRoute::Refuse);

        let relay: SocketAddr = "10.0.0.1:1080".parse().unwrap();
        let from_relay = |data: &[u8]| {
            let mut pkt = Vec::new();
            udp_wrap(&Target::new("1.2.3.4", 6881), data, &mut pkt);
            pkt
        };
        let with_relay = UdpProxy { relay: Some(relay), ..socks };
        let pkt = from_relay(b"d1:ae");
        assert_eq!(pkt.len(), UDP_HEADER_V4 + 5);
        assert_eq!(
            udp_incoming(&with_relay, relay, &pkt),
            UdpIn::Proxied(UdpSource::Addr("1.2.3.4:6881".parse().unwrap()), &b"d1:ae"[..])
        );
        assert_eq!(udp_incoming(&with_relay, "10.0.0.2:1080".parse().unwrap(), &pkt), UdpIn::Drop);
        assert_eq!(udp_incoming(&with_relay, relay, b"short"), UdpIn::Drop);
        // peers proxied, trackers not: direct datagrams still pass until the relay is up
        assert_eq!(udp_incoming(&socks, relay, b"x"), UdpIn::Direct);
        let all = UdpProxy { trackers: true, ..socks };
        assert_eq!(udp_incoming(&all, relay, b"x"), UdpIn::Drop);
        assert_eq!(udp_incoming(&none, relay, b"x"), UdpIn::Direct);
    }
}
