// Ported from libtorrent-rasterbar 2.0.11: src/lsd.cpp (lsd::start, announce_impl,
// resend_announce, on_announce), src/session_impl.cpp (start_lsd, stop_lsd, on_lsd_announce,
// announce_lsd, on_lsd_peer, expand_unspecified_address), src/torrent.cpp (lsd_announce)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: libtorrent runs one LSD socket per listen socket, after expanding wildcard listen
// addresses into every local address. We do the same expansion but open one socket per
// interface and family (an interface's other addresses only widen the subnet filter), and the
// IPv6 socket joins the group on that interface's index rather than the address's scope id
// (0 for global addresses, which means "any"). One session-wide resend timer stands in for
// the per-socket ones, which all fired together anyway. The announce round-robin covers
// running torrents only, as the DHT's does. After a network change the sockets are reopened
// when the set of interfaces they would cover changed (libtorrent: per listen socket).

//! Local service discovery (BEP 14): multicast announces of running torrents to the local
//! network, and peers from other clients' announces.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::time::{Duration, Instant};

use mio::net::UdpSocket;
use mio::{Interest, Token};

use super::Engine;
use crate::alert::Alert;
use crate::net::enum_net::{IpInterface, match_addr_mask};
use crate::net::sys;
use crate::params::TorrentFlags;
use crate::proto::lsd;
use crate::status::PeerSource;
use crate::types::{Sha1Hash, TorrentId};

pub(crate) const LSD_TOKEN_BASE: usize = 13;
pub(crate) const MAX_LSD_SOCKETS: usize = 32;
/// libtorrent `lsd::start`: `hops(32)`.
const MULTICAST_HOPS: u32 = 32;
/// An announce goes out three times, 2 s and then 4 s apart (libtorrent `announce_impl`).
const SENDS_PER_ANNOUNCE: u32 = 3;

struct LsdSocket {
    sock: UdpSocket,
    /// The interface's index.
    index: u32,
    v4: bool,
    /// The address shown in alerts.
    local: IpAddr,
    /// The interface's addresses, netmasks and IPv6 scope ids: datagrams from outside these
    /// subnets are ignored (libtorrent `match_addr_mask` against the listen socket's netmask).
    nets: Vec<(IpAddr, IpAddr, u32)>,
    /// A send failed: libtorrent's `m_disabled`.
    disabled: bool,
}

/// The LSD part of the engine.
pub(super) struct LsdSession {
    socks: Vec<LsdSocket>,
    /// Marks our own announces, which multicast loopback brings back.
    cookie: u32,
    next_torrent: usize,
    next_announce: Option<Instant>,
    /// The last announce's next send: when, the info-hash, the port and how many went out.
    resend: Option<(Instant, Sha1Hash, u16, u32)>,
    buf: Vec<u8>,
}

impl LsdSession {
    pub fn new() -> LsdSession {
        LsdSession {
            socks: Vec::new(),
            cookie: (crate::util::random::random_u64() as u32) & 0x7fff_ffff,
            next_torrent: 0,
            next_announce: None,
            resend: None,
            buf: vec![0; 1500],
        }
    }

    pub fn is_running(&self) -> bool {
        self.socks.iter().any(|s| !s.disabled)
    }

    /// When the reactor must wake for LSD next.
    pub fn deadline(&self) -> Option<Instant> {
        if self.socks.is_empty() {
            return None;
        }
        let resend = self.resend.map(|r| r.0);
        match (self.next_announce, resend) {
            (Some(a), Some(r)) => Some(a.min(r)),
            (a, r) => a.or(r),
        }
    }
}

/// One LSD socket to open: an interface's index, family and addresses.
#[derive(Debug, PartialEq)]
struct Endpoint {
    index: u32,
    v4: bool,
    nets: Vec<(IpAddr, IpAddr, u32)>,
}

