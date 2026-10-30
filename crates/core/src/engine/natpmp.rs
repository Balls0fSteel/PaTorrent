// Ported from libtorrent-rasterbar 2.0.11: src/natpmp.cpp, include/libtorrent/natpmp.hpp;
// natpmp_lease_duration from 2.0.13 (455e9da7f1)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the asio timers are deadlines polled by `service`; the callback is a queue of
// `PortmapEvent`s and log lines the engine drains; the gateway is looked up by the caller.

//! A NAT-PMP / PCP port mapper for one interface: speaks PCP to the gateway first and falls
//! back to NAT-PMP, keeps the session's mappings alive and deletes them on close.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use mio::net::UdpSocket;
use mio::{Interest, Registry, Token};

use super::portmap::{Action, PortmapEvent};
use crate::alert::Protocol;
use crate::proto::natpmp::{self, Response};

/// Resends go out 250 ms, 500 ms, ... apart, and after nine the mapping waits two hours.
const RESEND_STEP: Duration = Duration::from_millis(250);
const MAX_RETRIES: u32 = 9;
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(2 * 3600);

struct Mapping {
    /// `None`: the slot is free (libtorrent `portmap_protocol::none`).
    protocol: Option<Protocol>,
    external_port: u16,
    local_port: u16,
    act: Action,
    expires: Option<Instant>,
    /// Random identifier, used by PCP.
    nonce: [u8; 12],
    /// Only valid if the router supports PCP.
    external_address: Option<IpAddr>,
    /// Set when the first map request is sent.
    map_sent: bool,
    /// Set while we're waiting for a response.
    outstanding_request: bool,
}

impl Mapping {
    fn new() -> Mapping {
        Mapping {
            protocol: None,
            external_port: 0,
            local_port: 0,
            act: Action::None,
            expires: None,
            nonce: [0; 12],
            external_address: None,
            map_sent: false,
            outstanding_request: false,
        }
    }
}

pub(super) struct NatPmp {
    sock: Option<UdpSocket>,
    local: IpAddr,
    version: u8,
    mappings: Vec<Mapping>,
    /// The endpoint of the NAT router.
    nat_endpoint: SocketAddr,
    /// The mapping being updated, if any.
    currently_mapping: Option<usize>,
    retry_count: u32,
    /// Router external IP address; only used when the router does not support PCP (with PCP
    /// the external IP is stored with the mapping).
    external_ip: Option<IpAddr>,
    /// libtorrent `m_send_timer`: when to resend, and for which mapping.
    resend_at: Option<(Instant, usize)>,
    /// libtorrent `m_refresh_timer` and `m_next_refresh`.
    refresh_at: Option<(Instant, usize)>,
    disabled: bool,
    abort: bool,
    /// `natpmp_lease_duration`: seconds a mapping is asked for, 0 for the default.
    lease_duration: u32,
    buf: Vec<u8>,
    pub events: Vec<PortmapEvent>,
    pub log: Vec<String>,
}

