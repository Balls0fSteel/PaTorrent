// Ported from libtorrent-rasterbar 2.0.11: src/session_impl.cpp (start_natpmp, start_upnp,
// stop_natpmp, stop_upnp, remap_ports / map_port, on_port_mapping, update_upnp,
// update_natpmp), include/libtorrent/aux_/portmap.hpp (portmap_action, portmap_callback)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: libtorrent runs one mapper pair per listen socket; here a wildcard listen address
// is expanded into the IPv4 interfaces that have a default route (UPnP alone on every
// multicast-capable one where the routing table cannot be read). A mapper that is being
// closed stays in a `closing` list until its delete requests are answered.

//! Port forwarding through the router: a NAT-PMP / PCP and a UPnP mapper per interface, each
//! asked to forward the TCP listen port and the DHT's UDP port.

use std::net::{IpAddr, Ipv4Addr};
use std::time::Instant;

use mio::Token;

use super::Engine;
use super::natpmp::NatPmp;
use super::upnp::{Upnp, UpnpSettings};
use crate::alert::{Alert, PortmapTransport, Protocol};
use crate::net::enum_net::{IpInterface, get_gateway};
use crate::net::ip_voter::is_local;
use crate::net::sys;
use crate::proto::natpmp;

/// Reactor tokens of the mappers' UDP sockets, never reused within a session.
pub(crate) const PORTMAP_UDP_TOKEN_BASE: usize = 0x4000_0000;
/// Reactor tokens of UPnP's HTTP connections: a block of `HTTP_TOKENS_PER_MAPPER` per mapper.
pub(crate) const UPNP_HTTP_TOKEN_BASE: usize = 0x3000_0000;
const HTTP_TOKENS_PER_MAPPER: usize = 64;

/// libtorrent `portmap_action`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    None,
    Add,
    Del,
}

/// libtorrent `portmap_callback::on_port_mapping`: a mapping was added (`error` `None`, a
/// port), deleted (`error` `None`, port 0) or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortmapEvent {
    pub mapping: usize,
    pub external_ip: Option<IpAddr>,
    pub external_port: u16,
    pub protocol: Protocol,
    pub error: Option<String>,
}

const NATPMP: usize = 0;
const UPNP: usize = 1;
const TRANSPORTS: [PortmapTransport; 2] = [PortmapTransport::NatPmp, PortmapTransport::Upnp];

/// libtorrent `listen_socket_t`'s mapper half: the two mappers of one interface and the
/// handles of the session's mappings on each.
struct Mapper {
    iface: IpInterface,
    gateway: Option<IpAddr>,
    natpmp: Option<NatPmp>,
    natpmp_token: usize,
    upnp: Option<Upnp>,
    upnp_tokens: (usize, usize),
    /// Per transport: the TCP and UDP mapping handles (libtorrent `tcp_port_mapping`,
    /// `udp_port_mapping`).
    tcp: [Option<usize>; 2],
    udp: [Option<usize>; 2],
}

pub(super) struct PortmapSession {
    mappers: Vec<Mapper>,
    /// UPnP mappers whose delete requests are still in flight.
    closing: Vec<Upnp>,
    next_udp_token: usize,
    next_http_block: usize,
}

impl PortmapSession {
    pub fn new() -> PortmapSession {
        PortmapSession {
            mappers: Vec::new(),
            closing: Vec::new(),
            next_udp_token: PORTMAP_UDP_TOKEN_BASE,
            next_http_block: UPNP_HTTP_TOKEN_BASE,
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        let mut d: Option<Instant> = None;
        let mut take = |t: Option<Instant>| {
            d = match (d, t) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        };
        for m in &self.mappers {
            take(m.natpmp.as_ref().and_then(NatPmp::deadline));
            take(m.upnp.as_ref().and_then(Upnp::deadline));
        }
        for u in &self.closing {
            take(u.deadline());
        }
        d
    }

    /// Delete requests still to be answered: the engine's shutdown waits for them.
    pub fn pending(&self) -> bool {
        self.closing.iter().any(Upnp::pending)
            || self.mappers.iter().any(|m| m.upnp.as_ref().is_some_and(Upnp::pending))
    }

    fn udp_token(&mut self) -> usize {
        let t = self.next_udp_token;
        self.next_udp_token += 1;
        t
    }

    fn http_block(&mut self) -> usize {
        let t = self.next_http_block;
        self.next_http_block += HTTP_TOKENS_PER_MAPPER;
        t
    }
}

impl Engine {
    fn upnp_settings(&self) -> UpnpSettings {
        UpnpSettings {
            ignore_nonrouters: self.settings.upnp_ignore_nonrouters,
            lease_duration: self.settings.upnp_lease_duration,
            user_agent: self.settings.user_agent.clone(),
        }
    }