/// libtorrent `expand_unspecified_address` for LSD: a wildcard listen address stands for every
/// up, preferred address of its family; a specific one for itself. Addresses are then grouped
/// by interface and family.
fn lsd_endpoints(listen: &[IpAddr], ifs: &[IpInterface]) -> Vec<Endpoint> {
    let mut out: Vec<Endpoint> = Vec::new();
    let mut add = |ifc: &IpInterface| {
        let net = (ifc.addr, ifc.netmask, ifc.scope_id);
        let v4 = ifc.addr.is_ipv4();
        match out.iter_mut().find(|e| e.index == ifc.index && e.v4 == v4) {
            Some(e) if !e.nets.contains(&net) => e.nets.push(net),
            Some(_) => {}
            None => out.push(Endpoint { index: ifc.index, v4, nets: vec![net] }),
        }
    };
    for ip in listen {
        if ip.is_unspecified() {
            for ifc in ifs.iter().filter(|i| i.addr.is_ipv4() == ip.is_ipv4()) {
                // interfaces that cannot multicast (VPN tunnels) would only fail to join
                if ifc.up && ifc.preferred && (ifc.multicast || ifc.loopback) {
                    add(ifc);
                }
            }
        } else {
            match ifs.iter().find(|i| i.addr == *ip) {
                Some(ifc) => add(ifc),
                None => add(&IpInterface {
                    addr: *ip,
                    netmask: crate::net::enum_net::build_netmask(
                        if ip.is_ipv4() { 32 } else { 128 },
                        ip.is_ipv4(),
                    ),
                    scope_id: 0,
                    index: 0,
                    name: String::new(),
                    up: true,
                    loopback: ip.is_loopback(),
                    multicast: true,
                    pointopoint: false,
                    preferred: true,
                }),
            }
        }
    }
    out
}

/// libtorrent `lsd::start`: a socket on the LSD port, shared with other clients, in the group on
/// this interface, sending through it with loopback on.
fn open_socket(ep: &Endpoint) -> io::Result<std::net::UdpSocket> {
    let any: IpAddr = if ep.v4 { Ipv4Addr::UNSPECIFIED.into() } else { Ipv6Addr::UNSPECIFIED.into() };
    let s = sys::udp_socket(SocketAddr::new(any, lsd::PORT), true)?;
    match ep.nets[0].0 {
        IpAddr::V4(local) => {
            s.join_multicast_v4(&lsd::MULTICAST_V4, &local)?;
            s.set_multicast_ttl_v4(MULTICAST_HOPS)?;
            s.set_multicast_loop_v4(true)?;
            sys::set_multicast_if_v4(&s, local)?;
        }
        IpAddr::V6(_) => {
            s.join_multicast_v6(&lsd::MULTICAST_V6, ep.index)?;
            sys::set_multicast_hops_v6(&s, MULTICAST_HOPS)?;
            s.set_multicast_loop_v6(true)?;
            if ep.index != 0 {
                sys::set_multicast_if_v6(&s, ep.index)?;
            }
        }
    }
    s.set_nonblocking(true)?;
    Ok(s)
}

/// Whether a datagram from `from` belongs to one of the socket's subnets.
fn in_network(nets: &[(IpAddr, IpAddr, u32)], from: SocketAddr) -> bool {
    let scope = match from {
        SocketAddr::V6(a) => a.scope_id(),
        SocketAddr::V4(_) => 0,
    };
    nets.iter().any(|&(addr, mask, s)| {
        // link-local subnets repeat on every interface: the scope tells them apart
        let link_local = matches!(addr, IpAddr::V6(a) if a.segments()[0] & 0xffc0 == 0xfe80);
        match_addr_mask(from.ip(), addr, mask) && (!link_local || s == scope)
    })
}

impl Engine {
    /// The listen addresses LSD covers: the bound listen sockets' (a wildcard stands for
    /// every address of its family).
    fn lsd_listen_addresses(&self) -> Vec<IpAddr> {
        self.listeners.iter().map(|l| l.local.ip()).collect()
    }