impl NatPmp {
    pub fn new() -> NatPmp {
        NatPmp {
            sock: None,
            local: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            version: natpmp::VERSION_PCP,
            mappings: Vec::new(),
            nat_endpoint: SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0),
            currently_mapping: None,
            retry_count: 0,
            external_ip: None,
            resend_at: None,
            refresh_at: None,
            disabled: false,
            abort: false,
            lease_duration: natpmp::MAP_TTL,
            buf: vec![0; natpmp::MAX_PACKET],
            events: Vec::new(),
            log: Vec::new(),
        }
    }

    /// Takes effect with the next request and expiry check, as libtorrent reads the setting.
    pub fn set_lease_duration(&mut self, secs: u32) {
        self.lease_duration = secs;
    }

    /// The lifetime a mapping is asked for: `natpmp_lease_duration`, or an hour for 0.
    fn lease(&self) -> u32 {
        if self.lease_duration > 0 { self.lease_duration } else { natpmp::MAP_TTL }
    }

    #[cfg(test)]
    fn is_open(&self) -> bool {
        self.sock.is_some()
    }

    pub fn deadline(&self) -> Option<Instant> {
        match (self.resend_at.map(|r| r.0), self.refresh_at.map(|r| r.0)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// libtorrent `natpmp::start`: a socket on the interface address, talking to the gateway
    /// at `nat_endpoint` (an IPv6 gateway is link-local: the address carries the interface's
    /// scope id). Assumes the server supports PCP and falls back to NAT-PMP if necessary.
    pub fn start(
        &mut self,
        registry: &Registry,
        token: Token,
        local: IpAddr,
        nat_endpoint: SocketAddr,
        now: Instant,
    ) {
        self.version = natpmp::VERSION_PCP;
        self.disabled = false;
        self.local = local;
        if nat_endpoint == self.nat_endpoint && self.sock.is_some() {
            return;
        }
        self.nat_endpoint = nat_endpoint;
        self.log.push(format!("found gateway at: {}", nat_endpoint.ip()));
        let opened = UdpSocket::bind(SocketAddr::new(local, 0)).and_then(|mut s| {
            registry.register(&mut s, token, Interest::READABLE)?;
            Ok(s)
        });
        match opened {
            Ok(s) => self.sock = Some(s),
            Err(e) => {
                self.disable(e.to_string(), now);
                return;
            }
        }
        if self.version == natpmp::VERSION_NATPMP {
            self.send_get_ip_address_request();
        }
        for i in 0..self.mappings.len() {
            let m = &mut self.mappings[i];
            if m.protocol.is_none() || m.act != Action::None {
                continue;
            }
            m.act = Action::Add;
            self.update_mapping(i, now);
        }
    }

    fn send(&mut self, packet: &[u8]) {
        let Some(s) = self.sock.as_ref() else { return };
        if let Err(e) = s.send_to(packet, self.nat_endpoint) {
            if e.kind() != io::ErrorKind::WouldBlock {
                self.log.push(format!("*** send failed: {e}"));
            }
        }
    }

    /// This opcode only exists in NAT-PMP; PCP routers report the external IP in the response
    /// to a MAP operation.
    fn send_get_ip_address_request(&mut self) {
        if self.version != natpmp::VERSION_NATPMP {
            return;
        }
        self.log.push("==> get public IP address".into());
        self.send(&natpmp::external_address_request());
    }

    fn mapping_log(&mut self, op: &str, i: usize, now: Instant) {
        let m = &self.mappings[i];
        let ttl = m.expires.map_or(0, |e| e.saturating_duration_since(now).as_secs());
        self.log.push(format!(
            "{op}-mapping: proto: {} port: {} local-port: {} action: {:?} ttl: {ttl}",
            m.protocol.map_or("none", crate::proto::upnp::protocol_name),
            m.external_port,
            m.local_port,
            m.act
        ));
    }

    fn disable(&mut self, error: String, now: Instant) {
        self.disabled = true;
        for i in 0..self.mappings.len() {
            let Some(proto) = self.mappings[i].protocol.take() else { continue };
            self.events.push(PortmapEvent {
                mapping: i,
                external_ip: None,
                external_port: 0,
                protocol: proto,
                error: Some(error.clone()),
            });
        }
        self.close_impl(now);
    }

    pub fn delete_mapping(&mut self, i: usize, now: Instant) {
        let Some(m) = self.mappings.get_mut(i) else { return };
        if m.protocol.is_none() {
            return;
        }
        if !m.map_sent {
            m.act = Action::None;
            m.protocol = None;
            return;
        }
        m.act = Action::Del;
        self.update_mapping(i, now);
    }

    /// Returns the mapping's index, `None` when the mapper is disabled.
    pub fn add_mapping(
        &mut self,
        p: Protocol,
        external_port: u16,
        local_port: u16,
        now: Instant,
    ) -> Option<usize> {
        if self.disabled {
            return None;
        }
        let i = match self.mappings.iter().position(|m| m.protocol.is_none()) {
            Some(i) => i,
            None => {
                self.mappings.push(Mapping::new());
                self.mappings.len() - 1
            }
        };
        let m = &mut self.mappings[i];
        crate::util::random::crypto_random_bytes(&mut m.nonce);
        m.protocol = Some(p);
        m.external_port = external_port;
        m.local_port = local_port;
        m.act = Action::Add;
        self.mapping_log("add", i, now);
        self.update_mapping(i, now);
        Some(i)
    }

    fn try_next_mapping(&mut self, i: usize, now: Instant) {
        if i + 1 < self.mappings.len() {
            self.update_mapping(i + 1, now);
            return;
        }
        match self.mappings.iter().position(|m| m.act != Action::None && m.protocol.is_some()) {
            Some(next) => self.update_mapping(next, now),
            None => {
                if self.abort {
                    self.resend_at = None;
                    self.sock = None;
                }
            }
        }
    }

    fn update_mapping(&mut self, i: usize, now: Instant) {
        if i == self.mappings.len() {
            if self.abort {
                self.resend_at = None;
                self.sock = None;
            }
            return;
        }
        self.mapping_log("update", i, now);
        let m = &self.mappings[i];
        if m.act == Action::None || m.protocol.is_none() {
            self.try_next_mapping(i, now);
            return;
        }
        if self.currently_mapping.is_none() {
            // the socket is not currently in use: send out a mapping request
            self.retry_count = 0;
            self.send_map_request(i, now);
        }
    }

    fn send_map_request(&mut self, i: usize, now: Instant) {
        debug_assert!(self.currently_mapping.is_none_or(|c| c == i));
        self.currently_mapping = Some(i);
        let m = &self.mappings[i];
        debug_assert!(m.act != Action::None);
        let Some(protocol) = m.protocol else { return };
        let ttl = if m.act == Action::Add { self.lease() } else { 0 };
        let packet = if self.version == natpmp::VERSION_NATPMP {
            natpmp::map_request_natpmp(protocol, m.local_port, m.external_port, ttl)
        } else {
            natpmp::map_request_pcp(
                self.local,
                &m.nonce,
                protocol,
                m.local_port,
                m.external_port,
                m.external_address,
                ttl,
            )
        };
        self.log.push(format!(
            "==> port map [ mapping: {i} action: {:?} transport: {} proto: {} local: {} external: {} ttl: {ttl} ]",
            m.act,
            natpmp::version_name(self.version),
            crate::proto::upnp::protocol_name(protocol),
            m.local_port,
            m.external_port
        ));
        self.send(&packet);
        let m = &mut self.mappings[i];
        m.map_sent = true;
        m.outstanding_request = true;

        if self.abort {
            // when we're shutting down, ignore the responses and just remove all mappings
            // immediately
            self.currently_mapping = None;
            m.act = Action::None;
            self.try_next_mapping(i, now);
        } else {
            // linear back-off instead of exponential
            self.retry_count += 1;
            self.resend_at = Some((now + RESEND_STEP * self.retry_count, i));
        }
    }

    fn resend_request(&mut self, i: usize, now: Instant) {
        if self.currently_mapping != Some(i) {
            return;
        }
        // if we're shutting down, don't retry, just move on to the next mapping
        if self.retry_count >= MAX_RETRIES || self.abort {
            self.currently_mapping = None;
            self.mappings[i].act = Action::None;
            // try again in two hours
            self.mappings[i].expires = Some(now + RETRY_AFTER_FAILURE);
            self.try_next_mapping(i, now);
            return;
        }
        self.send_map_request(i, now);
    }

    /// Runs the resend and refresh timers.
    pub fn service(&mut self, now: Instant) {
        if let Some((at, i)) = self.resend_at {
            if now >= at {
                self.resend_at = None;
                self.resend_request(i, now);
            }
        }
        if let Some((at, i)) = self.refresh_at {
            if now >= at {
                self.refresh_at = None;
                self.mapping_expired(i, now);
            }
        }
    }

    /// Reads the replies that arrived.
    pub fn on_readable(&mut self, now: Instant) {
        loop {
            let Some(s) = self.sock.as_ref() else { return };
            let mut buf = std::mem::take(&mut self.buf);
            let r = s.recv_from(&mut buf);
            self.buf = buf;
            match r {
                Ok((n, from)) => {
                    let packet = self.buf[..n].to_vec();
                    self.on_reply(&packet, from, now);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if matches!(e.kind(), io::ErrorKind::Interrupted | io::ErrorKind::ConnectionReset) => {
                }
                Err(e) => {
                    self.log.push(format!("error on receiving reply: {e}"));
                    return;
                }
            }
        }
    }

    /// libtorrent `natpmp::on_reply`.
    fn on_reply(&mut self, buf: &[u8], from: SocketAddr, now: Instant) {
        if self.abort {
            return;
        }
        if from != self.nat_endpoint {
            self.log.push(format!("received packet from wrong IP: {from}"));
            return;
        }
        self.resend_at = None;
        let resp = match natpmp::parse_response(buf) {
            Ok(r) => r,
            Err(natpmp::Invalid::Size) => {
                self.log.push(format!("received packet of invalid size: {}", buf.len()));
                return;
            }
            Err(natpmp::Invalid::Version) => {
                self.log.push(format!("unexpected version: {}", buf[0]));
                return;
            }
            Err(natpmp::Invalid::Opcode) => {
                self.log.push(format!("unexpected opcode: {}", buf[1]));
                return;
            }
        };
        let (version, protocol, nonce, private_port, public_port, lifetime, external_addr, result) =
            match resp {
                Response::UnsupportedVersion { .. } => {
                    self.log.push("unsupported version".into());
                    if self.version == natpmp::VERSION_PCP && self.local.is_ipv4() {
                        self.version = natpmp::VERSION_NATPMP;
                        if let Some(i) = self.currently_mapping {
                            self.resend_request(i, now);
                        }
                        self.send_get_ip_address_request();
                    }
                    return;
                }
                Response::ExternalAddress(ip) => {
                    self.external_ip = Some(ip.into());
                    self.log.push(format!("<== public IP address [ {ip} ]"));
                    return;
                }
                Response::Map {
                    version,
                    protocol,
                    nonce,
                    private_port,
                    public_port,
                    lifetime,
                    external_addr,
                    result,
                } => (version, protocol, nonce, private_port, public_port, lifetime, external_addr, result),
            };
        let msg = format!(
            "<== port map [ transport: {} protocol: {} local: {private_port} external: {public_port} ttl: {lifetime} ]",
            natpmp::version_name(version),
            crate::proto::upnp::protocol_name(protocol)
        );
        let found = self.mappings.iter().position(|m| {
            m.local_port == private_port
                && m.protocol == Some(protocol)
                && m.map_sent
                && m.outstanding_request
                && (version != natpmp::VERSION_PCP || nonce == Some(m.nonce))
        });
        let Some(index) = found else {
            self.log.push(format!("{msg} not found in map table"));
            return;
        };
        self.log.push(msg);
        let m = &mut self.mappings[index];
        m.outstanding_request = false;

        if public_port == 0 || lifetime == 0 {
            // this means the mapping was successfully closed
            m.protocol = None;
        } else {
            m.expires = Some(now + Duration::from_secs(lifetime as u64 * 3 / 4));
            m.external_port = public_port;
            if let Some(a) = external_addr.filter(|a| !a.is_unspecified()) {
                m.external_address = Some(a);
            }
        }

        if result != natpmp::RESULT_SUCCESS {
            m.expires = Some(now + RETRY_AFTER_FAILURE);
            let proto = m.protocol.unwrap_or(protocol);
            self.events.push(PortmapEvent {
                mapping: index,
                external_ip: None,
                external_port: 0,
                protocol: proto,
                error: Some(natpmp::result_message(version, result).to_owned()),
            });
        } else if m.act == Action::Add {
            let proto = m.protocol.unwrap_or(protocol);
            let ext_ip = if version == natpmp::VERSION_PCP { m.external_address } else { self.external_ip };
            let external_port = m.external_port;
            self.events.push(PortmapEvent {
                mapping: index,
                external_ip: ext_ip,
                external_port,
                protocol: proto,
                error: None,
            });
        }

        self.currently_mapping = None;
        self.mappings[index].act = Action::None;
        self.resend_at = None;
        self.update_expiration_timer(now);
        self.try_next_mapping(index, now);
    }

    fn update_expiration_timer(&mut self, now: Instant) {
        if self.abort {
            return;
        }
        let now = now + Duration::from_millis(100);
        let mut min_expire = now + Duration::from_secs(self.lease() as u64);
        let mut min_index = None;
        for i in 0..self.mappings.len() {
            let m = &self.mappings[i];
            if m.protocol.is_none() || m.act != Action::None {
                continue;
            }
            let Some(expires) = m.expires else { continue };
            if expires < now {
                self.log.push(format!("mapping {i} expired"));
                self.mappings[i].act = Action::Add;
                if self.refresh_at.is_some_and(|(_, r)| r == i) {
                    self.refresh_at = None;
                }
                self.update_mapping(i, now);
            } else if expires < min_expire {
                min_expire = expires;
                min_index = Some(i);
            }
        }
        // this is already the mapping we're waiting for
        if self.refresh_at.map(|(_, i)| i) == min_index {
            return;
        }
        if let Some(i) = min_index {
            self.log.push(format!(
                "next expiration [ idx: {i} ttl: {} ]",
                min_expire.saturating_duration_since(now).as_secs()
            ));
            self.refresh_at = Some((min_expire, i));
        }
    }

    fn mapping_expired(&mut self, i: usize, now: Instant) {
        if self.abort || i >= self.mappings.len() {
            return;
        }
        self.log.push(format!("mapping {i} expired"));
        self.mappings[i].act = Action::Add;
        self.update_mapping(i, now);
    }

    /// Deletes every mapping (the requests go out at once, without waiting for replies) and
    /// closes the socket.
    pub fn close(&mut self, now: Instant) {
        self.close_impl(now);
    }

    fn close_impl(&mut self, now: Instant) {
        self.abort = true;
        self.log.push("closing".into());
        if self.disabled {
            self.sock = None;
            return;
        }
        for m in &mut self.mappings {
            if m.protocol.is_none() {
                continue;
            }
            m.act = Action::Del;
        }
        self.refresh_at = None;
        self.currently_mapping = None;
        self.update_mapping(0, now);
        self.sock = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mio::{Events, Poll};
    use std::net::UdpSocket as StdUdp;

    /// A gateway on loopback. `pcp` says whether it speaks PCP; otherwise PCP requests get
    /// "unsupported version" and NAT-PMP requests are served.
    fn fake_gateway(pcp: bool) -> (SocketAddr, std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>) {
        let s = StdUdp::bind("127.0.0.1:0").unwrap();
        let addr = s.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 1100];
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            while let Ok((n, from)) = s.recv_from(&mut buf) {
                let req = buf[..n].to_vec();
                seen2.lock().unwrap().push(req.clone());
                let reply: Vec<u8> = match req[0] {
                    natpmp::VERSION_PCP if !pcp => {
                        vec![2, 0x81, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
                    }
                    natpmp::VERSION_PCP => {
                        let mut r = vec![2, 0x81, 0, 0];
                        let lifetime = u32::from_be_bytes(req[4..8].try_into().unwrap());
                        r.extend_from_slice(&lifetime.to_be_bytes());
                        r.extend_from_slice(&[0, 0, 0, 7]);
                        r.extend_from_slice(&[0; 12]);
                        r.extend_from_slice(&req[24..36]); // nonce
                        r.extend_from_slice(&req[36..40]); // protocol
                        r.extend_from_slice(&req[40..42]); // internal port
                        r.extend_from_slice(&req[42..44]); // external port: as asked
                        r.extend_from_slice(
                            &"::ffff:198.51.100.7".parse::<std::net::Ipv6Addr>().unwrap().octets(),
                        );
                        r
                    }
                    _ if req[1] == 0 => {
                        let mut r = vec![0, 128, 0, 0, 0, 0, 0, 7];
                        r.extend_from_slice(&[203, 0, 113, 9]);
                        r
                    }
                    _ => {
                        let mut r = vec![0, 128 + req[1], 0, 0, 0, 0, 0, 7];
                        r.extend_from_slice(&req[4..8]); // ports as asked
                        r.extend_from_slice(&req[8..12]); // lifetime as asked
                        r
                    }
                };
                s.send_to(&reply, from).unwrap();
            }
        });
        (addr, seen)
    }

    fn run(pcp: bool) -> (Vec<PortmapEvent>, Vec<Vec<u8>>) {
        run_with_lease(pcp, natpmp::MAP_TTL)
    }

    fn run_with_lease(pcp: bool, lease: u32) -> (Vec<PortmapEvent>, Vec<Vec<u8>>) {
        let (gw, seen) = fake_gateway(pcp);
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(16);
        let mut n = NatPmp::new();
        n.set_lease_duration(lease);
        let now = Instant::now();
        n.start(poll.registry(), Token(1), "127.0.0.1".parse().unwrap(), gw, now);
        assert!(n.is_open());
        let tcp = n.add_mapping(Protocol::Tcp, 6881, 6881, now).unwrap();
        let udp = n.add_mapping(Protocol::Udp, 6882, 6882, now).unwrap();
        assert_eq!((tcp, udp), (0, 1));
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut closed = false;
        while Instant::now() < deadline {
            let now = Instant::now();
            let timeout =
                n.deadline().map_or(Duration::from_millis(100), |d| d.saturating_duration_since(now));
            poll.poll(&mut events, Some(timeout.min(Duration::from_millis(100)))).unwrap();
            for _ in events.iter() {
                n.on_readable(Instant::now());
            }
            n.service(Instant::now());
            got.append(&mut n.events);
            if got.len() == 2 && !closed {
                closed = true;
                n.close(Instant::now());
                break;
            }
        }
        for l in n.log.drain(..) {
            eprintln!("{l}");
        }
        assert!(closed, "no mapping replies: {got:?}");
        assert!(!n.is_open());
        // the delete requests went out before the socket closed
        std::thread::sleep(Duration::from_millis(200));
        let seen = seen.lock().unwrap().clone();
        (got, seen)
    }

    #[test]
    fn maps_through_pcp_and_deletes_on_close() {
        let (got, seen) = run(true);
        assert_eq!(got.len(), 2);
        for (e, port, proto) in [(&got[0], 6881, Protocol::Tcp), (&got[1], 6882, Protocol::Udp)] {
            assert_eq!(e.error, None);
            assert_eq!((e.external_port, e.protocol), (port, proto));
            assert_eq!(e.external_ip, Some("198.51.100.7".parse().unwrap()));
        }
        // two MAP requests, then two deletes (lifetime 0), all PCP
        let maps: Vec<&Vec<u8>> = seen.iter().filter(|r| r[0] == 2 && r[1] == 1).collect();
        assert!(maps.len() >= 4, "{}", maps.len());
        let deletes = maps.iter().filter(|r| r[4..8] == [0, 0, 0, 0]).count();
        assert_eq!(deletes, 2);
    }

    #[test]
    fn falls_back_to_natpmp() {
        let (got, seen) = run(false);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].error, None);
        assert_eq!(got[0].external_port, 6881);
        assert_eq!(got[1].external_port, 6882);
        // the public address request goes out right after the first (resent) map request, so
        // its answer is in by the time the second mapping is reported
        assert_eq!(
            got[1].external_ip,
            Some("203.0.113.9".parse().unwrap()),
            "from the public address request"
        );
        // after the PCP refusal every request is NAT-PMP, including the deletes
        assert!(seen.iter().any(|r| r[0] == 2));
        assert!(seen.iter().any(|r| r[0] == 0 && r[1] == 0), "public address request");
        let deletes = seen.iter().filter(|r| r[0] == 0 && r[1] != 0 && r[8..12] == [0, 0, 0, 0]).count();
        assert_eq!(deletes, 2);
    }

    /// libtorrent 2.0.13 `natpmp_lease_duration`: map requests ask for the configured lifetime
    /// (PCP and NAT-PMP alike), 0 for the default hour; deletes still ask for 0.
    #[test]
    fn map_requests_ask_for_the_lease_duration() {
        let lifetimes = |seen: &[Vec<u8>]| -> Vec<u32> {
            seen.iter()
                .filter_map(|r| match (r[0], r[1]) {
                    (2, 1) => Some(u32::from_be_bytes(r[4..8].try_into().unwrap())),
                    (0, 1 | 2) => Some(u32::from_be_bytes(r[8..12].try_into().unwrap())),
                    _ => None,
                })
                .collect()
        };
        for (pcp, lease, want) in [(true, 600, 600), (false, 7200, 7200), (true, 0, natpmp::MAP_TTL)] {
            let (got, seen) = run_with_lease(pcp, lease);
            assert_eq!(got.len(), 2);
            let all = lifetimes(&seen);
            let maps: Vec<u32> = all.iter().copied().filter(|&l| l != 0).collect();
            assert!(maps.len() >= 2, "{all:?}");
            assert!(maps.iter().all(|&l| l == want), "pcp {pcp}, lease {lease}: {all:?}");
            assert_eq!(all.len() - maps.len(), 2, "two deletes: {all:?}");
        }
    }

    /// Without a mapping to refresh sooner, the expiry check waits one lease.
    #[test]
    fn the_expiry_check_waits_one_lease() {
        let mut n = NatPmp::new();
        n.set_lease_duration(120);
        let now = Instant::now();
        n.mappings.push(Mapping::new());
        n.mappings[0].protocol = Some(Protocol::Tcp);
        n.mappings[0].expires = Some(now + Duration::from_secs(100));
        n.update_expiration_timer(now);
        assert_eq!(n.refresh_at.map(|(_, i)| i), Some(0), "100 s is within the 120 s lease");
        let mut n = NatPmp::new();
        n.set_lease_duration(60);
        n.mappings.push(Mapping::new());
        n.mappings[0].protocol = Some(Protocol::Tcp);
        n.mappings[0].expires = Some(now + Duration::from_secs(100));
        n.update_expiration_timer(now);
        assert_eq!(n.refresh_at, None, "past the 60 s lease: checked again later");
    }

    #[test]
    fn a_silent_gateway_gives_up_after_nine_resends() {
        let gw = StdUdp::bind("127.0.0.1:0").unwrap();
        let gw_addr = gw.local_addr().unwrap();
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(16);
        let mut n = NatPmp::new();
        let mut now = Instant::now();
        n.start(poll.registry(), Token(1), "127.0.0.1".parse().unwrap(), gw_addr, now);
        n.add_mapping(Protocol::Tcp, 1, 1, now).unwrap();
        // drive the clock through the resends without waiting for them
        let mut sends = 1;
        while let Some(d) = n.deadline() {
            now = d;
            n.service(now);
            sends += 1;
            if sends > 20 {
                break;
            }
        }
        poll.poll(&mut events, Some(Duration::from_millis(10))).unwrap();
        assert_eq!(sends, MAX_RETRIES as usize + 1, "nine requests, then a two-hour wait");
        assert!(n.events.is_empty(), "no event for a silent gateway");
        assert_eq!(n.mappings[0].act, Action::None);
        assert!(n.mappings[0].expires.is_some());
        let mut buf = [0u8; 64];
        gw.set_nonblocking(true).unwrap();
        let mut received = 0;
        while gw.recv_from(&mut buf).is_ok() {
            received += 1;
        }
        assert_eq!(received, MAX_RETRIES);
    }
}
