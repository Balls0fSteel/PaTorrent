// Ported from libtorrent-rasterbar 2.0.11: src/session_impl.cpp (reopen_listen_sockets,
// reopen_network_sockets, setup_listener, update_listen_interfaces, on_ip_change,
// start_ip_notifier), include/libtorrent/aux_/session_impl.hpp (listen_socket_t)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a wildcard listen address is bound as it is (a TCP listener and a UDP socket per
// family, IPv6 v6-only) rather than as one listen socket per interface address; libtorrent's
// expanded list is computed beside it and drives LSD, port mapping, the local addresses and the
// reaction to network changes. Device names and specific addresses are bound per address, as
// there, but all listen sockets share one port and each family has one UDP socket (uTP and the
// DHT), on the wildcard if there is one. Votes stay per family; a family's voter starts over
// when one of its internet-facing addresses goes away (libtorrent drops that socket's voter).
// A change notification is acted on a second later (changes come in bursts), and only if the
// interfaces or default routes differ from the last read; TCP connections whose local address
// went away are closed.

//! Listen sockets, and following network changes.

use std::io;
use std::net::{IpAddr, SocketAddr, SocketAddrV6};
use std::time::{Duration, Instant};

use mio::net::{TcpListener, UdpSocket};
use mio::{Interest, Token};

use super::peer::Transport;
use super::{Engine, UDP4_LISTEN_TOKEN, UDP6_LISTEN_TOKEN, UdpListen};
use crate::alert::{Alert, Protocol};
use crate::error::Error;
use crate::net::enum_net::{IpInterface, IpRoute};
use crate::net::ip_notifier::{IpNotifier, Wake};
use crate::net::listen::{
    ListenEndpoint, listen_plan, parse_listen_interfaces, partition_listen_sockets, print_listen_interfaces,
};
use crate::net::sys;
use crate::settings::Settings;

/// Reactor tokens of the TCP listeners, never reused within a session.
pub(super) const LISTEN_TOKEN_BASE: usize = 0x0800_0000;
/// The network-change notifier's socket (Linux, FreeBSD, OpenBSD).
pub(super) const IP_NOTIFIER_TOKEN: Token = Token(1);
/// At most this many bound listen sockets (a device can have many IPv6 addresses).
const MAX_LISTEN_SOCKETS: usize = 32;
/// Changes come in bursts (an address, its routes, IPv6 duplicate address detection): act once
/// they settle.
const IP_CHANGE_DELAY: Duration = Duration::from_secs(1);
/// Where there is no notifier, the interfaces are read this often.
const RESCAN_INTERVAL: Duration = Duration::from_secs(30);

/// libtorrent `listen_socket_t`: a bound listen address, with the endpoint it was opened for.
pub(super) struct ListenSocket {
    /// The endpoint (configured port, device, flags) that `partition_listen_sockets` matches.
    pub ep: ListenEndpoint,
    pub tcp: Option<TcpListener>,
    pub token: Token,
    /// The bound address: the TCP listener's, else the UDP socket's.
    pub local: SocketAddr,
    /// Whether the family's UDP socket (`Engine::udp`) belongs to this listen socket.
    pub udp: bool,
}

/// What the interfaces and default routes looked like at the last read.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NetSnapshot {
    ifs: Vec<IpInterface>,
    gateways: Vec<IpRoute>,
}

impl NetSnapshot {
    fn new(ifs: &[IpInterface], routes: &[IpRoute]) -> NetSnapshot {
        let mut ifs = ifs.to_vec();
        ifs.sort_by_key(|a| (a.index, a.addr));
        // only default routes matter (to the port mappers); the rest of the table churns
        let mut gateways: Vec<IpRoute> =
            routes.iter().filter(|r| r.destination.is_unspecified()).cloned().collect();
        gateways.sort_by_key(|a| (a.index, a.gateway, a.source_hint));
        gateways.dedup();
        NetSnapshot { ifs, gateways }
    }
}

/// The engine's listen-socket and network-change state.
pub(super) struct NetworkState {
    wake: Wake,
    notifier: Option<IpNotifier>,
    /// A change was notified: when to act on it.
    change_due: Option<Instant>,
    /// Without a notifier: when to read the interfaces again.
    next_rescan: Option<Instant>,
    snapshot: Option<NetSnapshot>,
    /// libtorrent's expanded listen endpoints, as of the last reopen.
    pub expanded: Vec<ListenEndpoint>,
    next_token: usize,
    /// The configured port the listen port was picked for.
    port_for: Option<u16>,
    /// Interfaces and routes a test pretends the OS reported.
    #[cfg(test)]
    pub fake: Option<(Vec<IpInterface>, Vec<IpRoute>)>,
}

impl NetworkState {
    pub fn new(wake: Wake) -> NetworkState {
        NetworkState {
            wake,
            notifier: None,
            change_due: None,
            next_rescan: None,
            snapshot: None,
            expanded: Vec::new(),
            next_token: LISTEN_TOKEN_BASE,
            port_for: None,
            #[cfg(test)]
            fake: None,
        }
    }