    /// libtorrent `start_lsd`: one socket per interface the listen sockets cover.
    pub(super) fn start_lsd(&mut self, now: Instant) {
        if !self.settings.enable_lsd || !self.lsd.socks.is_empty() {
            return;
        }
        let listen = self.lsd_listen_addresses();
        if listen.is_empty() {
            return;
        }
        let ifs = match self.net_interfaces() {
            Ok(ifs) => ifs,
            Err(e) => {
                self.alert(Alert::Log {
                    id: None,
                    message: format!("LSD: cannot list the network interfaces: {e}"),
                });
                return;
            }
        };
        for ep in lsd_endpoints(&listen, &ifs).into_iter().take(MAX_LSD_SOCKETS) {
            let local = ep.nets[0].0;
            let token = Token(LSD_TOKEN_BASE + self.lsd.socks.len());
            let res = open_socket(&ep).and_then(|s| {
                let mut s = UdpSocket::from_std(s);
                self.poll.registry().register(&mut s, token, Interest::READABLE)?;
                Ok(s)
            });
            match res {
                Ok(sock) => self.lsd.socks.push(LsdSocket {
                    sock,
                    index: ep.index,
                    v4: ep.v4,
                    local,
                    nets: ep.nets,
                    disabled: false,
                }),
                Err(e) => self.alert(Alert::LsdError { address: local, error: e.into() }),
            }
        }
        if self.lsd.socks.is_empty() {
            return;
        }
        let n = self.lsd.socks.len();
        self.alert(Alert::Log { id: None, message: format!("LSD started on {n} interfaces") });
        self.lsd.next_announce = Some(now + self.lsd_interval());
        // when LSD is switched on at runtime, running torrents announce now rather than waiting
        // for the round-robin
        let running: Vec<TorrentId> = self
            .torrent_ids()
            .into_iter()
            .filter(|&id| self.torrent_ref(id).is_some_and(|t| t.is_active()))
            .collect();
        for id in running {
            self.lsd_announce(id, now);
        }
    }

    /// After a network change or a listen-socket reopen (libtorrent `update_lsd`, which runs
    /// per listen socket): when the interfaces LSD would cover differ from the ones it has
    /// sockets on, the sockets are reopened, and the running torrents announce at once.
    pub(super) fn update_lsd(&mut self, ifs: &[IpInterface], now: Instant) {
        if !self.settings.enable_lsd {
            return;
        }
        let mut want = lsd_endpoints(&self.lsd_listen_addresses(), ifs);
        want.truncate(MAX_LSD_SOCKETS);
        let have: Vec<Endpoint> = self
            .lsd
            .socks
            .iter()
            .map(|s| Endpoint { index: s.index, v4: s.v4, nets: s.nets.clone() })
            .collect();
        // a socket that failed to open is retried on the next change
        if want == have {
            return;
        }
        self.stop_lsd();
        self.start_lsd(now);
    }

    /// libtorrent `stop_lsd`.
    pub(super) fn stop_lsd(&mut self) {
        for mut s in self.lsd.socks.drain(..) {
            let _ = self.poll.registry().deregister(&mut s.sock);
        }
        self.lsd.next_announce = None;
        self.lsd.resend = None;
    }

    /// libtorrent `on_lsd_announce`'s delay: the interval spread over the running torrents.
    fn lsd_interval(&self) -> Duration {
        let running = self
            .torrent_ids()
            .iter()
            .filter(|&&id| self.torrent_ref(id).is_some_and(|t| t.is_active()))
            .count();
        let secs = (self.settings.local_service_announce_interval as u64 / running.max(1) as u64).max(1);
        Duration::from_secs(secs)
    }

