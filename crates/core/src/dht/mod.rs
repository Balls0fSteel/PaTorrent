// Ported from libtorrent-rasterbar 2.0.11: src/kademlia/dht_tracker.cpp,
// include/libtorrent/kademlia/dht_tracker.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: sans-IO. The tracker owns at most one node per address family (libtorrent has one
// per listen socket); asio timers become deadlines checked by `poll`; packets queue in an outbox
// the reactor sends from the UDP sockets; the nodes' external-address votes go to the engine's
// session voter (libtorrent's listen socket), which calls `update_node_id` when it changes.

//! The mainline DHT (BEP 5) with IPv6 (BEP 32), node-id security (BEP 42) and read-only mode
//! (BEP 43).

pub mod dos_blocker;
pub mod msg;
pub mod node;
pub mod node_id;
pub mod routing_table;
pub mod settings;
pub mod state;
pub mod storage;

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use crate::bencode::{Entry, bdecode_with_limits};
use crate::types::Sha1Hash;
use dos_blocker::DosBlocker;
pub use node::{AnnounceArgs, DhtEvent, Msg, Node, SocketManager};
use node_id::NodeId;
pub use settings::DhtSettings;
pub use state::{DhtState, read_dht_state, save_dht_state};
use storage::DhtStorage;

/// The write-token secret rotates this often.
const KEY_REFRESH: Duration = Duration::from_secs(5 * 60);
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const STORAGE_TICK: Duration = Duration::from_secs(2 * 60);

/// A packet for the reactor to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    /// Send it from the IPv4 socket (else the IPv6 one).
    pub v4_socket: bool,
    pub to: SocketAddr,
    pub data: Vec<u8>,
}

/// The `v` key of our messages: client id and version (BEP 5 / BEP 20 style).
fn version_bytes() -> [u8; 4] {
    let mut v = crate::VERSION.split('.').map(|p| p.parse::<u8>().unwrap_or(0));
    let (major, minor, tiny) = (v.next().unwrap_or(0), v.next().unwrap_or(0), v.next().unwrap_or(0));
    [b'P', b'A', major, (minor.min(15) << 4) | tiny.min(15)]
}

/// The send side shared by the nodes: the upload quota and the outbox.
#[derive(Debug)]
pub struct DhtIo {
    send_quota: i64,
    last_tick: Instant,
    limit: i64,
    has_v4: bool,
    has_v6: bool,
    pub outbox: Vec<Outgoing>,
    pub bytes_out: u64,
}

impl DhtIo {
    fn new(limit: i32, now: Instant) -> DhtIo {
        DhtIo {
            send_quota: limit as i64,
            last_tick: now,
            limit: limit as i64,
            has_v4: false,
            has_v6: false,
            outbox: Vec::new(),
            bytes_out: 0,
        }
    }
}

impl SocketManager for DhtIo {
    /// libtorrent `dht_tracker::has_quota`: a token bucket with 3 seconds of burst.
    fn has_quota(&mut self, now: Instant) -> bool {
        let delta = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        let limit = self.limit.max(1);
        let max_accrue = (3 * limit).min(i32::MAX as i64);
        if delta >= Duration::from_secs(3) || delta.as_micros() as i64 >= i32::MAX as i64 / limit {
            self.send_quota = max_accrue;
            return true;
        }
        let add = limit * delta.as_micros() as i64 / 1_000_000;
        if max_accrue - self.send_quota < add {
            self.send_quota = max_accrue;
            return true;
        }
        self.send_quota += add;
        self.send_quota > 0
    }

    fn send_packet(&mut self, local_v4: bool, e: &mut Entry, addr: SocketAddr, _now: Instant) -> bool {
        *e.key("v") = Entry::Bytes(version_bytes().to_vec());
        let data = e.bencode();
        // over quota is fine for sending; it only blocks answering the next query
        self.send_quota -= data.len() as i64;
        // a node may address the other family during bootstrap: use that family's socket
        let v4 = addr.is_ipv4();
        let ok = if v4 == local_v4 {
            true
        } else if v4 {
            self.has_v4
        } else {
            self.has_v6
        };
        if !ok {
            return false;
        }
        self.bytes_out += data.len() as u64;
        self.outbox.push(Outgoing { v4_socket: v4, to: addr, data });
        true
    }
}

struct TrackerNode {
    node: Node,
    next_timeout: Instant,
}

