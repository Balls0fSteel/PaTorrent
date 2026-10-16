// Ported from libtorrent-rasterbar 2.0.11: src/session_impl.cpp (start_dht, add_dht_router,
// on_dht_router_name_lookup, prioritize_dht, on_dht_announce, update_dht_announce_interval,
// save_state "dht state"), src/torrent.cpp (should_announce_dht, dht_announce,
// on_dht_announce_response)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: one UDP socket per address family on the listen port, polled by the reactor. The
// announce round-robin covers running torrents only (libtorrent spreads the interval over every
// loaded torrent). The DHT state is saved to `dht_state.dat` beside the session store.

//! The engine side of the DHT: sockets, bootstrap, announces and peers for torrents.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mio::net::UdpSocket;
use mio::{Interest, Token};

use super::Engine;
use crate::alert::{Alert, Protocol};
use crate::bencode::bdecode;
use crate::dht::{AnnounceArgs, DhtEvent, DhtSettings, DhtState, DhtTracker, read_dht_state, save_dht_state};
use crate::net::resolver::Resolved;
use crate::status::PeerSource;
use crate::types::TorrentId;

pub(crate) const DHT4_TOKEN: Token = Token(11);
pub(crate) const DHT6_TOKEN: Token = Token(12);
/// Resolver cookies of DHT bootstrap lookups carry this bit (tracker cookies count up from 1).
pub(crate) const DHT_COOKIE: u64 = 1 << 63;
const STATE_FILE: &str = "dht_state.dat";
const SAVE_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Largest UDP datagram we read.
const RECV_BUFFER: usize = 1500 * 2;

/// The DHT's part of the engine.
pub(super) struct DhtSession {
    pub tracker: Option<DhtTracker>,
    pub(super) socks: Vec<(bool, UdpSocket)>,
    pending_routers: usize,
    routers: Vec<SocketAddr>,
    /// Nodes from magnet links, added once the DHT runs (libtorrent `m_dht_nodes`).
    pending_nodes: Vec<SocketAddr>,
    state_path: Option<PathBuf>,
    state: Option<DhtState>,
    next_poll: Instant,
    /// Torrents waiting for their first announce (libtorrent `m_dht_torrents`).
    prioritized: VecDeque<TorrentId>,
    next_torrent: usize,
    next_announce: Option<Instant>,
    last_save: Instant,
    buf: Vec<u8>,
}

impl DhtSession {
    pub fn new(now: Instant) -> DhtSession {
        DhtSession {
            tracker: None,
            socks: Vec::new(),
            pending_routers: 0,
            routers: Vec::new(),
            pending_nodes: Vec::new(),
            state_path: None,
            state: None,
            next_poll: now,
            prioritized: VecDeque::new(),
            next_torrent: 0,
            next_announce: None,
            last_save: now,
            buf: vec![0; RECV_BUFFER],
        }
    }

    /// When the reactor must wake for the DHT next.
    pub fn deadline(&self) -> Option<Instant> {
        let t = self.tracker.as_ref().filter(|t| t.is_running())?;
        let _ = t;
        Some(match self.next_announce {
            Some(a) => a.min(self.next_poll),
            None => self.next_poll,
        })
    }

    pub fn num_nodes(&self) -> u32 {
        self.tracker.as_ref().map_or(0, |t| t.num_nodes() as u32)
    }
}

/// libtorrent `parse_comma_separated_string_port`: `host:port` items, IPv6 in brackets.
pub(super) fn parse_bootstrap_nodes(s: &str) -> Vec<(String, u16)> {
    s.split(',')
        .map(str::trim)
        .filter_map(|e| {
            let (host, port) = e.rsplit_once(':')?;
            let host = host.trim_start_matches('[').trim_end_matches(']');
            let port = port.parse::<u16>().ok()?;
            (!host.is_empty() && port != 0).then(|| (host.to_owned(), port))
        })
        .collect()
}

fn load_state(path: &Path) -> DhtState {
    match std::fs::read(path) {
        Ok(buf) => bdecode(&buf[..]).map(|d| read_dht_state(d.root())).unwrap_or_default(),
        Err(_) => DhtState::default(),
    }
}