    /// The interfaces to map ports on: the IPv4 interfaces the listen sockets cover that lead
    /// to the internet (a default route through them; every multicast-capable one when the
    /// routing table cannot be read), never loopback.
    fn portmap_interfaces(&mut self) -> Vec<(IpInterface, Option<IpAddr>)> {
        let listen: Vec<Ipv4Addr> = self
            .listeners
            .iter()
            .filter_map(|l| l.local_addr().ok())
            .filter_map(|a| match a.ip() {
                IpAddr::V4(v4) => Some(v4),
                IpAddr::V6(_) => None,
            })
            .collect();
        if listen.is_empty() {
            return Vec::new();
        }
        let ifs = match sys::interfaces() {
            Ok(ifs) => ifs,
            Err(e) => {
                self.alert(Alert::Log {
                    id: None,
                    message: format!("port mapping: cannot list the network interfaces: {e}"),
                });
                return Vec::new();
            }
        };
        let routes = match sys::routes() {
            Ok(r) => Some(r),
            Err(e) => {
                self.alert(Alert::Log {
                    id: None,
                    message: format!("port mapping: cannot read the routing table ({e}); NAT-PMP is off"),
                });
                None
            }
        };
        let mut out: Vec<(IpInterface, Option<IpAddr>)> = Vec::new();
        let mut add = |ifc: &IpInterface, wildcard: bool| {
            if ifc.loopback || !ifc.addr.is_ipv4() || out.iter().any(|(i, _)| i.addr == ifc.addr) {
                return;
            }
            let gateway = routes.as_ref().and_then(|r| get_gateway(ifc, r));
            // a wildcard listen address stands for the interfaces that reach the internet
            let internet = match &routes {
                Some(_) => gateway.is_some(),
                None => ifc.multicast,
            };
            if wildcard && !internet {
                return;
            }
            out.push((ifc.clone(), gateway));
        };
        for ip in listen {
            if ip.is_unspecified() {
                for ifc in ifs.iter().filter(|i| i.up && i.preferred) {
                    add(ifc, true);
                }
            } else if let Some(ifc) = ifs.iter().find(|i| i.addr == IpAddr::V4(ip)) {
                add(ifc, false);
            }
        }
        out
    }

    /// libtorrent `reopen_listen_sockets`' mapper part: mappers for the listen interfaces, then
    /// the transports the settings enable.
    pub(super) fn start_portmap(&mut self) {
        if !self.settings.enable_natpmp && !self.settings.enable_upnp {
            return;
        }
        if self.portmap.mappers.is_empty() {
            for (iface, gateway) in self.portmap_interfaces() {
                let natpmp_token = self.portmap.udp_token();
                let upnp_tokens = (self.portmap.udp_token(), self.portmap.udp_token());
                self.portmap.mappers.push(Mapper {
                    iface,
                    gateway,
                    natpmp: None,
                    natpmp_token,
                    upnp: None,
                    upnp_tokens,
                    tcp: [None; 2],
                    udp: [None; 2],
                });
            }
        }
        if self.settings.enable_natpmp {
            self.start_natpmp();
        }
        if self.settings.enable_upnp {
            self.start_upnp();
        }
    }

    /// libtorrent `start_natpmp`.
    pub(super) fn start_natpmp(&mut self) {
        let now = Instant::now();
        for mi in 0..self.portmap.mappers.len() {
            let m = &mut self.portmap.mappers[mi];
            if m.natpmp.is_some() {
                continue;
            }
            let Some(gateway) = m.gateway else {
                let message = format!("no default gateway on {}; not started", m.iface.addr);
                self.alert(Alert::PortmapLog { transport: PortmapTransport::NatPmp, message });
                continue;
            };
            let m = &mut self.portmap.mappers[mi];
            let mut n = NatPmp::new();
            n.start(self.poll.registry(), Token(m.natpmp_token), m.iface.addr, gateway, natpmp::PORT, now);
            m.natpmp = Some(n);
            self.remap_ports(mi, NATPMP, now);
        }
        self.drain_portmap();
    }