    /// Runs the announce round-robin and the resends.
    pub(super) fn lsd_service(&mut self, now: Instant) {
        if self.lsd.next_announce.is_some_and(|a| now >= a) {
            self.lsd.next_announce = Some(now + self.lsd_interval());
            let running: Vec<TorrentId> = self
                .torrent_ids()
                .into_iter()
                .filter(|&id| self.torrent_ref(id).is_some_and(|t| t.is_active()))
                .collect();
            if !running.is_empty() {
                if self.lsd.next_torrent >= running.len() {
                    self.lsd.next_torrent = 0;
                }
                let id = running[self.lsd.next_torrent];
                self.lsd.next_torrent += 1;
                self.lsd_announce(id, now);
            }
        }
        if let Some((at, ih, port, sent)) = self.lsd.resend {
            if now >= at {
                self.lsd_send(ih, port, sent, now);
            }
        }
    }

    /// libtorrent `torrent::lsd_announce`.
    pub(super) fn lsd_announce(&mut self, id: TorrentId, now: Instant) {
        if !self.lsd.is_running() || self.listen_port == 0 {
            return;
        }
        let Some(t) = self.torrent_ref(id) else { return };
        if t.flags.contains(TorrentFlags::DISABLE_LSD) || !self.is_running(t) || !t.is_active() {
            return;
        }
        // a torrent with metadata announces once its files are checked; a magnet right away
        if t.has_metadata() && (t.checking || !t.checked) && t.fetch.is_none() {
            return;
        }
        // private torrents are never announced on LSD
        if t.ti.as_ref().is_some_and(|ti| ti.is_private()) {
            return;
        }
        let ih = t.info_hashes.best_v1_compatible();
        self.lsd_send(ih, self.listen_port, 0, now);
    }