/// libtorrent `dht_tracker`: the nodes, their timers, the DoS blocker and the peer storage.
pub struct DhtTracker {
    settings: DhtSettings,
    nodes: Vec<TrackerNode>,
    storage: DhtStorage,
    blocker: DosBlocker,
    pub io: DhtIo,
    state: DhtState,
    running: bool,
    next_refresh: Instant,
    next_key_refresh: Instant,
    next_storage_tick: Instant,
    events: Vec<DhtEvent>,
    pub bytes_in: u64,
}

impl DhtTracker {
    pub fn new(settings: DhtSettings, state: DhtState, now: Instant) -> DhtTracker {
        let mut blocker = DosBlocker::default();
        blocker.set_block_timer(settings.block_timeout);
        blocker.set_rate_limit(settings.block_ratelimit);
        DhtTracker {
            io: DhtIo::new(settings.upload_rate_limit, now),
            settings,
            nodes: Vec::new(),
            storage: DhtStorage::default(),
            blocker,
            state,
            running: false,
            next_refresh: now,
            next_key_refresh: now,
            next_storage_tick: now + STORAGE_TICK,
            events: Vec::new(),
            bytes_in: 0,
        }
    }

    /// A UDP socket of this family is open: run a node on it (libtorrent `new_socket`).
    /// `external` is the family's external address if the session knows it; the node id is
    /// derived from it (BEP 42).
    pub fn new_socket(&mut self, is_v4: bool, external: Option<IpAddr>, now: Instant) {
        if self.nodes.iter().any(|n| n.node.is_v4() == is_v4) {
            return;
        }
        let nid = self
            .state
            .nids
            .iter()
            .find(|(a, _)| a.is_ipv4() == is_v4)
            .map_or(NodeId::default(), |(_, id)| *id);
        let node = Node::new(is_v4, nid, external, self.settings.clone());
        if is_v4 {
            self.io.has_v4 = true;
        } else {
            self.io.has_v6 = true;
        }
        let mut n = TrackerNode { node, next_timeout: now + Duration::from_secs(1) };
        if self.running {
            n.node.bootstrap(&mut self.io, &[], false, now);
        }
        self.nodes.push(n);
    }

    pub fn set_settings(&mut self, s: DhtSettings) {
        self.io.limit = s.upload_rate_limit as i64;
        for n in &mut self.nodes {
            n.node.set_settings(s.clone());
        }
        self.settings = s;
    }

    pub fn add_router_node(&mut self, ep: SocketAddr) {
        for n in &mut self.nodes {
            n.node.add_router_node(ep);
        }
    }

    pub fn add_node(&mut self, ep: SocketAddr, now: Instant) {
        for n in &mut self.nodes {
            n.node.add_node(&mut self.io, ep, now);
        }
    }

    /// Bootstraps every node from the saved nodes (its own family first).
    pub fn start(&mut self, now: Instant) {
        self.running = true;
        self.next_key_refresh = now + KEY_REFRESH;
        self.next_refresh = now + REFRESH_INTERVAL;
        for n in &mut self.nodes {
            n.next_timeout = now + Duration::from_secs(1);
            let (own, other) = if n.node.is_v4() {
                (&self.state.nodes, &self.state.nodes6)
            } else {
                (&self.state.nodes6, &self.state.nodes)
            };
            let nodes: Vec<SocketAddr> = own.iter().chain(other.iter()).copied().collect();
            n.node.bootstrap(&mut self.io, &nodes, true, now);
        }
        self.state.clear();
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Runs the timers that are due; returns when to call again.
    pub fn poll(&mut self, now: Instant) -> Instant {
        if !self.running {
            return now + Duration::from_secs(3600);
        }
        for n in &mut self.nodes {
            if now >= n.next_timeout {
                let d = n.node.connection_timeout(&mut self.io, now);
                n.next_timeout = now + d;
            }
        }
        if now >= self.next_storage_tick {
            self.storage.tick(now);
            self.next_storage_tick = now + STORAGE_TICK;
        }
        if now >= self.next_refresh {
            for n in &mut self.nodes {
                n.node.tick(&mut self.io, now);
            }
            self.blocker.set_block_timer(self.settings.block_timeout);
            self.blocker.set_rate_limit(self.settings.block_ratelimit);
            self.next_refresh = now + REFRESH_INTERVAL;
        }
        if now >= self.next_key_refresh {
            for n in &mut self.nodes {
                n.node.new_write_key();
            }
            self.next_key_refresh = now + KEY_REFRESH;
        }
        self.collect_events();
        let mut next = self.next_refresh.min(self.next_key_refresh).min(self.next_storage_tick);
        for n in &self.nodes {
            next = next.min(n.next_timeout);
        }
        next
    }

    /// Moves node events out, external-address votes (`DhtEvent::ExternalAddress`) included.
    fn collect_events(&mut self) {
        for n in &mut self.nodes {
            self.events.extend(n.node.take_events());
        }
    }

    pub fn take_events(&mut self) -> Vec<DhtEvent> {
        self.collect_events();
        std::mem::take(&mut self.events)
    }

    /// libtorrent `dht_tracker::update_node_id`: the session's external address of this family
    /// changed, so the node takes a BEP 42 id for it unless its id already matches.
    pub fn update_node_id(&mut self, is_v4: bool, external: IpAddr, now: Instant) {
        for n in self.nodes.iter_mut().filter(|n| n.node.is_v4() == is_v4) {
            n.node.update_node_id(external, now);
        }
    }

    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        std::mem::take(&mut self.io.outbox)
    }