    /// libtorrent `start_upnp` (IPv4 interfaces only, as there).
    pub(super) fn start_upnp(&mut self) {
        let now = Instant::now();
        let settings = self.upnp_settings();
        for mi in 0..self.portmap.mappers.len() {
            if self.portmap.mappers[mi].upnp.is_some() {
                continue;
            }
            let block = self.portmap.http_block();
            let m = &mut self.portmap.mappers[mi];
            let (IpAddr::V4(addr), IpAddr::V4(mask)) = (m.iface.addr, m.iface.netmask) else { continue };
            let mut u = Upnp::new(settings.clone(), addr, mask, block);
            u.start(self.poll.registry(), Token(m.upnp_tokens.0), Token(m.upnp_tokens.1), now);
            m.upnp = Some(u);
            self.remap_ports(mi, UPNP, now);
        }
        self.drain_portmap();
    }

    /// Hands changed UPnP settings (lease, subnet filter, description) to the running mappers.
    pub(super) fn update_upnp_settings(&mut self) {
        let settings = self.upnp_settings();
        for u in self.portmap.mappers.iter_mut().filter_map(|m| m.upnp.as_mut()) {
            u.set_settings(settings.clone());
        }
    }

    /// libtorrent `stop_natpmp`: the delete requests go out at once.
    pub(super) fn stop_natpmp(&mut self) {
        let now = Instant::now();
        for m in &mut self.portmap.mappers {
            m.tcp[NATPMP] = None;
            m.udp[NATPMP] = None;
            if let Some(mut n) = m.natpmp.take() {
                n.close(now);
                self.out.extend(
                    n.log
                        .drain(..)
                        .map(|message| Alert::PortmapLog { transport: PortmapTransport::NatPmp, message }),
                );
            }
        }
    }

    /// libtorrent `stop_upnp`: the mapper lives on until its delete requests are answered.
    pub(super) fn stop_upnp(&mut self) {
        let now = Instant::now();
        for m in &mut self.portmap.mappers {
            m.tcp[UPNP] = None;
            m.udp[UPNP] = None;
            if let Some(mut u) = m.upnp.take() {
                u.close(self.poll.registry(), now);
                if u.pending() {
                    self.portmap.closing.push(u);
                } else {
                    self.out.extend(
                        u.log
                            .drain(..)
                            .map(|message| Alert::PortmapLog { transport: PortmapTransport::Upnp, message }),
                    );
                }
            }
        }
        self.drain_portmap();
    }

    /// At shutdown: delete every mapping (libtorrent `session_impl::abort`).
    pub(super) fn close_portmap(&mut self) {
        self.stop_natpmp();
        self.stop_upnp();
    }

    /// libtorrent `remap_ports` / `map_port` for one mapper and transport: the TCP listen port
    /// and the DHT's UDP port, with the same external port numbers.
    fn remap_ports(&mut self, mi: usize, transport: usize, now: Instant) {
        let tcp_port = self.listen_port;
        let udp_port = self
            .dht
            .socks
            .iter()
            .find(|(v4, _)| *v4)
            .and_then(|(_, s)| s.local_addr().ok())
            .map(|a| a.port())
            .unwrap_or(0);
        let registry = self.poll.registry();
        let m = &mut self.portmap.mappers[mi];
        for (proto, port, handle) in
            [(Protocol::Tcp, tcp_port, &mut m.tcp), (Protocol::Udp, udp_port, &mut m.udp)]
        {
            if transport == NATPMP {
                let Some(n) = m.natpmp.as_mut() else { continue };
                if let Some(h) = handle[NATPMP].take() {
                    n.delete_mapping(h, now);
                }
                // only update this mapping if we actually have a socket listening
                if port != 0 {
                    handle[NATPMP] = n.add_mapping(proto, port, port, now);
                }
            } else {
                let Some(u) = m.upnp.as_mut() else { continue };
                if let Some(h) = handle[UPNP].take() {
                    u.delete_mapping(h, registry, now);
                }
                if port != 0 {
                    handle[UPNP] = u.add_mapping(proto, port, port, registry, now);
                }
            }
        }
    }