    /// When the reactor must wake for a pending change or re-read.
    pub fn deadline(&self) -> Option<Instant> {
        match (self.change_due, self.next_rescan) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Whether a settings change touches the listen sockets: `Some(true)` when they must all be
/// reopened (TCP or UDP was switched on or off), `Some(false)` when `listen_interfaces`
/// changed (libtorrent `update_listen_interfaces`).
pub(super) fn listen_settings_changed(old: &Settings, new: &Settings) -> Option<bool> {
    let wants = |s: &Settings| {
        (s.enable_incoming_tcp, s.enable_dht || s.enable_incoming_utp || s.enable_outgoing_utp)
    };
    if wants(old) != wants(new) {
        Some(true)
    } else if old.listen_interfaces != new.listen_interfaces {
        Some(false)
    } else {
        None
    }
}

/// The address to bind for an endpoint: a link-local IPv6 address needs its interface's scope.
fn bind_addr(ep: &ListenEndpoint, port: u16, ifs: &[IpInterface]) -> SocketAddr {
    match ep.addr {
        IpAddr::V6(a) => {
            let scope = ifs
                .iter()
                .find(|i| i.addr == ep.addr && (ep.device.is_empty() || i.name == ep.device))
                .map_or(0, |i| i.scope_id);
            SocketAddr::V6(SocketAddrV6::new(a, port, 0, scope))
        }
        IpAddr::V4(_) => SocketAddr::new(ep.addr, port),
    }
}

/// The addresses of the internet-facing endpoints of a family.
fn internet_addresses(eps: &[ListenEndpoint], v4: bool) -> Vec<IpAddr> {
    let mut v: Vec<IpAddr> =
        eps.iter().filter(|e| e.addr.is_ipv4() == v4 && !e.is_local_network()).map(|e| e.addr).collect();
    v.sort();
    v.dedup();
    v
}

/// Whether a connection's local address is gone from the interfaces (loopback never goes).
fn local_address_gone(local: IpAddr, ifs: &[IpInterface]) -> bool {
    let local = local.to_canonical();
    !(local.is_loopback() || local.is_unspecified() || ifs.iter().any(|i| i.addr == local))
}

impl Engine {
    /// The local interfaces (`net::sys::interfaces`, or a test's pretend ones).
    pub(super) fn net_interfaces(&self) -> io::Result<Vec<IpInterface>> {
        #[cfg(test)]
        if let Some((ifs, _)) = &self.network.fake {
            return Ok(ifs.clone());
        }
        sys::interfaces()
    }

    /// The routing table (`net::sys::routes`, or a test's pretend one).
    pub(super) fn net_routes(&self) -> io::Result<Vec<IpRoute>> {
        #[cfg(test)]
        if let Some((_, routes)) = &self.network.fake {
            return Ok(routes.clone());
        }
        sys::routes()
    }

    /// Interfaces and routes for a reopen; what cannot be read is empty (libtorrent posts a
    /// `listen_failed_alert` for `enum_if` / `enum_route` and goes on).
    fn read_network(&mut self) -> (Vec<IpInterface>, Vec<IpRoute>) {
        let ifs = self.net_interfaces().unwrap_or_else(|e| {
            self.out
                .push(Alert::Log { id: None, message: format!("cannot list the network interfaces: {e}") });
            Vec::new()
        });
        let routes = self.net_routes().unwrap_or_else(|e| {
            if e.kind() != io::ErrorKind::Unsupported {
                self.out
                    .push(Alert::Log { id: None, message: format!("cannot read the routing table: {e}") });
            }
            Vec::new()
        });
        (ifs, routes)
    }

    fn listen_token(&mut self) -> Token {
        let t = self.network.next_token;
        self.network.next_token += 1;
        Token(t)
    }

    /// At startup: the listen sockets for the current interfaces.
    pub(super) fn open_listen_sockets(&mut self) {
        let (ifs, routes) = self.read_network();
        self.network.snapshot = Some(NetSnapshot::new(&ifs, &routes));
        self.reopen_with(&ifs, &routes, true);
    }

    /// `listen_interfaces` (or TCP / UDP being switched on or off) changed: libtorrent
    /// `update_listen_interfaces` and `reopen_listen_sockets`. Sockets that still match an
    /// endpoint stay open; with `close_all` every socket is reopened.
    pub(super) fn on_listen_settings_changed(&mut self, close_all: bool) {
        let now = Instant::now();
        if close_all {
            for l in std::mem::take(&mut self.listeners) {
                self.close_listen_socket(l);
            }
        }
        let (ifs, routes) = self.read_network();
        self.network.snapshot = Some(NetSnapshot::new(&ifs, &routes));
        let old_expanded = self.network.expanded.clone();
        let old_port = self.listen_port;
        self.reopen_with(&ifs, &routes, true);
        self.after_listen_change(&old_expanded, old_port, &ifs, false, now);
    }

    /// libtorrent `start_ip_notifier`; without one, the interfaces are read periodically.
    pub(super) fn start_ip_notifier(&mut self, now: Instant) {
        let res = IpNotifier::new(self.network.wake.clone()).and_then(|mut n| {
            n.register(self.poll.registry(), IP_NOTIFIER_TOKEN)?;
            Ok(n)
        });
        match res {
            Ok(n) => self.network.notifier = Some(n),
            Err(e) => {
                let secs = RESCAN_INTERVAL.as_secs();
                self.alert(Alert::Log {
                    id: None,
                    message: format!(
                        "no network change notifications ({e}); reading the interfaces every {secs} s"
                    ),
                });
                self.network.next_rescan = Some(now + RESCAN_INTERVAL);
            }
        }
    }

    /// libtorrent `stop_ip_notifier`, at shutdown.
    pub(super) fn stop_ip_notifier(&mut self) {
        if let Some(mut n) = self.network.notifier.take() {
            n.deregister(self.poll.registry());
        }
        self.network.next_rescan = None;
        self.network.change_due = None;
    }

    /// The notifier's socket is readable.
    pub(super) fn on_ip_notifier_readable(&mut self) {
        if self.network.notifier.as_mut().is_some_and(IpNotifier::on_readable) {
            self.ip_change_notified(Instant::now());
        }
    }

    fn ip_change_notified(&mut self, now: Instant) {
        if self.network.change_due.is_none() {
            self.network.change_due = Some(now + IP_CHANGE_DELAY);
        }
    }

    /// Acts on notified changes once they settled, and on the periodic re-read.
    pub(super) fn network_service(&mut self, now: Instant) {
        if self.network.notifier.as_ref().is_some_and(IpNotifier::take_signal) {
            self.ip_change_notified(now);
        }
        if self.network.change_due.is_some_and(|d| now >= d) {
            self.network.change_due = None;
            self.on_ip_change(now, false);
        }
        if self.network.next_rescan.is_some_and(|d| now >= d) {
            self.network.next_rescan = Some(now + RESCAN_INTERVAL);
            self.on_ip_change(now, false);
        }
    }

    /// libtorrent `on_ip_change` / `reopen_network_sockets`: the interfaces are read again and,
    /// if they changed (or `force`: `SessionHandle::reopen_network_sockets`, which also asks
    /// the routers for every mapping again), the listen sockets, LSD, the port mappers, the
    /// votes and the announces follow.
    pub(super) fn on_ip_change(&mut self, now: Instant, force: bool) {
        let (ifs, routes) = self.read_network();
        let snapshot = NetSnapshot::new(&ifs, &routes);
        if !force && self.network.snapshot.as_ref() == Some(&snapshot) {
            return;
        }
        self.network.snapshot = Some(snapshot);
        self.alert(Alert::Log { id: None, message: "network change: reopening the listen sockets".into() });
        let old_expanded = self.network.expanded.clone();
        let old_port = self.listen_port;
        self.reopen_with(&ifs, &routes, false);
        self.after_listen_change(&old_expanded, old_port, &ifs, force, now);
        self.close_stale_connections(&ifs);
    }

    /// libtorrent `reopen_listen_sockets`: the endpoints for these interfaces; sockets that
    /// match one stay, the others close, and the remaining endpoints are opened. With
    /// `report_config`, unusable `listen_interfaces` entries are reported.
    fn reopen_with(&mut self, ifs: &[IpInterface], routes: &[IpRoute], report_config: bool) {
        let want_tcp = self.settings.enable_incoming_tcp;
        let want_udp = self.settings.enable_dht
            || self.settings.enable_incoming_utp
            || self.settings.enable_outgoing_utp;
        let mut err = Vec::new();
        let ifaces = parse_listen_interfaces(&self.settings.listen_interfaces, &mut err);
        if report_config {
            let message = format!("listen interfaces: {}", print_listen_interfaces(&ifaces));
            self.out.push(Alert::Log { id: None, message });
            for e in err {
                self.alert(Alert::ListenFailed {
                    address: e,
                    protocol: Protocol::Tcp,
                    error: io::Error::new(io::ErrorKind::InvalidInput, "invalid listen interface").into(),
                });
            }
            // no SSL torrents, so no SSL listen sockets (libtorrent without TORRENT_USE_SSL)
            for i in ifaces.iter().filter(|i| i.ssl) {
                self.alert(Alert::ListenFailed {
                    address: format!("{}:{}", i.device, i.port),
                    protocol: Protocol::Tcp,
                    error: Error::Unsupported("SSL listen sockets"),
                });
            }
        }
        let plan = listen_plan(&ifaces, ifs, routes);
        self.network.expanded = plan.expanded;
        let mut all = if want_tcp || want_udp { plan.bind } else { Vec::new() };
        all.truncate(MAX_LISTEN_SOCKETS);

        // first remove the sockets that are not needed, so a new one can't fail to bind on a
        // conflict with a stale one
        let mut eps = all.clone();
        let keep = partition_listen_sockets(&mut eps, &mut self.listeners, |l| &l.ep);
        let removed: Vec<ListenSocket> = self.listeners.drain(keep..).collect();
        for l in removed {
            self.close_listen_socket(l);
        }
        // with every socket replaced, a new port is picked unless the first endpoint asks for
        // the same one as before (a device coming back keeps its port); with nothing to bind
        // the port is remembered for later
        if self.listeners.is_empty() && all.first().is_some_and(|e| Some(e.port) != self.network.port_for) {
            self.listen_port = 0;
        }

        // the UDP socket of each family: on the wildcard, else on the first internet-facing
        // endpoint, else on the first
        let mut udp_for: [Option<ListenEndpoint>; 2] = [None, None];
        let mut udp_opened = [false; 2];
        for v4 in [true, false] {
            if !want_udp || self.udp.iter().any(|u| u.v4 == v4) {
                continue;
            }
            let pick = all
                .iter()
                .filter(|e| e.addr.is_ipv4() == v4)
                .min_by_key(|e| (!e.addr.is_unspecified(), e.is_local_network()));
            let Some(pick) = pick else { continue };
            match self.listeners.iter().position(|l| &l.ep == pick) {
                Some(i) => udp_opened[!v4 as usize] = self.open_udp_for(i, ifs),
                None => udp_for[!v4 as usize] = Some(pick.clone()),
            }
        }
        for ep in eps {
            let fam = !ep.addr.is_ipv4() as usize;
            let udp = want_udp && udp_for[fam].as_ref() == Some(&ep);
            if !want_tcp && !udp {
                continue;
            }
            if self.open_listen_socket(ep, want_tcp, udp, ifs) && udp {
                udp_opened[fam] = self.udp.iter().any(|u| u.v4 == (fam == 0));
            }
        }
        if self.listeners.is_empty() && !all.is_empty() {
            self.alert(Alert::Log { id: None, message: "giving up on binding listen sockets".into() });
        }
        self.update_external_ip();
        for (fam, opened) in udp_opened.into_iter().enumerate() {
            if opened {
                self.dht_socket_opened(fam == 0);
            }
        }
    }

    /// libtorrent `setup_listener`: binds the TCP listener and (with `want_udp`) the family's
    /// UDP socket on the same port. The first socket picks the port: when the configured one
    /// is busy for either protocol the next ones are tried (`max_retry_port_bind`), and an
    /// ephemeral port is drawn again. Later sockets listen on that port. Whether it bound.
    fn open_listen_socket(
        &mut self,
        ep: ListenEndpoint,
        want_tcp: bool,
        want_udp: bool,
        ifs: &[IpInterface],
    ) -> bool {
        let first = self.listen_port == 0;
        // every socket was replaced, but the port is kept: if it is taken now, start over
        let reuse = !first && self.listeners.is_empty();
        let retries = self.settings.max_retry_port_bind as usize;
        let configured = ep.port;
        let tries = if !first {
            1
        } else if configured != 0 {
            retries + 1
        } else {
            retries.min(3) + 1
        };
        let v4 = ep.addr.is_ipv4();
        let mut last_err: Option<(Protocol, io::Error)> = None;
        let mut bound: Option<ListenSocket> = None;
        for i in 0..tries {
            let port = if !first {
                self.listen_port
            } else if configured != 0 {
                configured.wrapping_add(i as u16)
            } else {
                0
            };
            let a = bind_addr(&ep, port, ifs);
            // TCP first: with port 0 it is the one that picks the port
            let tcp = match want_tcp.then(|| sys::tcp_listener(a)) {
                Some(Ok(l)) => Some(l),
                Some(Err(e)) => {
                    last_err = Some((Protocol::Tcp, e));
                    continue;
                }
                None => None,
            };
            let port = tcp.as_ref().and_then(|l| l.local_addr().ok()).map_or(port, |l| l.port());
            let udp = if want_udp {
                match sys::udp_socket(bind_addr(&ep, port, ifs), false) {
                    Ok(u) => Some(u),
                    Err(e) => {
                        last_err = Some((Protocol::Udp, e));
                        // free for TCP but taken for UDP: the next port, for both
                        if first && i + 1 < tries {
                            continue;
                        }
                        None
                    }
                }
            } else {
                None
            };
            let mut local = None;
            let mut listener = None;
            let mut token = Token(usize::MAX);
            if let Some(l) = tcp {
                let t = self.listen_token();
                let res = l.set_nonblocking(true).and_then(|()| {
                    let mut l = TcpListener::from_std(l);
                    self.poll.registry().register(&mut l, t, Interest::READABLE)?;
                    Ok(l)
                });
                match res {
                    Ok(l) => {
                        let addr = l.local_addr().unwrap_or(a);
                        self.out.push(Alert::ListenSucceeded { address: addr, protocol: Protocol::Tcp });
                        local = Some(addr);
                        listener = Some(l);
                        token = t;
                    }
                    Err(e) => last_err = Some((Protocol::Tcp, e)),
                }
            }
            let mut has_udp = false;
            if let Some(u) = udp {
                match self.register_udp(u, v4) {
                    Ok(addr) => {
                        local.get_or_insert(addr);
                        has_udp = true;
                    }
                    Err(e) => last_err = Some((Protocol::Udp, e)),
                }
            }
            if let Some(local) = local {
                bound = Some(ListenSocket { ep: ep.clone(), tcp: listener, token, local, udp: has_udp });
                break;
            }
        }
        let addr_str = SocketAddr::new(ep.addr, configured).to_string();
        let Some(sock) = bound else {
            if reuse {
                // the old port is taken: pick a new one
                self.listen_port = 0;
                return self.open_listen_socket(ep, want_tcp, want_udp, ifs);
            }
            let (protocol, error) = match last_err {
                Some((p, e)) => (p, e.into()),
                None => (Protocol::Tcp, Error::Unsupported("listen")),
            };
            self.out.push(Alert::ListenFailed { address: addr_str, protocol, error });
            return false;
        };
        // a failure of one protocol is reported even when the other one bound
        if let Some((Protocol::Udp, e)) = last_err.filter(|_| want_udp && !sock.udp) {
            self.out.push(Alert::ListenFailed {
                address: addr_str,
                protocol: Protocol::Udp,
                error: e.into(),
            });
        }
        if self.listen_port == 0 {
            self.listen_port = sock.local.port();
            self.network.port_for = Some(configured);
        }
        self.listeners.push(sock);
        true
    }

    /// Registers a UDP listen socket of a family with the reactor (it is shared by uTP and the
    /// DHT). Returns its address.
    fn register_udp(&mut self, u: std::net::UdpSocket, v4: bool) -> io::Result<SocketAddr> {
        let token = if v4 { UDP4_LISTEN_TOKEN } else { UDP6_LISTEN_TOKEN };
        u.set_nonblocking(true)?;
        let mut u = UdpSocket::from_std(u);
        self.poll.registry().register(&mut u, token, Interest::READABLE | Interest::WRITABLE)?;
        let local = u.local_addr()?;
        self.out.push(Alert::ListenSucceeded { address: local, protocol: Protocol::Udp });
        self.udp.push(UdpListen { v4, sock: u, local });
        Ok(local)
    }

    /// A kept listen socket takes over its family's UDP socket (the one that had it closed).
    fn open_udp_for(&mut self, i: usize, ifs: &[IpInterface]) -> bool {
        let (ep, port) = (self.listeners[i].ep.clone(), self.listeners[i].local.port());
        let res = sys::udp_socket(bind_addr(&ep, port, ifs), false)
            .and_then(|u| self.register_udp(u, ep.addr.is_ipv4()));
        match res {
            Ok(_) => {
                self.listeners[i].udp = true;
                true
            }
            Err(e) => {
                let address = SocketAddr::new(ep.addr, port).to_string();
                self.out.push(Alert::ListenFailed { address, protocol: Protocol::Udp, error: e.into() });
                false
            }
        }
    }

    fn close_listen_socket(&mut self, l: ListenSocket) {
        let message = format!("closing listen socket for {}", l.ep);
        self.out.push(Alert::Log { id: None, message });
        if let Some(mut t) = l.tcp {
            let _ = self.poll.registry().deregister(&mut t);
        }
        if l.udp {
            if let Some(i) = self.udp.iter().position(|u| u.v4 == l.local.is_ipv4()) {
                let mut u = self.udp.remove(i);
                let _ = self.poll.registry().deregister(&mut u.sock);
            }
        }
    }

    /// What follows a reopen: votes of a family whose internet-facing addresses lost one are
    /// forgotten, LSD and the port mappers follow the interfaces, and if the internet-facing
    /// addresses or the port changed, every running torrent announces again.
    fn after_listen_change(
        &mut self,
        old_expanded: &[ListenEndpoint],
        old_port: u16,
        ifs: &[IpInterface],
        remap: bool,
        now: Instant,
    ) {
        let port_changed = old_port != self.listen_port;
        let mut internet_changed = port_changed;
        for v4 in [true, false] {
            let before = internet_addresses(old_expanded, v4);
            let after = internet_addresses(&self.network.expanded, v4);
            if before != after {
                internet_changed = true;
            }
            if before.iter().any(|a| !after.contains(a)) && self.ip_voter.external_address(v4).is_some() {
                self.ip_voter.reset(v4, now);
                let family = if v4 { "IPv4" } else { "IPv6" };
                self.alert(Alert::Log {
                    id: None,
                    message: format!("network change: forgetting the {family} external address votes"),
                });
            }
        }
        self.update_external_ip();
        self.update_lsd(ifs, now);
        self.update_portmap(port_changed || remap);
        if internet_changed {
            self.reannounce_all();
        }
    }

    /// Every running torrent announces to its trackers (libtorrent `enable_all_trackers` plus
    /// the announces of new listen sockets) and the DHT.
    fn reannounce_all(&mut self) {
        let ids = self.torrent_ids();
        for id in ids {
            let Some(t) = self.torrent(id) else { continue };
            if !t.is_active() {
                continue;
            }
            for tr in &mut t.trackers {
                tr.next_announce = None;
                tr.fails = 0;
            }
        }
        self.dht_reannounce_all();
    }

    /// TCP connections whose local address is gone can carry nothing any more: they are closed
    /// now rather than at their timeouts. The others are kept.
    fn close_stale_connections(&mut self, ifs: &[IpInterface]) {
        for (_, p) in self.peers.iter_mut() {
            let Transport::Tcp(s) = &p.sock else { continue };
            let Ok(local) = s.local_addr() else { continue };
            if local_address_gone(local.ip(), ifs) {
                p.close("the local address went away", false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::enum_net::build_netmask;
    use crate::net::listen::ListenFlags;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn ifc(addr: &str, name: &str, index: u32) -> IpInterface {
        let a = ip(addr);
        IpInterface {
            addr: a,
            netmask: build_netmask(if a.is_ipv4() { 24 } else { 64 }, a.is_ipv4()),
            scope_id: if addr.starts_with("fe80") { index } else { 0 },
            index,
            name: name.into(),
            up: true,
            loopback: false,
            multicast: true,
            pointopoint: false,
            preferred: true,
        }
    }

    fn route(dst: &str, gw: &str, index: u32) -> IpRoute {
        let d = ip(dst);
        IpRoute {
            destination: d,
            netmask: build_netmask(0, d.is_ipv4()),
            gateway: ip(gw),
            source_hint: None,
            index,
        }
    }

    #[test]
    fn snapshots_ignore_order_and_non_default_routes() {
        let a = [ifc("192.168.1.2", "eth0", 2), ifc("10.0.0.2", "tun0", 5)];
        let b = [ifc("10.0.0.2", "tun0", 5), ifc("192.168.1.2", "eth0", 2)];
        let r1 = [route("0.0.0.0", "192.168.1.1", 2), route("192.168.1.0", "0.0.0.0", 2)];
        let r2 = [route("0.0.0.0", "192.168.1.1", 2), route("2001:db8::", "::", 2)];
        assert_eq!(NetSnapshot::new(&a, &r1), NetSnapshot::new(&b, &r2));
        // a new default route, an address going away, a flag changing: all differ
        let r3 = [route("0.0.0.0", "10.0.0.1", 5)];
        assert_ne!(NetSnapshot::new(&a, &r1), NetSnapshot::new(&a, &r3));
        assert_ne!(NetSnapshot::new(&a, &r1), NetSnapshot::new(&a[..1], &r1));
        let down = [IpInterface { up: false, ..a[0].clone() }, a[1].clone()];
        assert_ne!(NetSnapshot::new(&a, &r1), NetSnapshot::new(&down, &r1));
    }

    #[test]
    fn link_local_endpoints_bind_with_their_scope() {
        let ifs = [ifc("fe80::1", "eth0", 7), ifc("2001:db8::1", "eth0", 7)];
        let ep =
            |a: &str, dev: &str| ListenEndpoint::new(ip(a), 6881, dev, false, ListenFlags::ACCEPT_INCOMING);
        assert_eq!(bind_addr(&ep("fe80::1", "eth0"), 5, &ifs), "[fe80::1%7]:5".parse().unwrap());
        assert_eq!(bind_addr(&ep("2001:db8::1", ""), 5, &ifs), "[2001:db8::1]:5".parse().unwrap());
        assert_eq!(bind_addr(&ep("10.0.0.1", ""), 5, &ifs), "10.0.0.1:5".parse().unwrap());
    }

    #[test]
    fn internet_facing_addresses() {
        let f = ListenFlags::ACCEPT_INCOMING | ListenFlags::WAS_EXPANDED;
        let eps = [
            ListenEndpoint::new(ip("192.168.1.2"), 1, "eth0", false, f),
            ListenEndpoint::new(ip("172.17.0.1"), 1, "docker0", false, f | ListenFlags::LOCAL_NETWORK),
            ListenEndpoint::new(ip("2001:db8::1"), 1, "eth0", false, f),
        ];
        assert_eq!(internet_addresses(&eps, true), [ip("192.168.1.2")]);
        assert_eq!(internet_addresses(&eps, false), [ip("2001:db8::1")]);
    }

    #[test]
    fn stale_local_addresses() {
        let ifs = [ifc("192.168.1.2", "eth0", 2), ifc("2001:db8::1", "eth0", 2)];
        assert!(!local_address_gone(ip("192.168.1.2"), &ifs));
        assert!(!local_address_gone(ip("::ffff:192.168.1.2"), &ifs));
        assert!(!local_address_gone(ip("2001:db8::1"), &ifs));
        assert!(!local_address_gone(ip("127.0.0.1"), &ifs), "loopback never goes away");
        assert!(!local_address_gone(ip("::1"), &ifs));
        assert!(local_address_gone(ip("10.0.0.7"), &ifs));
        assert!(local_address_gone(ip("2001:db8::2"), &ifs));
    }

    #[test]
    fn settings_that_reopen() {
        let base = Settings::default();
        assert_eq!(listen_settings_changed(&base, &base.clone()), None);
        let s = Settings { listen_interfaces: "0.0.0.0:7000".into(), ..base.clone() };
        assert_eq!(listen_settings_changed(&base, &s), Some(false));
        let s = Settings { enable_incoming_tcp: !base.enable_incoming_tcp, ..base.clone() };
        assert_eq!(listen_settings_changed(&base, &s), Some(true));
        // UDP stays wanted while any of its users is on
        let s = Settings { enable_dht: !base.enable_dht, ..base.clone() };
        assert_eq!(listen_settings_changed(&base, &s), None);
    }
}

/// A real session whose network interfaces a test makes up (`SessionHandle::set_fake_network`).
#[cfg(test)]
mod session_tests {
    use std::io::{Read, Write};
    use std::net::{IpAddr, SocketAddr, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    use crate::alert::{Alert, Protocol};
    use crate::bencode::Entry;
    use crate::crypto::Sha1;
    use crate::net::enum_net::{IpInterface, IpRoute, build_netmask};
    use crate::params::{AddTorrentParams, TorrentFlags, TorrentSource};
    use crate::session::{Session, SessionHandle, SessionParams};
    use crate::settings::Settings;

    fn start(listen: &str) -> Session {
        let settings = Settings {
            listen_interfaces: listen.into(),
            tick_interval: 100,
            aio_threads: 2,
            enable_dht: false,
            enable_lsd: false,
            enable_upnp: false,
            enable_natpmp: false,
            ..Settings::default()
        };
        Session::start(SessionParams { settings, state_dir: None }).expect("session starts")
    }

    thread_local! {
        /// Alerts popped but not looked at yet (a wait stops at the one it wants).
        static PENDING: std::cell::RefCell<std::collections::VecDeque<Alert>> = Default::default();
    }

    /// Takes alerts until `f` returns `Some`, failing after `secs`; later alerts stay queued.
    fn wait<T>(h: &SessionHandle, secs: u64, what: &str, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut alerts = Vec::new();
        let mut seen = Vec::new();
        loop {
            h.pop_alerts(&mut alerts);
            PENDING.with_borrow_mut(|p| p.extend(alerts.drain(..)));
            while let Some(a) = PENDING.with_borrow_mut(|p| p.pop_front()) {
                if !matches!(a, Alert::StateUpdate(_)) {
                    seen.push(a.message());
                }
                if let Some(v) = f(&a) {
                    return v;
                }
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for {what}; alerts: {seen:#?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn log_containing(text: &str) -> impl FnMut(&Alert) -> Option<()> + '_ {
        move |a| match a {
            Alert::Log { message, .. } if message.contains(text) => Some(()),
            _ => None,
        }
    }

    fn listening(protocol: Protocol) -> impl FnMut(&Alert) -> Option<SocketAddr> {
        move |a| match a {
            Alert::ListenSucceeded { address, protocol: p } if *p == protocol => Some(*address),
            _ => None,
        }
    }

    fn ifc(addr: &str, name: &str, index: u32) -> IpInterface {
        let a: IpAddr = addr.parse().unwrap();
        IpInterface {
            addr: a,
            netmask: build_netmask(if a.is_ipv4() { 24 } else { 64 }, a.is_ipv4()),
            scope_id: 0,
            index,
            name: name.into(),
            up: true,
            loopback: false,
            multicast: true,
            pointopoint: false,
            preferred: true,
        }
    }

    fn default_route(gateway: &str, index: u32) -> IpRoute {
        IpRoute {
            destination: "0.0.0.0".parse().unwrap(),
            netmask: "0.0.0.0".parse().unwrap(),
            gateway: gateway.parse().unwrap(),
            source_hint: None,
            index,
        }
    }

    fn refused(addr: SocketAddr) -> bool {
        TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_err()
    }

    #[test]
    fn a_device_listen_socket_follows_its_device() {
        let s = start("fake0:0");
        let h = s.handle();
        // the OS has no "fake0": nothing is bound
        wait(&h, 5, "startup", |a| match a {
            Alert::ListenSucceeded { .. } => panic!("listening without the device: {a:?}"),
            Alert::Log { message, .. } if message.contains("listen interfaces: fake0:0") => Some(()),
            _ => None,
        });

        // the device appears
        let other = ifc("192.0.2.1", "eth9", 2);
        h.set_fake_network(Some((vec![ifc("127.0.0.1", "fake0", 1), other.clone()], Vec::new())));
        let tcp = wait(&h, 5, "TCP listener on fake0", listening(Protocol::Tcp));
        assert_eq!(tcp.ip(), IpAddr::from([127, 0, 0, 1]));
        let udp = wait(&h, 5, "UDP socket on fake0", listening(Protocol::Udp));
        assert_eq!(udp, tcp, "the UDP socket shares the TCP listener's port");
        drop(TcpStream::connect(tcp).expect("the listener accepts"));

        // it gains an IPv6 address (where the host has IPv6 loopback): a second listen socket
        // on the same port, the first one stays
        if std::net::TcpListener::bind("[::1]:0").is_ok() {
            h.set_fake_network(Some((
                vec![ifc("127.0.0.1", "fake0", 1), ifc("::1", "fake0", 1), other.clone()],
                Vec::new(),
            )));
            let v6 = wait(&h, 5, "IPv6 listener on fake0", |a| match a {
                Alert::ListenSucceeded { address, protocol: Protocol::Tcp } => Some(*address),
                Alert::Log { message, .. } if message.contains("closing listen socket") => {
                    panic!("an unchanged socket was closed: {message}")
                }
                _ => None,
            });
            assert_eq!(v6, SocketAddr::new("::1".parse().unwrap(), tcp.port()));
            drop(TcpStream::connect(v6).expect("the IPv6 listener accepts"));
        }

        // the device goes away: its sockets close
        h.set_fake_network(Some((vec![other.clone()], Vec::new())));
        wait(&h, 5, "socket closed", log_containing("closing listen socket for 127.0.0.1:0 on \"fake0\""));
        std::thread::sleep(Duration::from_millis(200));
        assert!(refused(tcp), "the listener is closed");

        // it comes back: same port
        h.set_fake_network(Some((vec![ifc("127.0.0.1", "fake0", 1), other], Vec::new())));
        let again = wait(&h, 5, "listener again", listening(Protocol::Tcp));
        assert_eq!(again, tcp, "a device coming back keeps the listen port");
        s.shutdown();
    }

    #[test]
    fn unchanged_interfaces_change_nothing() {
        let s = start("127.0.0.1:0");
        let h = s.handle();
        let tcp = wait(&h, 5, "listener", listening(Protocol::Tcp));
        wait(&h, 5, "UDP socket", listening(Protocol::Udp));
        // only the loopback interface (as the OS names it) is left: the socket is still wanted
        let lo = crate::net::sys::interfaces()
            .unwrap()
            .into_iter()
            .find(|i| i.addr == IpAddr::from([127, 0, 0, 1]))
            .expect("a loopback interface");
        let net = (vec![lo], Vec::new());
        h.set_fake_network(Some(net.clone()));
        // the same interfaces again: no reopen; a forced one keeps the socket
        h.set_fake_network(Some(net));
        h.reopen_network_sockets();
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut alerts = Vec::new();
        let mut reopens = 0;
        while Instant::now() < deadline {
            h.pop_alerts(&mut alerts);
            PENDING.with_borrow_mut(|p| {
                alerts.splice(0..0, p.drain(..));
            });
            for a in alerts.drain(..) {
                match a {
                    Alert::Log { message, .. } if message.contains("closing listen socket") => {
                        panic!("{message}")
                    }
                    Alert::Log { message, .. } if message.contains("network change") => reopens += 1,
                    Alert::ListenSucceeded { address, .. } => panic!("reopened {address}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // the first change and the forced reopen went through, the repeat did not
        assert_eq!(reopens, 2);
        drop(TcpStream::connect(tcp).expect("still listening"));
        s.shutdown();
    }

    /// A one-file torrent announcing to `announce`.
    fn torrent(announce: &str) -> Vec<u8> {
        let data = vec![7u8; 32 * 1024];
        let mut info = Entry::new_dict();
        *info.key("length") = Entry::Int(data.len() as i64);
        *info.key("name") = Entry::Bytes(b"netchange-test.bin".to_vec());
        *info.key("piece length") = Entry::Int(32 * 1024);
        *info.key("pieces") = Entry::Bytes(Sha1::digest(&data).0.to_vec());
        let mut t = Entry::new_dict();
        *t.key("announce") = Entry::Bytes(announce.as_bytes().to_vec());
        *t.key("info") = info;
        t.bencode()
    }

    #[test]
    fn a_lost_internet_address_resets_the_votes_and_reannounces() {
        // a tracker that tells every announce our address is 203.0.113.7
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/announce", l.local_addr().unwrap());
        let announces = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let counter = announces.clone();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut c) = c else { return };
                let mut buf = vec![0u8; 4096];
                let mut n = 0;
                while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    match c.read(&mut buf[n..]) {
                        Ok(0) | Err(_) => break,
                        Ok(k) => n += k,
                    }
                }
                let announce = buf[..n].starts_with(b"GET /announce");
                let mut body = b"d11:external ip4:".to_vec();
                body.extend_from_slice(&[203, 0, 113, 7]);
                body.extend_from_slice(b"8:intervali1800e5:peers0:e");
                let head =
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = c.write_all(head.as_bytes());
                let _ = c.write_all(&body);
                // scrapes are answered alike, but only announces count
                if announce {
                    counter.fetch_add(1, Ordering::SeqCst);
                    if tx.send(()).is_err() {
                        return;
                    }
                }
            }
        });

        let s = start("0.0.0.0:0");
        let h = s.handle();
        wait(&h, 5, "listener", listening(Protocol::Tcp));
        // a global address with a default route
        let net = |addr: &str| (vec![ifc(addr, "eth0", 2)], vec![default_route("198.51.100.1", 2)]);
        h.set_fake_network(Some(net("198.51.100.7")));
        wait(&h, 5, "first change", log_containing("network change"));

        let dir = std::env::temp_dir().join(format!("lt-netchange-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(torrent(&url)), dir.to_string_lossy());
        p.flags = TorrentFlags::empty();
        h.add_torrent(p);
        let ip = wait(&h, 15, "external IP", |a| match a {
            Alert::ExternalIp { ip } => Some(*ip),
            _ => None,
        });
        assert_eq!(ip, IpAddr::from([203, 0, 113, 7]));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(announces.load(Ordering::SeqCst), 1);

        // the address changes (a new DHCP lease): the old votes are forgotten and the torrent
        // announces again at once (its interval is 30 minutes)
        h.set_fake_network(Some(net("198.51.100.8")));
        wait(&h, 5, "votes reset", log_containing("forgetting the IPv4 external address votes"));
        rx.recv_timeout(Duration::from_secs(10)).expect("a second announce after the change");
        // the reply votes again, on an empty voter
        let ip = wait(&h, 10, "external IP again", |a| match a {
            Alert::ExternalIp { ip } => Some(*ip),
            _ => None,
        });
        assert_eq!(ip, IpAddr::from([203, 0, 113, 7]));

        // a change that keeps the internet-facing address (a second, local-only interface)
        // neither resets nor announces
        let (mut ifs, routes) = net("198.51.100.8");
        ifs.push(ifc("172.17.0.1", "docker0", 3));
        h.set_fake_network(Some((ifs, routes)));
        wait(&h, 5, "third change", log_containing("network change"));
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_err(), "no announce for a local-only change");
        s.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }
}