    /// libtorrent `incoming_packet`: returns false if it wasn't a DHT message.
    pub fn incoming_packet(&mut self, local_v4: bool, ep: SocketAddr, buf: &[u8], now: Instant) -> bool {
        if buf.len() <= 20 || buf[0] != b'd' || buf[buf.len() - 1] != b'e' {
            return false;
        }
        self.bytes_in += buf.len() as u64;
        if self.settings.ignore_dark_internet {
            if let IpAddr::V4(a) = ep.ip() {
                // class A networks not available to the public: suspicious
                const CLASS_A: [u8; 17] = [3, 6, 7, 9, 11, 19, 21, 22, 25, 26, 28, 29, 30, 33, 34, 48, 56];
                if CLASS_A.contains(&a.octets()[0]) {
                    return true;
                }
            }
        }
        if !self.blocker.incoming(ep.ip(), now) {
            return true;
        }
        let Ok(d) = bdecode_with_limits(buf, 10, 500) else { return false };
        let Some(root) = d.root().filter(|r| r.is_dict()) else { return false };
        let m = Msg { message: root, addr: ep };
        // the node of the socket it came in on answers queries; with a `want` it may need the
        // other family's table
        let Some(i) = self.nodes.iter().position(|n| n.node.is_v4() == local_v4) else { return true };
        let (a, b) = self.nodes.split_at_mut(i);
        let (me, rest) = b.split_first_mut().expect("index valid");
        let other = a.first_mut().or(rest.first_mut()).map(|n| &mut n.node);
        me.node.incoming(&mut self.io, &mut self.storage, &m, other, now);
        self.collect_events();
        true
    }

    pub fn get_peers(&mut self, ih: Sha1Hash, now: Instant) {
        for n in &mut self.nodes {
            n.node.get_peers(&mut self.io, ih, false, now);
        }
    }

    pub fn announce(&mut self, ih: Sha1Hash, args: AnnounceArgs, now: Instant) {
        for n in &mut self.nodes {
            n.node.announce(&mut self.io, ih, args, now);
        }
    }

    /// Live routing-table nodes over all families (the status bar's "DHT: N nodes").
    pub fn num_nodes(&self) -> usize {
        self.nodes.iter().map(|n| n.node.stats().0).sum()
    }