    /// libtorrent `lsd::announce_impl`: one packet per socket, then a resend 2 s and 4 s later.
    /// A newer announce replaces the pending resends, as libtorrent's single timer does.
    fn lsd_send(&mut self, ih: Sha1Hash, port: u16, sent: u32, now: Instant) {
        let cookie = self.lsd.cookie;
        let mut failed = Vec::new();
        for s in self.lsd.socks.iter_mut().filter(|s| !s.disabled) {
            let (group, v4): (IpAddr, bool) =
                if s.v4 { (lsd::MULTICAST_V4.into(), true) } else { (lsd::MULTICAST_V6.into(), false) };
            let packet = lsd::render(port, &ih, cookie, v4);
            match s.sock.send_to(&packet, SocketAddr::new(group, lsd::PORT)) {
                Ok(_) => {}
                // a full send buffer drops the datagram, as UDP may anyway
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    s.disabled = true;
                    failed.push((s.local, e));
                }
            }
        }
        for (address, e) in failed {
            self.alert(Alert::LsdError { address, error: e.into() });
        }
        let sent = sent + 1;
        self.lsd.resend = (sent < SENDS_PER_ANNOUNCE && self.lsd.is_running())
            .then(|| (now + Duration::from_secs(2 * sent as u64), ih, port, sent));
    }

    /// libtorrent `lsd::on_announce`: reads announces from an LSD socket.
    pub(super) fn on_lsd_readable(&mut self, index: usize) {
        loop {
            let Some(s) = self.lsd.socks.get(index) else { return };
            let (n, from) = match s.sock.recv_from(&mut self.lsd.buf) {
                Ok(r) => r,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if matches!(e.kind(), io::ErrorKind::Interrupted | io::ErrorKind::ConnectionReset) => {
                    continue;
                }
                Err(_) => break,
            };
            // other networks are not ours to answer (Linux hands every socket on the port the
            // datagrams of every interface)
            if !in_network(&s.nets, from) {
                continue;
            }
            let Ok(msg) = lsd::parse(&self.lsd.buf[..n]) else { continue };
            if msg.cookie == Some(self.lsd.cookie as i64) {
                continue;
            }
            let peer = match from {
                SocketAddr::V6(a) => SocketAddr::V6(SocketAddrV6::new(*a.ip(), msg.port, 0, a.scope_id())),
                SocketAddr::V4(a) => SocketAddr::new((*a.ip()).into(), msg.port),
            };
            for ih in msg.info_hashes {
                self.on_lsd_peer(peer, ih);
            }
        }
    }

    /// libtorrent `session_impl::on_lsd_peer`.
    fn on_lsd_peer(&mut self, peer: SocketAddr, ih: Sha1Hash) {
        let Some(&id) = self.by_hash.get(&ih) else { return };
        let Some(t) = self.torrent(id) else { return };
        // no LSD peers for private torrents
        if !t.is_active() || t.ti.as_ref().is_some_and(|ti| ti.is_private()) {
            return;
        }
        self.add_peers(id, &[peer], PeerSource::LSD);
        self.alert(Alert::LsdPeer { id, address: peer });
        self.connect_peers_for(id, 10);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ifc(addr: &str, bits: u32, index: u32, loopback: bool) -> IpInterface {
        let addr: IpAddr = addr.parse().unwrap();
        IpInterface {
            addr,
            netmask: crate::net::enum_net::build_netmask(bits, addr.is_ipv4()),
            scope_id: if addr.to_string().starts_with("fe80") { index } else { 0 },
            index,
            name: format!("if{index}"),
            up: true,
            loopback,
            multicast: !loopback,
            pointopoint: false,
            preferred: true,
        }
    }

    #[test]
    fn endpoints_from_listen_addresses() {
        let ifs = [
            ifc("127.0.0.1", 8, 1, true),
            ifc("::1", 128, 1, true),
            ifc("192.168.1.5", 24, 7, false),
            ifc("10.0.0.2", 8, 7, false),
            ifc("fe80::1", 64, 7, false),
            ifc("2001:db8::5", 64, 7, false),
            IpInterface { up: false, ..ifc("172.16.0.1", 16, 9, false) },
            IpInterface { multicast: false, ..ifc("100.64.0.1", 10, 10, false) },
        ];
        let any4: IpAddr = Ipv4Addr::UNSPECIFIED.into();
        let eps = lsd_endpoints(&[any4], &ifs);
        assert_eq!(eps.len(), 2, "{eps:?}");
        assert_eq!(eps[0].index, 1);
        assert_eq!(eps[1].nets.len(), 2, "both addresses of one interface share a socket");
        let any6: IpAddr = Ipv6Addr::UNSPECIFIED.into();
        let eps = lsd_endpoints(&[any4, any6], &ifs);
        assert_eq!(eps.len(), 4);
        assert!(eps.iter().any(|e| !e.v4 && e.index == 7 && e.nets.len() == 2));
        // a specific address stands for itself, even one the interface list lacks
        let eps = lsd_endpoints(&["127.0.0.1".parse().unwrap(), "10.9.9.9".parse().unwrap()], &ifs);
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].nets, [ifs[0].clone()].map(|i| (i.addr, i.netmask, i.scope_id)));
        assert_eq!(eps[1].nets[0].1, "255.255.255.255".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn subnet_filter() {
        let nets = [
            ("192.168.1.5".parse().unwrap(), "255.255.255.0".parse().unwrap(), 0),
            ("fe80::1".parse().unwrap(), "ffff:ffff:ffff:ffff::".parse().unwrap(), 7),
        ];
        assert!(in_network(&nets, "192.168.1.200:6771".parse().unwrap()));
        assert!(!in_network(&nets, "192.168.2.1:6771".parse().unwrap()));
        assert!(in_network(&nets, "[fe80::9%7]:6771".parse().unwrap()));
        assert!(!in_network(&nets, "[fe80::9%8]:6771".parse().unwrap()), "same prefix, other link");
        assert!(!in_network(&nets, "[2001:db8::1]:6771".parse().unwrap()));
    }
}