impl Engine {
    /// Opens the DHT sockets and resolves the bootstrap routers; the DHT starts once they are
    /// resolved (libtorrent `start_dht` waits for `m_outstanding_router_lookups`).
    pub(super) fn start_dht(&mut self, state_dir: Option<&Path>) {
        if !self.settings.enable_dht {
            return;
        }
        let now = Instant::now();
        // the DHT shares the TCP listen port (libtorrent's UDP listen socket)
        let port = self.listen_port;
        for (v4, token) in [(true, DHT4_TOKEN), (false, DHT6_TOKEN)] {
            let ip = if v4 { "0.0.0.0" } else { "[::]" };
            let bind = |p: u16| UdpSocket::bind(format!("{ip}:{p}").parse().expect("valid address"));
            let sock = match bind(port) {
                Ok(s) => Ok(s),
                // the port is taken for UDP: any port works for the DHT
                Err(_) if port != 0 => bind(0),
                Err(e) => Err(e),
            };
            match sock {
                Ok(mut s) => {
                    if let Err(e) = self.poll.registry().register(&mut s, token, Interest::READABLE) {
                        self.alert(Alert::Log { id: None, message: format!("DHT: register failed: {e}") });
                        continue;
                    }
                    if let Ok(local) = s.local_addr() {
                        self.alert(Alert::ListenSucceeded { address: local, protocol: Protocol::Udp });
                    }
                    self.dht.socks.push((v4, s));
                }
                Err(e) => {
                    // no IPv6 here, or (Linux) [::] is dual-stack and 0.0.0.0 has the port
                    let v6_dual = !v4 && e.kind() == io::ErrorKind::AddrInUse;
                    if !v6_dual {
                        self.alert(Alert::ListenFailed {
                            address: format!("{ip}:{port}"),
                            protocol: Protocol::Udp,
                            error: e.into(),
                        });
                    }
                }
            }
        }
        if self.dht.socks.is_empty() {
            return;
        }
        let path = state_dir.map(|d| d.join(STATE_FILE));
        let state = path.as_deref().map(load_state).unwrap_or_default();
        self.dht.state_path = path;
        self.dht.state = Some(state);
        self.dht.next_poll = now;
        for (i, (host, port)) in
            parse_bootstrap_nodes(&self.settings.dht_bootstrap_nodes).into_iter().enumerate()
        {
            match self.resolver.resolve(&host, DHT_COOKIE | ((port as u64) << 32) | i as u64) {
                Some(ips) => self.dht.routers.extend(ips.into_iter().map(|ip| SocketAddr::new(ip, port))),
                None => self.dht.pending_routers += 1,
            }
        }
        if self.dht.pending_routers == 0 {
            self.really_start_dht(now);
        }
    }

    /// A bootstrap router's name resolved (or failed).
    pub(super) fn on_dht_resolved(&mut self, r: Resolved) {
        let port = ((r.cookie >> 32) & 0xffff) as u16;
        match r.result {
            Ok(ips) => {
                let eps: Vec<SocketAddr> = ips.into_iter().map(|ip| SocketAddr::new(ip, port)).collect();
                if let Some(t) = self.dht.tracker.as_mut() {
                    for ep in &eps {
                        t.add_router_node(*ep);
                    }
                }
                self.dht.routers.extend(eps);
            }
            Err(e) => {
                self.alert(Alert::Log { id: None, message: format!("DHT: cannot resolve {}: {e}", r.host) })
            }
        }
        if self.dht.pending_routers > 0 {
            self.dht.pending_routers -= 1;
            if self.dht.pending_routers == 0 && self.dht.tracker.is_none() {
                self.really_start_dht(Instant::now());
            }
        }
    }

    fn really_start_dht(&mut self, now: Instant) {
        let state = self.dht.state.take().unwrap_or_default();
        let mut t = DhtTracker::new(DhtSettings::from(&self.settings), state, now);
        for (v4, _) in &self.dht.socks {
            t.new_socket(*v4, now);
        }
        for r in &self.dht.routers {
            t.add_router_node(*r);
        }
        t.start(now);
        for n in std::mem::take(&mut self.dht.pending_nodes) {
            t.add_node(n, now);
        }
        self.dht.tracker = Some(t);
        self.dht.next_poll = now;
        // torrents started before the DHT was up announce first
        let running: Vec<TorrentId> = self
            .torrent_ids()
            .into_iter()
            .filter(|&id| self.torrent_ref(id).is_some_and(|t| t.is_active()))
            .collect();
        for id in running {
            if !self.dht.prioritized.contains(&id) {
                self.dht.prioritized.push_back(id);
            }
        }
        self.update_dht_announce_interval(now);
        let routers = self.dht.routers.len();
        self.alert(Alert::Log { id: None, message: format!("DHT started ({routers} bootstrap routers)") });
        self.dht_service(now);
    }

