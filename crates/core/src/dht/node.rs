// Ported from libtorrent-rasterbar 2.0.11: src/kademlia/node.cpp, src/kademlia/rpc_manager.cpp,
// src/kademlia/traversal_algorithm.cpp, src/kademlia/find_data.cpp, src/kademlia/get_peers.cpp,
// src/kademlia/refresh.cpp, include/libtorrent/kademlia/{node,rpc_manager,observer,
// traversal_algorithm,find_data,get_peers,refresh}.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: observers and traversal algorithms live in slabs owned by the node instead of a
// shared_ptr graph (an observer is freed once it is neither a lookup result nor an outstanding
// transaction; a lookup once it is done and has no observers left). Callbacks become
// `DhtEvent`s and the follow-up announce is a field of the lookup. Outgoing packets go through
// the `SocketManager` trait; the clock is passed in. BEP 44 / BEP 51 queries are answered like
// `find_node` until Phase 8. The session's IP filter comes with the `SocketManager` of each
// call; fixes from 2.1.1 (abc121b1e9).

//! One DHT node (one address family): answers queries, runs lookups, keeps its routing table.

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use super::msg::{
    KeyDesc, LAST_CHILD, OPTIONAL, PARSE_CHILDREN, endpoint_to_bytes, key, read_endpoint, read_endpoint_list,
    read_nodes, verify_message, write_nodes_entry,
};
use super::node_id::{
    NodeId, compare_ref, distance_exp, generate_id, generate_prefix_mask, generate_random_id,
    generate_secret_id, make_id_secret, random_inclusive, verify_id,
};
use super::routing_table::{NodeEntry, RoutingTable};
use super::settings::DhtSettings;
use super::storage::DhtStorage;
use crate::bencode::{Entry, Node as BNode, NodeType};
use crate::crypto::Sha1;
use crate::net::ip_filter::IpFilter;
use crate::net::ip_voter::is_local;
use crate::types::Sha1Hash;
use crate::util::random::crypto_random_bytes;

const WRITE_TOKEN_SIZE: usize = 4;
/// A lookup keeps at most this many results.
const MAX_RESULTS: usize = 100;

/// Sends packets for a node and meters its replies (libtorrent `socket_manager`).
pub trait SocketManager {
    /// False when we are over the DHT upload rate and should not answer queries.
    fn has_quota(&mut self, now: Instant) -> bool;
    /// Adds the version key, encodes and sends `e`. `local_v4` is the family of the sending
    /// node's socket; a packet for the other family may go out on the other socket.
    fn send_packet(&mut self, local_v4: bool, e: &mut Entry, addr: SocketAddr, now: Instant) -> bool;
    /// The session's IP filter, if any (libtorrent `dht_observer::get_dht_ip_filter`).
    fn ip_filter(&self) -> Option<&IpFilter> {
        None
    }
}

/// Something the node reports to its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DhtEvent {
    /// Peers for a lookup started with `get_peers` or `announce`.
    Peers { info_hash: Sha1Hash, peers: Vec<SocketAddr> },
    /// The startup bootstrap finished.
    Bootstrapped,
    /// A node told us which address it sees us at (the `ip` key of BEP 42).
    ExternalAddress { ip: IpAddr, source: IpAddr },
    /// A lookup started with `get_peers` / `announce` has finished.
    LookupDone { info_hash: Sha1Hash, responses: u32 },
}

/// A received message and where it came from (libtorrent `msg`).
#[derive(Clone, Copy, Debug)]
pub struct Msg<'a> {
    pub message: BNode<'a>,
    pub addr: SocketAddr,
}

// observer flags (libtorrent `observer::flag_*`)
pub const FLAG_QUERIED: u8 = 1 << 0;
pub const FLAG_INITIAL: u8 = 1 << 1;
pub const FLAG_NO_ID: u8 = 1 << 2;
pub const FLAG_SHORT_TIMEOUT: u8 = 1 << 3;
pub const FLAG_FAILED: u8 = 1 << 4;
pub const FLAG_IPV6_ADDRESS: u8 = 1 << 5;
pub const FLAG_ALIVE: u8 = 1 << 6;
pub const FLAG_DONE: u8 = 1 << 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObserverKind {
    /// `null_observer` / `announce_observer`: a reply just completes the request.
    Null,
    /// `get_peers_observer` (also used by bootstrap).
    GetPeers,
    /// `obfuscated_get_peers_observer`.
    Obfuscated,
    /// `ping_observer` of `send_single_refresh`.
    Ping,
}

#[derive(Clone, Debug)]
struct Observer {
    algorithm: usize,
    id: NodeId,
    addr: SocketAddr,
    sent: Instant,
    flags: u8,
    kind: ObserverKind,
    in_results: bool,
    in_transactions: bool,
    /// Held by a caller that is still using it.
    pins: u16,
}

/// What a lookup does when it finishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceArgs {
    pub port: u16,
    pub seed: bool,
    pub implied_port: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TraversalKind {
    /// Not a lookup: the owner of single requests (pings, refreshes, announces).
    Dummy,
    /// `get_peers` / `obfuscated_get_peers`.
    GetPeers { noseeds: bool, obfuscated: bool },
    /// `bootstrap` (a `get_peers` for our own id).
    Bootstrap,
}

#[derive(Clone, Debug)]
struct Traversal {
    target: NodeId,
    kind: TraversalKind,
    results: Vec<usize>,
    sorted_results: usize,
    invoke_count: i32,
    branch_factor: i32,
    responses: u32,
    timeouts: u32,
    done: bool,
    abort: bool,
    peer4_prefixes: BTreeSet<u32>,
    peer6_prefixes: BTreeSet<u64>,
    /// Live observers pointing here.
    observers: usize,
    // find_data
    write_tokens: HashMap<NodeId, Vec<u8>>,
    find_data_done: bool,
    /// Report peers as `DhtEvent::Peers` (libtorrent's data callback).
    report_peers: bool,
    /// Announce to the closest nodes with write tokens when done (the nodes callback).
    announce: Option<AnnounceArgs>,
    /// Post `Bootstrapped` when done.
    notify_bootstrap: bool,
}

impl Traversal {
    fn new(target: NodeId, kind: TraversalKind) -> Traversal {
        Traversal {
            target,
            kind,
            results: Vec::new(),
            sorted_results: 0,
            invoke_count: 0,
            branch_factor: 3,
            responses: 0,
            timeouts: 0,
            done: false,
            abort: false,
            peer4_prefixes: BTreeSet::new(),
            peer6_prefixes: BTreeSet::new(),
            observers: 0,
            write_tokens: HashMap::new(),
            find_data_done: false,
            report_peers: false,
            announce: None,
            notify_bootstrap: false,
        }
    }
}

/// Index-stable storage with reuse of freed slots.
#[derive(Clone, Debug)]
struct Slab<T> {
    entries: Vec<Option<T>>,
    free: Vec<usize>,
    len: usize,
}

impl<T> Default for Slab<T> {
    fn default() -> Self {
        Slab { entries: Vec::new(), free: Vec::new(), len: 0 }
    }
}