    /// A reactor event on one of the mappers' sockets.
    pub(super) fn on_portmap_event(&mut self, token: usize) {
        let now = Instant::now();
        let registry = self.poll.registry();
        if token >= PORTMAP_UDP_TOKEN_BASE {
            for m in &mut self.portmap.mappers {
                if token == m.natpmp_token {
                    if let Some(n) = m.natpmp.as_mut() {
                        n.on_readable(now);
                    }
                } else if token == m.upnp_tokens.0 || token == m.upnp_tokens.1 {
                    if let Some(u) = m.upnp.as_mut() {
                        u.on_readable(token == m.upnp_tokens.0, registry, now);
                    }
                }
            }
        } else {
            let upnps = self
                .portmap
                .mappers
                .iter_mut()
                .filter_map(|m| m.upnp.as_mut())
                .chain(self.portmap.closing.iter_mut());
            for u in upnps {
                if u.owns_http_token(token) {
                    u.on_http_event(token - u.http_token_base(), registry, now);
                    break;
                }
            }
        }
        self.drain_portmap();
    }

    /// Runs the mappers' timers and hands their events on.
    pub(super) fn portmap_service(&mut self, now: Instant) {
        let registry = self.poll.registry();
        for m in &mut self.portmap.mappers {
            if let Some(n) = m.natpmp.as_mut() {
                n.service(now);
            }
            if let Some(u) = m.upnp.as_mut() {
                u.service(registry, now);
            }
        }
        for u in &mut self.portmap.closing {
            u.service(registry, now);
        }
        self.drain_portmap();
    }

    /// Turns the mappers' queued events and log lines into alerts.
    fn drain_portmap(&mut self) {
        let mut out = Vec::new();
        for (mi, m) in self.portmap.mappers.iter_mut().enumerate() {
            if let Some(n) = m.natpmp.as_mut() {
                out.extend(n.log.drain(..).map(|l| (mi, NATPMP, Err(l))));
                out.extend(n.events.drain(..).map(|e| (mi, NATPMP, Ok(e))));
            }
            if let Some(u) = m.upnp.as_mut() {
                out.extend(u.log.drain(..).map(|l| (mi, UPNP, Err(l))));
                out.extend(u.events.drain(..).map(|e| (mi, UPNP, Ok(e))));
            }
        }
        for u in &mut self.portmap.closing {
            out.extend(u.log.drain(..).map(|l| (usize::MAX, UPNP, Err(l))));
            out.extend(u.events.drain(..).map(|e| (usize::MAX, UPNP, Ok(e))));
        }
        self.portmap.closing.retain(Upnp::pending);
        for (mi, transport, item) in out {
            match item {
                Err(message) => self.alert(Alert::PortmapLog { transport: TRANSPORTS[transport], message }),
                Ok(e) => self.on_port_mapping(mi, transport, e),
            }
        }
    }

    /// libtorrent `session_impl::on_port_mapping`.
    fn on_port_mapping(&mut self, mi: usize, transport: usize, e: PortmapEvent) {
        let t = TRANSPORTS[transport];
        if let Some(message) = e.error {
            self.alert(Alert::PortmapFailed { transport: t, message });
            return;
        }
        // the router's idea of our external address (libtorrent casts a `source_router` vote;
        // trackers see the real public address, so theirs wins when it comes)
        if let Some(ip) = e.external_ip.filter(|ip| !ip.is_unspecified() && !is_local(ip)) {
            if self.external_ip.is_none() {
                self.external_ip = Some(ip);
                self.alert(Alert::ExternalIp { ip });
            }
        }
        if e.external_port == 0 {
            // a deleted mapping
            self.alert(Alert::PortmapLog {
                transport: t,
                message: format!("{:?} mapping {} removed", e.protocol, e.mapping),
            });
            return;
        }
        if let Some(m) = self.portmap.mappers.get(mi) {
            let ours = match e.protocol {
                Protocol::Tcp => m.tcp[transport] == Some(e.mapping),
                Protocol::Udp => m.udp[transport] == Some(e.mapping),
            };
            if !ours {
                return;
            }
        }
        self.alert(Alert::PortmapSucceeded {
            transport: t,
            protocol: e.protocol,
            external_port: e.external_port,
        });
    }
}