    /// Reads datagrams from a DHT socket.
    pub(super) fn on_dht_readable(&mut self, v4: bool) {
        let now = Instant::now();
        loop {
            let Some((_, sock)) = self.dht.socks.iter().find(|(f, _)| *f == v4) else { return };
            let (n, from) = match sock.recv_from(&mut self.dht.buf) {
                Ok(r) => r,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                // Windows reports ICMP port-unreachable as a receive error (without the address)
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let from = normalize(from);
            if let Some(t) = self.dht.tracker.as_mut() {
                let buf = &self.dht.buf[..n];
                t.incoming_packet(from.is_ipv4(), from, buf, now);
            }
        }
        self.dht_service(now);
    }

    /// Runs DHT timers and announces, delivers events and sends queued packets.
    pub(super) fn dht_service(&mut self, now: Instant) {
        let Some(t) = self.dht.tracker.as_mut() else { return };
        if now >= self.dht.next_poll {
            self.dht.next_poll = t.poll(now);
        }
        if self.dht.next_announce.is_some_and(|a| now >= a) {
            self.on_dht_announce(now);
        }
        let Some(t) = self.dht.tracker.as_mut() else { return };
        let events = t.take_events(now);
        for ev in events {
            self.on_dht_event(ev);
        }
        self.dht_flush();
        if now.duration_since(self.dht.last_save) >= SAVE_INTERVAL {
            self.save_dht_state();
        }
    }

    fn dht_flush(&mut self) {
        let Some(t) = self.dht.tracker.as_mut() else { return };
        for p in t.take_outgoing() {
            let Some((_, sock)) = self.dht.socks.iter().find(|(v4, _)| *v4 == p.v4_socket) else { continue };
            // a full socket buffer drops the datagram, as UDP would anyway
            let _ = sock.send_to(&p.data, p.to);
        }
    }

    fn on_dht_event(&mut self, ev: DhtEvent) {
        match ev {
            DhtEvent::Peers { info_hash, peers } => {
                let Some(&id) = self.by_hash.get(&info_hash) else { return };
                let max = self.settings.max_peerlist_size.max(1) as usize;
                let ext = self.external_ip;
                let Some(t) = self.torrent(id) else { return };
                // private torrents never take DHT peers (libtorrent on_dht_announce_response)
                if !t.is_active() || t.ti.as_ref().is_some_and(|ti| ti.is_private()) || peers.is_empty() {
                    return;
                }
                for p in &peers {
                    t.peer_list.add(*p, PeerSource::DHT, ext, max);
                }
                t.status_dirty = true;
                self.alert(Alert::DhtReply { id, num_peers: peers.len() as u32 });
                self.connect_peers_for(id, 10);
            }
            DhtEvent::Bootstrapped => self.alert(Alert::DhtBootstrap),
            DhtEvent::ExternalAddress { ip, .. } => {
                if self.external_ip.is_none() && ip.is_ipv4() {
                    self.external_ip = Some(ip);
                    self.alert(Alert::ExternalIp { ip });
                }
            }
            DhtEvent::LookupDone { .. } => {}
        }
    }

    /// A DHT node from a magnet link (`dht=`); only IP literals are used.
    pub(super) fn add_dht_node(&mut self, host: &str, port: u16) {
        let Ok(ip) = host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>() else {
            return;
        };
        if !self.settings.enable_dht || port == 0 {
            return;
        }
        let ep = SocketAddr::new(ip, port);
        match self.dht.tracker.as_mut() {
            Some(t) => t.add_node(ep, Instant::now()),
            None if self.dht.pending_nodes.len() < 200 => self.dht.pending_nodes.push(ep),
            None => {}
        }
    }

    /// A torrent just started: announce it soon (libtorrent `prioritize_dht`).
    pub(super) fn prioritize_dht(&mut self, id: TorrentId) {
        if !self.settings.enable_dht || self.dht.prioritized.contains(&id) {
            return;
        }
        self.dht.prioritized.push_back(id);
        if self.dht.prioritized.len() == 1 && self.dht.tracker.is_some() {
            self.dht.next_announce = Some(Instant::now());
        }
    }

    /// libtorrent `update_dht_announce_interval`: the interval spread over the torrents.
    fn update_dht_announce_interval(&mut self, now: Instant) {
        let running = self
            .torrent_ids()
            .iter()
            .filter(|&&id| self.torrent_ref(id).is_some_and(|t| t.is_active()))
            .count();
        let mut delay = (1000 * self.settings.dht_announce_interval as u64 / running.max(1) as u64).max(1);
        if !self.dht.prioritized.is_empty() {
            // first announces of new torrents shouldn't wait long
            delay = delay.min(4000);
        }
        self.dht.next_announce = Some(now + Duration::from_millis(delay));
    }

    /// libtorrent `on_dht_announce`: the next prioritized torrent, else the next running one.
    fn on_dht_announce(&mut self, now: Instant) {
        self.update_dht_announce_interval(now);
        while let Some(id) = self.dht.prioritized.pop_front() {
            if self.torrent_ref(id).is_some() {
                self.dht_announce(id, now);
                return;
            }
        }
        let running: Vec<TorrentId> = self
            .torrent_ids()
            .into_iter()
            .filter(|&id| self.torrent_ref(id).is_some_and(|t| t.is_active()))
            .collect();
        if running.is_empty() {
            return;
        }
        if self.dht.next_torrent >= running.len() {
            self.dht.next_torrent = 0;
        }
        let id = running[self.dht.next_torrent];
        self.dht.next_torrent += 1;
        self.dht_announce(id, now);
    }

    /// libtorrent `torrent::should_announce_dht`.
    fn should_announce_dht(&self, id: TorrentId) -> bool {
        let Some(t) = self.torrent_ref(id) else { return false };
        if !self.dht.tracker.as_ref().is_some_and(DhtTracker::is_running)
            || !self.is_running(t)
            || !t.is_active()
        {
            return false;
        }
        // a torrent with metadata announces once its files are checked
        if t.has_metadata() && (t.checking || !t.checked) && t.fetch.is_none() {
            return false;
        }
        // never announce private torrents
        !t.ti.as_ref().is_some_and(|ti| ti.is_private())
    }

    /// libtorrent `torrent::dht_announce`. The TCP listen port is announced explicitly (no
    /// `implied_port`): without uTP, peers must reach the TCP port.
    fn dht_announce(&mut self, id: TorrentId, now: Instant) {
        if !self.should_announce_dht(id) {
            return;
        }
        let port = self.listen_port;
        let Some(t) = self.torrent_ref(id) else { return };
        let ih = t.info_hashes.best_v1_compatible();
        let seed = t.is_seed();
        let Some(tracker) = self.dht.tracker.as_mut() else { return };
        if port == 0 {
            // nothing to announce: just look for peers
            tracker.get_peers(ih, now);
        } else {
            tracker.announce(ih, AnnounceArgs { port, seed, implied_port: false }, now);
        }
    }

    /// Writes `dht_state.dat` (atomically) beside the session store.
    pub(super) fn save_dht_state(&mut self) {
        self.dht.last_save = Instant::now();
        let (Some(t), Some(path)) = (self.dht.tracker.as_ref(), self.dht.state_path.as_ref()) else { return };
        let data = save_dht_state(&t.state()).bencode();
        let tmp = path.with_extension("tmp");
        let res = std::fs::write(&tmp, &data).and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = res {
            self.out.push(Alert::Log { id: None, message: format!("cannot save the DHT state: {e}") });
        }
    }
}

/// IPv4-mapped IPv6 senders are IPv4 nodes.
fn normalize(a: SocketAddr) -> SocketAddr {
    match a {
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::new(v4.into(), v6.port()),
            None => a,
        },
        SocketAddr::V4(_) => a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_nodes() {
        assert_eq!(
            parse_bootstrap_nodes(
                "dht.libtorrent.org:25401, router.bittorrent.com:6881,[2001:db8::1]:6881,bad,x:0"
            ),
            [
                ("dht.libtorrent.org".to_owned(), 25401),
                ("router.bittorrent.com".to_owned(), 6881),
                ("2001:db8::1".to_owned(), 6881)
            ]
        );
    }
}