    /// The state to save (libtorrent `dht_tracker::state`).
    pub fn state(&self) -> DhtState {
        let mut ret = DhtState::default();
        for n in &self.nodes {
            let local: IpAddr = if n.node.is_v4() {
                std::net::Ipv4Addr::UNSPECIFIED.into()
            } else {
                std::net::Ipv6Addr::UNSPECIFIED.into()
            };
            ret.nids.push((local, n.node.nid()));
            let nodes = n.node.save_nodes();
            if n.node.is_v4() {
                ret.nodes.extend(nodes);
            } else {
                ret.nodes6.extend(nodes);
            }
        }
        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io(limit: i32, now: Instant) -> DhtIo {
        DhtIo::new(limit, now)
    }

    // libtorrent test_dht.cpp: rate_limit_int_max, rate_limit_large_delta, rate_limit_accrue_limit
    #[test]
    fn rate_limit() {
        let now = Instant::now();
        let mut sm = io(i32::MAX, now);
        assert!(sm.has_quota(now));
        assert!(sm.has_quota(now + Duration::from_millis(10)));
        let mut sm = io(i32::MAX, now);
        assert!(sm.has_quota(now));
        assert!(sm.has_quota(now + Duration::from_millis(2500)));
        let mut sm = io(i32::MAX, now);
        let mut t = now;
        assert!(sm.has_quota(t));
        for _ in 0..10 {
            t += Duration::from_millis(500);
            assert!(sm.has_quota(t));
        }
        // a small limit runs out
        let mut sm = io(1000, now);
        assert!(sm.has_quota(now));
        let mut e = Entry::new_dict();
        *e.key("x") = Entry::Bytes(vec![0; 5000]);
        assert!(sm.send_packet(true, &mut e, "1.2.3.4:5".parse().unwrap(), now));
        assert!(!sm.has_quota(now + Duration::from_millis(10)));
        assert!(sm.has_quota(now + Duration::from_secs(4)));
    }

    #[test]
    fn version_key() {
        let v = version_bytes();
        assert_eq!(&v[..2], b"PA");
    }

    #[test]
    fn send_needs_a_socket_of_the_family() {
        let now = Instant::now();
        let mut sm = io(8000, now);
        let mut e = Entry::new_dict();
        assert!(sm.send_packet(true, &mut e, "1.2.3.4:5".parse().unwrap(), now));
        assert!(!sm.send_packet(true, &mut e, "[1::2]:5".parse().unwrap(), now));
        sm.has_v6 = true;
        assert!(sm.send_packet(true, &mut e, "[1::2]:5".parse().unwrap(), now));
        assert_eq!(sm.outbox.len(), 2);
        assert!(!sm.outbox[1].v4_socket);
    }

    #[test]
    fn dark_internet_and_garbage() {
        let now = Instant::now();
        let mut t = DhtTracker::new(DhtSettings::default(), DhtState::default(), now);
        t.new_socket(true, None, now);
        assert!(!t.incoming_packet(true, "1.2.3.4:5".parse().unwrap(), b"x", now));
        let ping = b"d1:ad2:id20:aaaaaaaaaaaaaaaaaaaae1:q4:ping1:t2:aa1:y1:qe";
        assert!(t.incoming_packet(true, "3.2.3.4:5".parse().unwrap(), ping, now));
        assert!(t.take_outgoing().is_empty(), "dark internet is ignored");
        assert!(t.incoming_packet(true, "8.2.3.4:5".parse().unwrap(), ping, now));
        assert_eq!(t.take_outgoing().len(), 1);
    }

    /// The nodes pass `ip` votes on to the session, and follow its external address.
    #[test]
    fn nodes_follow_the_session_external_address() {
        use node_id::verify_id;
        let now = Instant::now();
        let ext: IpAddr = "51.1.1.1".parse().unwrap();
        let mut t = DhtTracker::new(DhtSettings::default(), DhtState::default(), now);
        t.new_socket(true, Some(ext), now);
        t.new_socket(false, None, now);
        assert!(verify_id(&t.nodes[0].node.nid(), &ext), "a known address gives a BEP 42 id");
        let v6_id = t.nodes[1].node.nid();

        // a ping carrying `ip` is a vote, handed to the session as it is
        let mut q = b"d1:ad2:id20:aaaaaaaaaaaaaaaaaaaae2:ip6:".to_vec();
        q.extend_from_slice(&[53, 3, 3, 3, 0x1a, 0xe1]);
        q.extend_from_slice(b"1:q4:ping1:t2:aa1:y1:qe");
        assert!(t.incoming_packet(true, "8.2.3.4:5".parse().unwrap(), &q, now));
        let votes: Vec<_> = t
            .take_events()
            .into_iter()
            .filter_map(|e| match e {
                DhtEvent::ExternalAddress { ip, source } => Some((ip, source)),
                _ => None,
            })
            .collect();
        assert_eq!(votes, [("53.3.3.3".parse().unwrap(), "8.2.3.4".parse().unwrap())]);
        assert!(verify_id(&t.nodes[0].node.nid(), &ext), "one vote doesn't change the id");

        let ext2: IpAddr = "53.3.3.3".parse().unwrap();
        t.update_node_id(true, ext2, now);
        assert!(verify_id(&t.nodes[0].node.nid(), &ext2));
        assert_eq!(t.nodes[1].node.nid(), v6_id, "the IPv6 node keeps its id");
    }
}