impl<T> Slab<T> {
    fn insert(&mut self, v: T) -> usize {
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

    fn get(&self, i: usize) -> &T {
        self.entries[i].as_ref().expect("live slab entry")
    }

    fn get_mut(&mut self, i: usize) -> &mut T {
        self.entries[i].as_mut().expect("live slab entry")
    }

    fn remove(&mut self, i: usize) -> T {
        self.len -= 1;
        self.free.push(i);
        self.entries[i].take().expect("live slab entry")
    }
}

/// libtorrent `node` (with its `rpc_manager`).
pub struct Node {
    pub settings: DhtSettings,
    id: NodeId,
    pub table: RoutingTable,
    is_v4: bool,
    secret: [[u8; 4]; 2],
    last_self_refresh: Option<Instant>,
    observers: Slab<Observer>,
    traversals: Slab<Traversal>,
    /// Outstanding requests: (transaction id, observer).
    transactions: Vec<(u16, usize)>,
    events: Vec<DhtEvent>,
    /// Running lookups (libtorrent `m_running_requests`).
    running: usize,
}

/// libtorrent `node::calculate_node_id`.
fn calculate_node_id(nid: NodeId, external: Option<IpAddr>) -> NodeId {
    match external {
        None => {
            if nid.is_zero() {
                generate_random_id()
            } else {
                nid
            }
        }
        Some(ext) => {
            if nid.is_zero() || !verify_id(&nid, &ext) {
                generate_id(&ext)
            } else {
                nid
            }
        }
    }
}

/// Whether an endpoint that the node at `from` told us about (a `nodes` record or a peer in
/// `values`) may be used (SEC-11, not in libtorrent): never port 0 or an unspecified,
/// multicast or broadcast address; loopback only from this machine, and a private,
/// link-local or CGNAT address only from such an address, so a LAN DHT keeps working while an
/// internet node cannot aim our queries and connections at the local network.
pub(crate) fn usable_endpoint(ep: &SocketAddr, from: &IpAddr) -> bool {
    let ip = ep.ip().to_canonical();
    let from = from.to_canonical();
    if ep.port() == 0 || ip.is_unspecified() || ip.is_multicast() || ip == IpAddr::V4(Ipv4Addr::BROADCAST) {
        return false;
    }
    if ip.is_loopback() {
        return from.is_loopback();
    }
    !is_local(&ip) || is_local(&from)
}

/// An error reply (libtorrent `incoming_error`).
fn incoming_error(e: &mut Entry, msg: &str, code: i64) {
    *e.key("y") = Entry::from("e");
    let l = e.key("e").list_mut();
    l.push(Entry::Int(code));
    l.push(Entry::from(msg));
}

impl Node {
    /// `external` is our external address if known (BEP 42 ids derive from it); `nid` a saved
    /// id, or zero.
    pub fn new(is_v4: bool, nid: NodeId, external: Option<IpAddr>, settings: DhtSettings) -> Node {
        let id = calculate_node_id(nid, external);
        let mut secret = [[0u8; 4]; 2];
        crypto_random_bytes(&mut secret[0]);
        crypto_random_bytes(&mut secret[1]);
        Node {
            table: RoutingTable::new(id, is_v4, 8, settings.clone()),
            settings,
            id,
            is_v4,
            secret,
            last_self_refresh: None,
            observers: Slab::default(),
            traversals: Slab::default(),
            transactions: Vec::new(),
            events: Vec::new(),
            running: 0,
        }
    }

    pub fn nid(&self) -> NodeId {
        self.id
    }

    pub fn is_v4(&self) -> bool {
        self.is_v4
    }

    pub fn set_settings(&mut self, s: DhtSettings) {
        self.table.settings = s.clone();
        self.settings = s;
    }

    /// "n4" / "n6": the `want` value for this family.
    pub fn protocol_family_name(&self) -> &'static str {
        if self.is_v4 { "n4" } else { "n6" }
    }

    /// "nodes" / "nodes6".
    pub fn protocol_nodes_key(&self) -> &'static str {
        if self.is_v4 { "nodes" } else { "nodes6" }
    }

    pub fn native_address(&self, a: &IpAddr) -> bool {
        a.is_ipv4() == self.is_v4
    }

    pub fn take_events(&mut self) -> Vec<DhtEvent> {
        std::mem::take(&mut self.events)
    }

    /// (live nodes, replacements, allocated observers).
    pub fn stats(&self) -> (usize, usize, usize) {
        let (n, r, _) = self.table.size();
        (n, r, self.observers.len)
    }

    #[cfg(test)]
    pub fn num_running_lookups(&self) -> usize {
        self.running
    }

    #[cfg(test)]
    pub fn num_transactions(&self) -> usize {
        self.transactions.len()
    }

    /// Our external address changed: take a BEP 42 id for it unless ours already matches.
    /// The routing table is rebuilt, so `filter` (the session's IP filter) applies to its
    /// nodes again.
    pub fn update_node_id(&mut self, external: IpAddr, filter: Option<&IpFilter>, now: Instant) {
        if verify_id(&self.id, &external) {
            return;
        }
        self.id = generate_id(&external);
        self.table.update_node_id(self.id, filter, now);
    }

    /// libtorrent `traversal_algorithm::filtered`: `addr` is blocked by the session's IP
    /// filter and `apply_filter_to_dht` is on.
    fn filtered(&self, sm: &dyn SocketManager, addr: &SocketAddr) -> bool {
        self.settings.apply_filter_to_dht && sm.ip_filter().is_some_and(|f| f.is_blocked(addr.ip()))
    }

    fn token_hash(&self, addr: &SocketAddr, info_hash: &Sha1Hash, secret: usize) -> Sha1Hash {
        let mut h = Sha1::new();
        h.update(addr.ip().to_string().as_bytes());
        h.update(&self.secret[secret]);
        h.update(&info_hash.0);
        h.finalize()
    }

    pub fn verify_token(&self, token: &[u8], info_hash: &Sha1Hash, addr: &SocketAddr) -> bool {
        token.len() == WRITE_TOKEN_SIZE
            && (token == &self.token_hash(addr, info_hash, 0).0[..WRITE_TOKEN_SIZE]
                || token == &self.token_hash(addr, info_hash, 1).0[..WRITE_TOKEN_SIZE])
    }

    pub fn generate_token(&self, addr: &SocketAddr, info_hash: &Sha1Hash) -> Vec<u8> {
        self.token_hash(addr, info_hash, 0).0[..WRITE_TOKEN_SIZE].to_vec()
    }

    /// Rotates the write-token secret (every 5 minutes); tokens of the previous one still pass.
    pub fn new_write_key(&mut self) {
        self.secret[1] = self.secret[0];
        crypto_random_bytes(&mut self.secret[0]);
    }

    pub fn add_router_node(&mut self, router: SocketAddr) {
        self.table.add_router_node(router);
    }

    /// Pings `ep`; it enters the routing table if it answers.
    pub fn add_node(&mut self, sm: &mut dyn SocketManager, ep: SocketAddr, now: Instant) {
        if !self.native_address(&ep.ip()) || self.filtered(sm, &ep) {
            return;
        }
        let bucket = self.table.num_active_buckets();
        self.send_single_refresh(sm, ep, bucket, NodeId::default(), now);
    }

    pub fn bootstrap(
        &mut self,
        sm: &mut dyn SocketManager,
        nodes: &[SocketAddr],
        notify: bool,
        now: Instant,
    ) {
        let mut target = self.id;
        make_id_secret(&mut target);
        let t = self.traversals.insert(Traversal::new(target, TraversalKind::Bootstrap));
        self.traversals.get_mut(t).notify_bootstrap = notify;
        self.last_self_refresh = Some(now);
        for n in nodes {
            // saved or configured nodes: unlike those found by `traverse`, not filtered yet
            if self.filtered(sm, n) {
                continue;
            }
            self.add_entry(t, NodeId::default(), *n, FLAG_INITIAL, now);
        }
        self.start(sm, t, now);
    }

    /// Looks up peers for `info_hash`; they arrive as `DhtEvent::Peers`.
    pub fn get_peers(
        &mut self,
        sm: &mut dyn SocketManager,
        info_hash: Sha1Hash,
        noseeds: bool,
        now: Instant,
    ) {
        self.start_get_peers(sm, info_hash, noseeds, None, now);
    }

    /// Looks up peers and then announces to the closest nodes (libtorrent `node::announce`).
    pub fn announce(
        &mut self,
        sm: &mut dyn SocketManager,
        info_hash: Sha1Hash,
        args: AnnounceArgs,
        now: Instant,
    ) {
        self.start_get_peers(sm, info_hash, args.seed, Some(args), now);
    }

    fn start_get_peers(
        &mut self,
        sm: &mut dyn SocketManager,
        info_hash: Sha1Hash,
        noseeds: bool,
        announce: Option<AnnounceArgs>,
        now: Instant,
    ) {
        let obfuscated = self.settings.privacy_lookups;
        let mut tr = Traversal::new(info_hash, TraversalKind::GetPeers { noseeds, obfuscated });
        tr.report_peers = true;
        tr.announce = announce;
        let t = self.traversals.insert(tr);
        self.start(sm, t, now);
    }

    // --- observers ------------------------------------------------------------------------

    fn new_observer(
        &mut self,
        t: usize,
        ep: SocketAddr,
        id: NodeId,
        kind: ObserverKind,
        now: Instant,
    ) -> usize {
        self.traversals.get_mut(t).observers += 1;
        let flags = if ep.is_ipv6() { FLAG_IPV6_ADDRESS } else { 0 };
        self.observers.insert(Observer {
            algorithm: t,
            id,
            addr: ep,
            sent: now,
            flags,
            kind,
            in_results: false,
            in_transactions: false,
            pins: 0,
        })
    }

    /// Frees an observer nobody refers to any more, and its lookup if that is finished too.
    fn maybe_free_observer(&mut self, o: usize) {
        let ob = self.observers.get(o);
        if ob.in_results || ob.in_transactions || ob.pins > 0 {
            return;
        }
        let t = ob.algorithm;
        self.observers.remove(o);
        let tr = self.traversals.get_mut(t);
        tr.observers -= 1;
        self.maybe_free_traversal(t);
    }

    fn pin(&mut self, o: usize) {
        self.observers.get_mut(o).pins += 1;
    }

    fn unpin(&mut self, o: usize) {
        self.observers.get_mut(o).pins -= 1;
        self.maybe_free_observer(o);
    }

    fn maybe_free_traversal(&mut self, t: usize) {
        let Some(Some(tr)) = self.traversals.entries.get(t) else { return };
        let finished = tr.done || tr.kind == TraversalKind::Dummy;
        if finished && tr.observers == 0 && tr.results.is_empty() {
            self.traversals.remove(t);
        }
    }

    fn observer_timeout(&mut self, sm: &mut dyn SocketManager, o: usize, now: Instant) {
        if self.observers.get(o).flags & FLAG_DONE != 0 {
            return;
        }
        self.observers.get_mut(o).flags |= FLAG_DONE;
        self.failed(sm, o, false, now);
    }

    fn observer_short_timeout(&mut self, sm: &mut dyn SocketManager, o: usize, now: Instant) {
        if self.observers.get(o).flags & FLAG_SHORT_TIMEOUT != 0 {
            return;
        }
        self.failed(sm, o, true, now);
    }

    fn observer_done(&mut self, sm: &mut dyn SocketManager, o: usize, now: Instant) {
        if self.observers.get(o).flags & FLAG_DONE != 0 {
            return;
        }
        self.observers.get_mut(o).flags |= FLAG_DONE;
        self.finished(sm, o, now);
    }

    fn observer_set_id(&mut self, o: usize, id: NodeId) {
        let ob = self.observers.get_mut(o);
        if ob.id == id {
            return;
        }
        ob.id = id;
        let t = ob.algorithm;
        self.resort_result(t, o);
    }

    /// The observer's reply (the `reply` virtual of each observer type).
    fn observer_reply(&mut self, sm: &mut dyn SocketManager, o: usize, m: &Msg<'_>, now: Instant) {
        let kind = self.observers.get(o).kind;
        match kind {
            ObserverKind::Null => self.observers.get_mut(o).flags |= FLAG_DONE,
            ObserverKind::Ping => {
                self.observers.get_mut(o).flags |= FLAG_DONE;
                let Some(r) = m.message.dict_find_dict("r") else { return };
                for (id, ep) in read_nodes(r, self.protocol_nodes_key(), !self.is_v4) {
                    if usable_endpoint(&ep, &m.addr.ip()) {
                        self.table.heard_about(&id, ep, sm.ip_filter(), now);
                    }
                }
            }
            ObserverKind::GetPeers => {
                let Some(r) = m.message.dict_find_dict("r") else {
                    return self.observer_timeout(sm, o, now);
                };
                if let Some(n) = r.dict_find_list("values") {
                    let first = n.list_at(0);
                    let peers: Vec<SocketAddr> =
                        if n.list_size() == 1 && first.is_some_and(|f| f.is_string()) && m.addr.is_ipv4() {
                            // mainline format: one string of 6-byte peers
                            first
                                .expect("checked")
                                .string_value()
                                .chunks_exact(6)
                                .filter_map(read_endpoint)
                                .collect()
                        } else {
                            read_endpoint_list(n)
                        };
                    let peers = peers.into_iter().filter(|p| usable_endpoint(p, &m.addr.ip())).collect();
                    let t = self.observers.get(o).algorithm;
                    let tr = self.traversals.get(t);
                    if tr.report_peers {
                        self.events.push(DhtEvent::Peers { info_hash: tr.target, peers });
                    }
                }
                // find_data_observer::reply
                let Some(id) = r.dict_find_string("id").filter(|i| i.string_length() == 20) else {
                    return self.observer_timeout(sm, o, now);
                };
                if let Some(token) = r.dict_find_string("token") {
                    let nid = NodeId::from_slice(id.string_value()).expect("20 bytes");
                    let t = self.observers.get(o).algorithm;
                    self.traversals.get_mut(t).write_tokens.insert(nid, token.string_value().to_vec());
                }
                self.traversal_observer_reply(sm, o, m, now);
                self.observer_done(sm, o, now);
            }
            ObserverKind::Obfuscated => {
                let Some(r) = m.message.dict_find_dict("r") else {
                    return self.observer_timeout(sm, o, now);
                };
                if r.dict_find_string("id").is_none_or(|i| i.string_length() != 20) {
                    return self.observer_timeout(sm, o, now);
                }
                self.traversal_observer_reply(sm, o, m, now);
                self.observer_done(sm, o, now);
            }
        }
    }

    /// libtorrent `traversal_observer::reply`: traverse the returned nodes, learn the id.
    fn traversal_observer_reply(&mut self, sm: &mut dyn SocketManager, o: usize, m: &Msg<'_>, now: Instant) {
        let Some(r) = m.message.dict_find_dict("r") else {
            return self.observer_timeout(sm, o, now);
        };
        let t = self.observers.get(o).algorithm;
        for (id, ep) in read_nodes(r, self.protocol_nodes_key(), !self.is_v4) {
            if usable_endpoint(&ep, &m.addr.ip()) {
                self.traverse(sm, id, ep, t, now);
            }
        }
        let Some(id) = r.dict_find_string("id").filter(|i| i.string_length() == 20) else {
            return self.observer_timeout(sm, o, now);
        };
        self.observer_set_id(o, NodeId::from_slice(id.string_value()).expect("20 bytes"));
    }

    // --- traversal_algorithm ----------------------------------------------------------------

    fn observer_kind_for(&self, t: usize) -> ObserverKind {
        match self.traversals.get(t).kind {
            TraversalKind::Dummy => ObserverKind::Null,
            TraversalKind::GetPeers { obfuscated: true, .. } => ObserverKind::Obfuscated,
            TraversalKind::GetPeers { .. } | TraversalKind::Bootstrap => ObserverKind::GetPeers,
        }
    }

    fn cmp_target(&self, t: usize, a: usize, b: usize) -> bool {
        compare_ref(&self.observers.get(a).id, &self.observers.get(b).id, &self.traversals.get(t).target)
    }

    /// Position of `o` among the sorted results (lower bound).
    fn lower_bound(&self, t: usize, o: usize) -> usize {
        let tr = self.traversals.get(t);
        tr.results[..tr.sorted_results].partition_point(|&r| self.cmp_target(t, r, o))
    }

    fn resort_result(&mut self, t: usize, o: usize) {
        let tr = self.traversals.get_mut(t);
        let Some(pos) = tr.results.iter().position(|&r| r == o) else { return };
        if pos < tr.sorted_results {
            tr.sorted_results -= 1;
        }
        tr.results.remove(pos);
        let at = self.lower_bound(t, o);
        let tr = self.traversals.get_mut(t);
        tr.results.insert(at, o);
        tr.sorted_results += 1;
    }

    fn add_entry(&mut self, t: usize, id: NodeId, addr: SocketAddr, flags: u8, now: Instant) {
        if self.traversals.get(t).done {
            return;
        }
        let kind = self.observer_kind_for(t);
        let o = self.new_observer(t, addr, id, kind, now);
        self.observers.get_mut(o).flags |= flags;
        if id.is_zero() {
            let ob = self.observers.get_mut(o);
            ob.id = generate_random_id();
            ob.flags |= FLAG_NO_ID;
            ob.in_results = true;
            self.traversals.get_mut(t).results.push(o);
        } else {
            let at = self.lower_bound(t, o);
            let tr = self.traversals.get(t);
            let end = tr.sorted_results;
            if at == end || self.observers.get(tr.results[at]).id != id {
                // the IP restriction does not apply to nodes from our own table
                if self.settings.restrict_search_ips && flags & FLAG_INITIAL == 0 {
                    let tr = self.traversals.get_mut(t);
                    let fresh = match addr.ip() {
                        IpAddr::V6(a) => tr
                            .peer6_prefixes
                            .insert(u64::from_be_bytes(a.octets()[..8].try_into().expect("8"))),
                        IpAddr::V4(a) => tr.peer4_prefixes.insert(u32::from(a) & 0xffff_ff00),
                    };
                    if !fresh {
                        // another result in this /24 (/64) claims a different id: ignore it
                        self.observers.get_mut(o).in_results = false;
                        self.maybe_free_observer(o);
                        return;
                    }
                }
                self.observers.get_mut(o).in_results = true;
                let tr = self.traversals.get_mut(t);
                tr.results.insert(at, o);
                tr.sorted_results += 1;
            } else {
                // already a result
                self.maybe_free_observer(o);
                return;
            }
        }
        let tr = self.traversals.get_mut(t);
        if tr.results.len() > MAX_RESULTS {
            let dropped: Vec<usize> = tr.results.split_off(MAX_RESULTS);
            tr.sorted_results = tr.sorted_results.min(MAX_RESULTS);
            for d in dropped {
                let ob = self.observers.get_mut(d);
                if ob.flags & (FLAG_QUERIED | FLAG_FAILED | FLAG_ALIVE) == FLAG_QUERIED {
                    // outstanding: make sure it won't report back
                    ob.flags |= FLAG_DONE;
                    let short = ob.flags & FLAG_SHORT_TIMEOUT != 0;
                    let tr = self.traversals.get_mut(t);
                    tr.invoke_count -= 1;
                    if short {
                        tr.branch_factor -= 1;
                    }
                }
                self.observers.get_mut(d).in_results = false;
                self.maybe_free_observer(d);
            }
        }
    }

    fn start(&mut self, sm: &mut dyn SocketManager, t: usize, now: Instant) {
        self.running += 1;
        // find_data::start: seed with the k closest nodes we know
        if self.traversals.get(t).results.is_empty() {
            let target = self.traversals.get(t).target;
            for n in self.table.find_node(&target, true, 0) {
                self.add_entry(t, n.id, n.ep(), FLAG_INITIAL, now);
            }
        }
        // an empty routing table: use the router nodes (configured, trusted bootstrap
        // endpoints: the IP filter does not apply to them)
        if self.traversals.get(t).results.len() < 3 {
            let routers: Vec<SocketAddr> = self.table.router_nodes().copied().collect();
            for r in routers {
                self.add_entry(t, NodeId::default(), r, FLAG_INITIAL, now);
            }
        }
        self.traversals.get_mut(t).branch_factor = self.settings.search_branching;
        if self.add_requests(sm, t, now) {
            self.done(sm, t, now);
        }
    }

    fn traverse(&mut self, sm: &dyn SocketManager, id: NodeId, addr: SocketAddr, t: usize, now: Instant) {
        if self.traversals.get(t).done || self.filtered(sm, &addr) {
            return;
        }
        // let the routing table know this node may exist
        self.table.heard_about(&id, addr, sm.ip_filter(), now);
        self.add_entry(t, id, addr, 0, now);
    }

    fn finished(&mut self, sm: &mut dyn SocketManager, o: usize, now: Instant) {
        let t = self.observers.get(o).algorithm;
        let ob = self.observers.get_mut(o);
        let short = ob.flags & FLAG_SHORT_TIMEOUT != 0;
        ob.flags |= FLAG_ALIVE;
        let tr = self.traversals.get_mut(t);
        if tr.kind == TraversalKind::Dummy {
            return;
        }
        if short {
            tr.branch_factor -= 1;
        }
        tr.responses += 1;
        tr.invoke_count -= 1;
        if self.add_requests(sm, t, now) {
            self.done(sm, t, now);
        }
    }

    fn failed(&mut self, sm: &mut dyn SocketManager, o: usize, short_timeout: bool, now: Instant) {
        let ob = self.observers.get(o).clone();
        // don't tell the routing table about ids we made up
        if ob.flags & FLAG_NO_ID == 0 {
            self.table.node_failed(&ob.id, &ob.addr);
        }
        let t = ob.algorithm;
        if self.traversals.get(t).results.is_empty() {
            return;
        }
        let mut decrement_branch_factor = false;
        let tr = self.traversals.get_mut(t);
        if short_timeout {
            // probably no answer coming: open the slot, but keep waiting for a late reply
            if ob.flags & FLAG_SHORT_TIMEOUT == 0 && tr.branch_factor < i8::MAX as i32 {
                tr.branch_factor += 1;
                self.observers.get_mut(o).flags |= FLAG_SHORT_TIMEOUT;
            }
        } else {
            self.observers.get_mut(o).flags |= FLAG_FAILED;
            decrement_branch_factor = ob.flags & FLAG_SHORT_TIMEOUT != 0;
            tr.timeouts += 1;
            tr.invoke_count -= 1;
        }
        if decrement_branch_factor {
            tr.branch_factor = (tr.branch_factor - 1).max(1);
        }
        if self.add_requests(sm, t, now) {
            self.done(sm, t, now);
        }
    }

    /// Issues requests to keep `branch_factor` outstanding; true when the lookup is complete.
    fn add_requests(&mut self, sm: &mut dyn SocketManager, t: usize, now: Instant) -> bool {
        if self.traversals.get(t).done {
            return true;
        }
        let mut results_target = self.table.bucket_size() as i32;
        let mut outstanding = 0;
        let agg = self.settings.aggressive_lookups;
        let mut i = 0;
        loop {
            let tr = self.traversals.get(t);
            if i >= tr.results.len()
                || results_target <= 0
                || !(if agg { outstanding < tr.branch_factor } else { tr.invoke_count < tr.branch_factor })
            {
                break;
            }
            let o = tr.results[i];
            let abort = tr.abort;
            i += 1;
            let flags = self.observers.get(o).flags;
            if flags & FLAG_ALIVE != 0 {
                results_target -= 1;
                continue;
            }
            if flags & FLAG_QUERIED != 0 {
                // queried, not alive and not failed: in flight
                if flags & FLAG_FAILED == 0 {
                    outstanding += 1;
                }
                continue;
            }
            if abort {
                continue;
            }
            self.observers.get_mut(o).flags |= FLAG_QUERIED;
            if self.invoke(sm, t, o, now) {
                self.traversals.get_mut(t).invoke_count += 1;
                outstanding += 1;
            } else {
                self.observers.get_mut(o).flags |= FLAG_FAILED;
            }
        }
        // done once k results answered with nothing in flight, or nothing could be sent
        (results_target == 0 && outstanding == 0) || self.traversals.get(t).invoke_count == 0
    }

    /// Sends the lookup's query to `o`.
    fn invoke(&mut self, sm: &mut dyn SocketManager, t: usize, o: usize, now: Instant) -> bool {
        let tr = self.traversals.get(t);
        let target = tr.target;
        let mut e = Entry::new_dict();
        *e.key("y") = Entry::from("q");
        *e.key("q") = Entry::from("get_peers");
        match tr.kind {
            TraversalKind::Dummy => return false,
            TraversalKind::Bootstrap => {
                // our current id (it may have changed since the bootstrap started)
                let mut target = self.id;
                make_id_secret(&mut target);
                let a = e.key("a");
                *a.key("info_hash") = Entry::Bytes(target.0.to_vec());
                if self.observers.get(o).flags & FLAG_INITIAL != 0 {
                    // tell router nodes we are bootstrapping
                    *a.key("bs") = Entry::Int(1);
                }
            }
            TraversalKind::GetPeers { noseeds, obfuscated } => {
                if tr.find_data_done {
                    return false;
                }
                let info_hash = if obfuscated {
                    // send only as many bits of the target as this node needs to answer well
                    let shared_prefix = 160 - distance_exp(&self.observers.get(o).id, &target);
                    let mask = generate_prefix_mask(shared_prefix + 3);
                    let mut obf = generate_random_id() & !mask;
                    obf |= target & mask;
                    obf
                } else {
                    target
                };
                let a = e.key("a");
                *a.key("info_hash") = Entry::Bytes(info_hash.0.to_vec());
                if noseeds && !obfuscated {
                    *a.key("noseed") = Entry::Int(1);
                }
            }
        }
        let addr = self.observers.get(o).addr;
        self.rpc_invoke(sm, e, addr, o, now)
    }

    fn done(&mut self, sm: &mut dyn SocketManager, t: usize, now: Instant) {
        let kind = self.traversals.get(t).kind.clone();
        match kind {
            TraversalKind::GetPeers { obfuscated: true, noseeds } => {
                // switch to a plain get_peers with the nodes we found
                let tr = self.traversals.get_mut(t);
                let (report, announce) = (tr.report_peers, tr.announce.take());
                tr.report_peers = false;
                let target = tr.target;
                let mut next = Traversal::new(target, TraversalKind::GetPeers { noseeds, obfuscated: false });
                next.report_peers = report;
                next.announce = announce;
                let nt = self.traversals.insert(next);
                let mut added = 0;
                for &o in &self.traversals.get(t).results.clone() {
                    if added >= 16 {
                        break;
                    }
                    let ob = self.observers.get(o);
                    if ob.flags & FLAG_NO_ID != 0 || ob.flags & FLAG_ALIVE == 0 {
                        continue;
                    }
                    let (id, addr) = (ob.id, ob.addr);
                    self.add_entry(nt, id, addr, FLAG_INITIAL, now);
                    added += 1;
                }
                self.start(sm, nt, now);
                self.find_data_done(sm, t, now);
            }
            TraversalKind::GetPeers { .. } => self.find_data_done(sm, t, now),
            TraversalKind::Bootstrap => {
                // ping the nodes we never got to
                let unqueried: Vec<SocketAddr> = self
                    .traversals
                    .get(t)
                    .results
                    .iter()
                    .map(|&o| self.observers.get(o))
                    .filter(|ob| ob.flags & FLAG_QUERIED == 0)
                    .map(|ob| ob.addr)
                    .collect();
                for ep in unqueried {
                    self.add_node(sm, ep, now);
                }
                self.find_data_done(sm, t, now);
            }
            TraversalKind::Dummy => self.traversal_done(t),
        }
    }

    /// libtorrent `find_data::done`: hand the closest live nodes with tokens to the callback.
    fn find_data_done(&mut self, sm: &mut dyn SocketManager, t: usize, now: Instant) {
        let tr = self.traversals.get_mut(t);
        tr.find_data_done = true;
        let mut results: Vec<(NodeEntry, Vec<u8>)> = Vec::new();
        let mut num_results = self.table.bucket_size();
        let tr = self.traversals.get(t);
        for &o in &tr.results {
            if num_results == 0 {
                break;
            }
            let ob = self.observers.get(o);
            if ob.flags & FLAG_ALIVE == 0 {
                continue;
            }
            let Some(token) = tr.write_tokens.get(&ob.id) else { continue };
            results.push((NodeEntry::new(ob.id, ob.addr, 0xffff, false, now), token.clone()));
            num_results -= 1;
        }
        let (announce, notify, report, target, responses) =
            (tr.announce, tr.notify_bootstrap, tr.report_peers, tr.target, tr.responses);
        if let Some(args) = announce {
            self.announce_to(sm, &results, target, args, now);
        }
        if notify {
            self.events.push(DhtEvent::Bootstrapped);
        }
        if report {
            self.events.push(DhtEvent::LookupDone { info_hash: target, responses });
        }
        self.traversal_done(t);
    }

    /// libtorrent `traversal_algorithm::done`.
    fn traversal_done(&mut self, t: usize) {
        let tr = self.traversals.get_mut(t);
        debug_assert!(!tr.done);
        tr.done = true;
        if tr.kind != TraversalKind::Dummy {
            self.running -= 1;
        }
        let results = std::mem::take(&mut tr.results);
        tr.sorted_results = 0;
        tr.invoke_count = 0;
        for o in results {
            let ob = self.observers.get_mut(o);
            if ob.flags & (FLAG_QUERIED | FLAG_FAILED) == FLAG_QUERIED {
                // outstanding: don't report back to a finished lookup
                ob.flags |= FLAG_DONE;
            }
            ob.in_results = false;
            self.maybe_free_observer(o);
        }
        self.maybe_free_traversal(t);
    }

    /// libtorrent `announce_fun`: announce_peer to each node with a write token.
    fn announce_to(
        &mut self,
        sm: &mut dyn SocketManager,
        nodes: &[(NodeEntry, Vec<u8>)],
        info_hash: Sha1Hash,
        args: AnnounceArgs,
        now: Instant,
    ) {
        let t = self.traversals.insert(Traversal::new(NodeId::default(), TraversalKind::Dummy));
        for (n, token) in nodes {
            let o = self.new_observer(t, n.ep(), n.id, ObserverKind::Null, now);
            let mut e = Entry::new_dict();
            *e.key("y") = Entry::from("q");
            *e.key("q") = Entry::from("announce_peer");
            let a = e.key("a");
            *a.key("info_hash") = Entry::Bytes(info_hash.0.to_vec());
            *a.key("port") = Entry::Int(args.port as i64);
            *a.key("token") = Entry::Bytes(token.clone());
            *a.key("seed") = Entry::Int(i64::from(args.seed));
            if args.implied_port {
                *a.key("implied_port") = Entry::Int(1);
            }
            self.observers.get_mut(o).flags |= FLAG_QUERIED;
            self.rpc_invoke(sm, e, n.ep(), o, now);
        }
        self.maybe_free_traversal(t);
    }

    // --- rpc_manager --------------------------------------------------------------------------

    /// Sends a query for observer `o`; it is tracked until answered or timed out.
    fn rpc_invoke(
        &mut self,
        sm: &mut dyn SocketManager,
        mut e: Entry,
        target: SocketAddr,
        o: usize,
        now: Instant,
    ) -> bool {
        *e.key("y") = Entry::from("q");
        let family = self.protocol_family_name();
        let native = self.native_address(&target.ip());
        let a = e.key("a");
        *a.key("id") = Entry::Bytes(self.id.0.to_vec());
        if !native {
            a.key("want").list_mut().push(Entry::from(family));
        }
        // from the ChaCha20 generator keyed by the OS (SEC-02), so not predictable
        let tid = random_inclusive(0xffff) as u16;
        *e.key("t") = Entry::Bytes(tid.to_be_bytes().to_vec());
        // BEP 43: read-only nodes say so in every query
        if self.settings.read_only {
            *e.key("ro") = Entry::Int(1);
        }
        let ob = self.observers.get_mut(o);
        ob.sent = now;
        ob.addr = target;
        if target.is_ipv6() {
            ob.flags |= FLAG_IPV6_ADDRESS;
        } else {
            ob.flags &= !FLAG_IPV6_ADDRESS;
        }
        if sm.send_packet(self.is_v4, &mut e, target, now) {
            self.observers.get_mut(o).in_transactions = true;
            self.transactions.push((tid, o));
            true
        } else {
            self.maybe_free_observer(o);
            false
        }
    }

    /// Times out requests; returns when to call again.
    pub fn connection_timeout(&mut self, sm: &mut dyn SocketManager, now: Instant) -> Duration {
        const SHORT_TIMEOUT: Duration = Duration::from_secs(1);
        const TIMEOUT: Duration = Duration::from_secs(15);
        if self.transactions.is_empty() {
            return SHORT_TIMEOUT;
        }
        let mut timeouts = Vec::new();
        let mut short_timeouts = Vec::new();
        let mut ret = SHORT_TIMEOUT;
        let mut i = 0;
        while i < self.transactions.len() {
            let o = self.transactions[i].1;
            let ob = self.observers.get(o);
            if ob.flags & FLAG_DONE != 0 {
                // cancelled
                self.transactions.remove(i);
                self.observers.get_mut(o).in_transactions = false;
                self.maybe_free_observer(o);
                continue;
            }
            let diff = now.saturating_duration_since(ob.sent);
            if diff >= TIMEOUT {
                self.transactions.remove(i);
                self.observers.get_mut(o).in_transactions = false;
                self.pin(o);
                timeouts.push(o);
                continue;
            }
            if diff >= SHORT_TIMEOUT && ob.flags & FLAG_SHORT_TIMEOUT == 0 {
                short_timeouts.push(o);
            } else {
                ret = ret.min(TIMEOUT - diff);
            }
            i += 1;
        }
        for o in timeouts {
            self.observer_timeout(sm, o, now);
            self.unpin(o);
        }
        for o in short_timeouts {
            self.observer_short_timeout(sm, o, now);
        }
        ret.max(Duration::from_millis(200))
    }

    /// A reply or error: returns the replying node's id if it matched a request.
    fn rpc_incoming(&mut self, sm: &mut dyn SocketManager, m: &Msg<'_>, now: Instant) -> Option<NodeId> {
        let t = m.message.dict_find_string_value("t").unwrap_or_default();
        if t.is_empty() {
            return None;
        }
        let tid = if t.len() == 2 { u16::from_be_bytes([t[0], t[1]]) } else { 0xffff };
        // the reply must come from the address and port we asked (libtorrent checks the
        // address only), so a spoofer has to hit both and the transaction id (SEC-11). Not
        // `SocketAddr` equality: a received IPv6 address may carry a flow label or scope.
        let i = self.transactions.iter().position(|&(id, o)| {
            let to = self.observers.get(o).addr;
            id == tid && to.ip() == m.addr.ip() && to.port() == m.addr.port()
        })?;
        // not necessarily an attack: requests are dropped when we restart
        let (_, o) = self.transactions.remove(i);
        self.observers.get_mut(o).in_transactions = false;
        self.pin(o);
        let ret = self.rpc_incoming_matched(sm, m, o, now);
        self.unpin(o);
        ret
    }

    fn rpc_incoming_matched(
        &mut self,
        sm: &mut dyn SocketManager,
        m: &Msg<'_>,
        o: usize,
        now: Instant,
    ) -> Option<NodeId> {
        if m.message.dict_find_string_value("y") == Some(b"e") {
            // an error counts as a failure, not a response
            self.observer_timeout(sm, o, now);
            return None;
        }
        let Some(r) = m.message.dict_find_dict("r") else {
            self.observer_timeout(sm, o, now);
            return None;
        };
        let Some(id) = r.dict_find_string("id").filter(|n| n.string_length() == 20) else {
            self.observer_timeout(sm, o, now);
            return None;
        };
        let nid = NodeId::from_slice(id.string_value()).expect("20 bytes");
        if self.settings.enforce_node_id && !verify_id(&nid, &m.addr.ip()) {
            self.observer_timeout(sm, o, now);
            return None;
        }
        let sent = self.observers.get(o).sent;
        self.observer_reply(sm, o, m, now);
        let rtt = now.saturating_duration_since(sent).as_millis().min(0xfffe) as i32;
        // it answered our transaction id, so it isn't spoofed: add it to the table
        self.table.node_seen(&nid, m.addr, rtt, sm.ip_filter(), now);
        Some(nid)
    }

    // --- node ---------------------------------------------------------------------------------

    /// Handles a message from `m.addr`. `other` is the routing table of the other address
    /// family, for `want` lists.
    pub fn incoming(
        &mut self,
        sm: &mut dyn SocketManager,
        storage: &mut DhtStorage,
        m: &Msg<'_>,
        other: Option<&mut Node>,
        now: Instant,
    ) {
        // obviously broken messages get no answer (no magnification)
        let Some(y) = m.message.dict_find_string_value("y").filter(|y| y.len() == 1) else { return };
        if let Some(ip) = m.message.dict_find_string_value("ip") {
            let ext = if ip.len() >= 16 {
                Some(IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&ip[..16]).expect("16"))))
            } else if ip.len() >= 4 {
                Some(IpAddr::V4(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])))
            } else {
                None
            };
            if let Some(ip) = ext {
                self.events.push(DhtEvent::ExternalAddress { ip, source: m.addr.ip() });
            }
        }
        match y[0] {
            b'r' | b'e' => {
                self.rpc_incoming(sm, m, now);
            }
            b'q' => {
                // BEP 43: read-only nodes don't answer queries
                if self.settings.read_only {
                    return;
                }
                if !sm.has_quota(now) {
                    return;
                }
                let mut e = Entry::new_dict();
                self.incoming_request(storage, m, &mut e, other, sm.ip_filter(), now);
                sm.send_packet(self.is_v4, &mut e, m.addr, now);
            }
            _ => {}
        }
    }

    /// Builds the response to a query (libtorrent `node::incoming_request`). `filter` is the
    /// session's IP filter, for the routing table.
    pub fn incoming_request(
        &mut self,
        storage: &mut DhtStorage,
        m: &Msg<'_>,
        e: &mut Entry,
        mut other: Option<&mut Node>,
        filter: Option<&IpFilter>,
        now: Instant,
    ) {
        *e = Entry::new_dict();
        *e.key("y") = Entry::from("r");
        *e.key("t") = Entry::Bytes(m.message.dict_find_string_value("t").unwrap_or_default().to_vec());
        const TOP_DESC: [KeyDesc; 4] = [
            key("q", NodeType::Str, 0, 0),
            key("ro", NodeType::Int, 0, OPTIONAL),
            key("a", NodeType::Dict, 0, PARSE_CHILDREN),
            key("id", NodeType::Str, 20, LAST_CHILD),
        ];
        let top = match verify_message(m.message, &TOP_DESC) {
            Ok(t) => t,
            Err(err) => return incoming_error(e, &err, 203),
        };
        *e.key("ip") = Entry::Bytes(endpoint_to_bytes(&m.addr));
        let arg_ent = top[2].expect("required");
        let read_only = top[1].is_some_and(|n| n.int_value() != 0);
        let id = NodeId::from_slice(top[3].expect("required").string_value()).expect("20 bytes");
        // a node id that doesn't match the IP gets an error
        if self.settings.enforce_node_id && !verify_id(&id, &m.addr.ip()) {
            return incoming_error(e, "invalid node ID", 203);
        }
        if !read_only {
            self.table.heard_about(&id, m.addr, filter, now);
        }
        {
            let reply = e.key("r");
            *reply.key("id") = Entry::Bytes(self.id.0.to_vec());
            // mirror back the other node's external port
            *reply.key("p") = Entry::Int(m.addr.port() as i64);
        }
        let query = top[0].expect("required").string_value();
        match query {
            b"ping" => {}
            b"get_peers" => {
                const DESC: [KeyDesc; 4] = [
                    key("info_hash", NodeType::Str, 20, 0),
                    key("noseed", NodeType::Int, 0, OPTIONAL),
                    key("scrape", NodeType::Int, 0, OPTIONAL),
                    key("want", NodeType::List, 0, OPTIONAL),
                ];
                let k = match verify_message(arg_ent, &DESC) {
                    Ok(k) => k,
                    Err(err) => return incoming_error(e, &err, 203),
                };
                let info_hash = Sha1Hash::from_slice(k[0].expect("required").string_value()).expect("20");
                // always return nodes as well as peers
                self.write_nodes_entries(&info_hash, k[3], e.key("r"), other.as_deref_mut());
                let noseed = k[1].is_some_and(|n| n.int_value() != 0);
                let scrape = k[2].is_some_and(|n| n.int_value() != 0);
                // a full storage withholds the token, so announces spill over to neighbours
                let full =
                    storage.get_peers(&self.settings, &info_hash, noseed, scrape, &m.addr.ip(), e.key("r"));
                if !full {
                    *e.key("r").key("token") = Entry::Bytes(self.generate_token(&m.addr, &info_hash));
                }
            }
            b"find_node" => {
                const DESC: [KeyDesc; 2] =
                    [key("target", NodeType::Str, 20, 0), key("want", NodeType::List, 0, OPTIONAL)];
                let k = match verify_message(arg_ent, &DESC) {
                    Ok(k) => k,
                    Err(err) => return incoming_error(e, &err, 203),
                };
                let target = Sha1Hash::from_slice(k[0].expect("required").string_value()).expect("20");
                self.write_nodes_entries(&target, k[1], e.key("r"), other.as_deref_mut());
            }
            b"announce_peer" => {
                const DESC: [KeyDesc; 6] = [
                    key("info_hash", NodeType::Str, 20, 0),
                    key("port", NodeType::Int, 0, 0),
                    key("token", NodeType::Str, 0, 0),
                    key("n", NodeType::Str, 0, OPTIONAL),
                    key("seed", NodeType::Int, 0, OPTIONAL),
                    key("implied_port", NodeType::Int, 0, OPTIONAL),
                ];
                let k = match verify_message(arg_ent, &DESC) {
                    Ok(k) => k,
                    Err(err) => return incoming_error(e, &err, 203),
                };
                let mut port = k[1].expect("required").int_value();
                // use the packet's source port instead of the given one?
                if k[5].is_some_and(|n| n.int_value() != 0) {
                    port = m.addr.port() as i64;
                }
                // port 0 cannot be connected to (SEC-11; libtorrent stores it)
                if !(1..65536).contains(&port) {
                    return incoming_error(e, "invalid port", 203);
                }
                let info_hash = Sha1Hash::from_slice(k[0].expect("required").string_value()).expect("20");
                if !self.verify_token(k[2].expect("required").string_value(), &info_hash, &m.addr) {
                    return incoming_error(e, "invalid token", 203);
                }
                // a correct token means it isn't spoofing its address: let it into the table
                self.table.node_seen(&id, m.addr, 0xffff, filter, now);
                let addr = SocketAddr::new(m.addr.ip(), port as u16);
                let name = k[3].map(|n| n.string_value()).unwrap_or_default();
                let seed = k[4].is_some_and(|n| n.int_value() != 0);
                storage.announce_peer(&self.settings, &info_hash, addr, name, seed, now);
            }
            b"sample_infohashes" => {
                // BEP 51: a sample of the info-hashes we store, with nodes as find_node gives
                const DESC: [KeyDesc; 2] =
                    [key("target", NodeType::Str, 20, 0), key("want", NodeType::List, 0, OPTIONAL)];
                let k = match verify_message(arg_ent, &DESC) {
                    Ok(k) => k,
                    Err(err) => return incoming_error(e, &err, 203),
                };
                let target = Sha1Hash::from_slice(k[0].expect("required").string_value()).expect("20");
                storage.get_infohashes_sample(&self.settings, now, e.key("r"));
                self.write_nodes_entries(&target, k[1], e.key("r"), other.as_deref_mut());
            }
            _ => {
                // unknown (or not yet supported: BEP 44) queries with a target or info_hash
                // are answered like find_node, to be future compatible
                let target = arg_ent
                    .dict_find_string("target")
                    .filter(|n| n.string_length() == 20)
                    .or_else(|| arg_ent.dict_find_string("info_hash").filter(|n| n.string_length() == 20));
                let Some(target) = target else {
                    return incoming_error(e, "unknown message", 203);
                };
                let target = Sha1Hash::from_slice(target.string_value()).expect("20");
                if query == b"get" {
                    // BEP 44 nodes expect a token with a `get` reply
                    *e.key("r").key("token") = Entry::Bytes(self.generate_token(&m.addr, &target));
                }
                let want = arg_ent.dict_find_list("want");
                self.write_nodes_entries(&target, want, e.key("r"), other);
            }
        }
    }

    /// `nodes` / `nodes6` for `target`: our family by default, or what `want` lists.
    fn write_nodes_entries(
        &mut self,
        target: &Sha1Hash,
        want: Option<BNode<'_>>,
        r: &mut Entry,
        other: Option<&mut Node>,
    ) {
        let Some(want) = want.filter(BNode::is_list) else {
            let n = self.table.find_node(target, false, 0);
            *r.key(self.protocol_nodes_key()) = Entry::Bytes(write_nodes_entry(&n));
            return;
        };
        let mut other = other;
        for w in want.list_iter() {
            if !w.is_string() {
                continue;
            }
            let w = w.string_value();
            if w == self.protocol_family_name().as_bytes() {
                let n = self.table.find_node(target, false, 0);
                *r.key(self.protocol_nodes_key()) = Entry::Bytes(write_nodes_entry(&n));
            } else if let Some(o) = other.as_deref_mut().filter(|o| w == o.protocol_family_name().as_bytes())
            {
                let n = o.table.find_node(target, false, 0);
                *r.key(o.protocol_nodes_key()) = Entry::Bytes(write_nodes_entry(&n));
            }
        }
    }

    /// Every 5 seconds: refresh our own neighbourhood while the table is shallow, else ping
    /// the least recently queried node.
    pub fn tick(&mut self, sm: &mut dyn SocketManager, now: Instant) {
        let stale = self.last_self_refresh.is_none_or(|t| t + Duration::from_secs(600) < now);
        if stale && self.table.depth() < 4 {
            let mut target = self.id;
            make_id_secret(&mut target);
            let t = self.traversals.insert(Traversal::new(target, TraversalKind::Bootstrap));
            self.start(sm, t, now);
            self.last_self_refresh = Some(now);
            return;
        }
        let Some(ne) = self.table.next_refresh(now) else { return };
        if ne.id == self.id {
            return;
        }
        let bucket = (159 - distance_exp(&self.id, &ne.id)) as usize;
        self.send_single_refresh(sm, ne.ep(), bucket, ne.id, now);
    }

    fn send_single_refresh(
        &mut self,
        sm: &mut dyn SocketManager,
        ep: SocketAddr,
        bucket: usize,
        id: NodeId,
        now: Instant,
    ) {
        // a random target within the bucket
        let mask = generate_prefix_mask(bucket as i32 + 1);
        let mut target = generate_secret_id() & !mask;
        target |= self.id & mask;
        let t = self.traversals.insert(Traversal::new(NodeId::default(), TraversalKind::Dummy));
        let o = self.new_observer(t, ep, id, ObserverKind::Ping, now);
        let mut e = Entry::new_dict();
        *e.key("y") = Entry::from("q");
        if self.table.is_full(bucket) {
            // a full bucket: just ping
            *e.key("q") = Entry::from("ping");
        } else {
            // get_peers rather than find_node; either returns nodes
            *e.key("q") = Entry::from("get_peers");
            *e.key("a").key("info_hash") = Entry::Bytes(target.0.to_vec());
        }
        self.observers.get_mut(o).flags |= FLAG_QUERIED;
        self.rpc_invoke(sm, e, ep, o, now);
        self.maybe_free_traversal(t);
    }

    /// The nodes to save in the DHT state.
    pub fn save_nodes(&self) -> Vec<SocketAddr> {
        self.table.all_nodes().map(NodeEntry::ep).collect()
    }

    #[cfg(test)]
    fn check_invariant(&self) {
        let mut referenced = 0;
        for (i, o) in self.observers.entries.iter().enumerate() {
            let Some(o) = o else { continue };
            assert!(o.in_results || o.in_transactions, "orphan observer {i}");
            referenced += 1;
        }
        assert_eq!(referenced, self.observers.len);
        for &(_, o) in &self.transactions {
            assert!(self.observers.get(o).in_transactions);
        }
    }
}

#[cfg(test)]
mod tests;
