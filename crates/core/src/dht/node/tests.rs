// Ported from libtorrent-rasterbar 2.0.11: test/test_dht.cpp (the node and rpc tests)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: BEP 44 (put/get item) tests wait for Phase 8; traversal_done and read_only_node use
// get_peers lookups instead of item lookups. Log-based assertions check behaviour instead.

use std::collections::BTreeSet;

use super::*;
use crate::bencode::{Bdecoded, bdecode};
use crate::dht::msg::SIZE_DIVISIBLE;
use crate::dht::node_id::tests::to_hash;
use crate::net::ip_voter::tests::{rand_v4, rand_v6};
use crate::util::bloom_filter::BloomFilter;

#[derive(Default)]
struct MockSocket {
    sent: Vec<(SocketAddr, Entry)>,
}

impl SocketManager for MockSocket {
    fn has_quota(&mut self, _now: Instant) -> bool {
        true
    }

    fn send_packet(&mut self, _local_v4: bool, e: &mut Entry, addr: SocketAddr, _now: Instant) -> bool {
        self.sent.push((addr, e.clone()));
        true
    }
}

impl MockSocket {
    fn find(&self, ep: &SocketAddr) -> Option<usize> {
        self.sent.iter().position(|(a, _)| a == ep)
    }

    fn take(&mut self, ep: &SocketAddr) -> Option<Entry> {
        self.find(ep).map(|i| self.sent.remove(i).1)
    }
}

fn test_settings() -> DhtSettings {
    DhtSettings { max_torrents: 4, max_dht_items: 4, enforce_node_id: false, ..DhtSettings::default() }
}

struct Setup {
    node: Node,
    sm: MockSocket,
    storage: DhtStorage,
    source: SocketAddr,
    now: Instant,
}

impl Setup {
    fn new(src: SocketAddr) -> Setup {
        Setup::with(src, test_settings())
    }

    fn with(src: SocketAddr, s: DhtSettings) -> Setup {
        Setup {
            node: Node::new(src.is_ipv4(), NodeId::default(), Some(src.ip()), s),
            sm: MockSocket::default(),
            storage: DhtStorage::default(),
            source: src,
            now: Instant::now(),
        }
    }

    /// Delivers an encoded message from `ep`.
    fn deliver(&mut self, e: &Entry, ep: SocketAddr) {
        let buf = e.bencode();
        let d = bdecode(&buf[..]).expect("valid");
        let m = Msg { message: d.root().expect("root"), addr: ep };
        self.node.incoming(&mut self.sm, &mut self.storage, &m, None, self.now);
    }

    /// libtorrent `send_dht_request`: the reply, if one was sent to `ep`.
    fn request(&mut self, q: &str, ep: SocketAddr, args: Entry, t: &str) -> Option<Bdecoded<Vec<u8>>> {
        let mut e = Entry::new_dict();
        *e.key("q") = Entry::from(q);
        *e.key("t") = Entry::from(t);
        *e.key("y") = Entry::from("q");
        let mut a = args;
        if a.get("id").is_none() {
            *a.key("id") = Entry::Bytes(generate_random_id().0.to_vec());
        }
        *e.key("a") = a;
        self.deliver(&e, ep);
        self.sm.take(&ep).map(|r| bdecode(r.bencode()).expect("valid reply"))
    }

    /// libtorrent `send_dht_response`: answers `request` from `ep` with `args`.
    fn respond(&mut self, request: &Entry, ep: SocketAddr, args: Entry) {
        let mut e = Entry::new_dict();
        *e.key("y") = Entry::from("r");
        *e.key("t") = request.get("t").cloned().expect("t");
        let mut r = args;
        if r.get("id").is_none() {
            *r.key("id") = Entry::Bytes(generate_random_id().0.to_vec());
        }
        *e.key("r") = r;
        self.deliver(&e, ep);
    }
}

fn args() -> Entry {
    Entry::new_dict()
}

fn with(mut e: Entry, k: &str, v: impl Into<Entry>) -> Entry {
    *e.key(k) = v.into();
    e
}

fn bytes(b: &[u8]) -> Entry {
    Entry::Bytes(b.to_vec())
}

fn nodes_entry(nodes: &[NodeEntry]) -> Entry {
    Entry::Bytes(write_nodes_entry(nodes))
}

fn write_peers(peers: &BTreeSet<SocketAddr>) -> Entry {
    Entry::List(peers.iter().map(|p| Entry::Bytes(endpoint_to_bytes(p))).collect())
}

fn rand_ep(rand_addr: fn() -> IpAddr) -> SocketAddr {
    SocketAddr::new(rand_addr(), (crate::util::random::random_below(14000) + 1024) as u16)
}

const ERR_DESC: [KeyDesc; 2] = [key("y", NodeType::Str, 1, 0), key("e", NodeType::List, 2, SIZE_DIVISIBLE)];

const PEER1_DESC: [KeyDesc; 4] = [
    key("y", NodeType::Str, 1, 0),
    key("r", NodeType::Dict, 0, PARSE_CHILDREN),
    key("token", NodeType::Str, 0, 0),
    key("id", NodeType::Str, 20, LAST_CHILD),
];

fn error_text(d: &Bdecoded<Vec<u8>>) -> String {
    let r = d.root().unwrap();
    let k = verify_message(r, &ERR_DESC).unwrap_or_else(|e| panic!("not an error reply: {e}"));
    assert_eq!(k[0].unwrap().string_value(), b"e");
    let l = k[1].unwrap();
    assert!(l.list_at(0).unwrap().is_int());
    String::from_utf8(l.list_at(1).unwrap().string_value().to_vec()).unwrap()
}

#[test]
fn ping() {
    let mut t = Setup::new(SocketAddr::new(rand_v4(), 20));
    let src = t.source;
    let r = t.request("ping", src, args(), "10").expect("a reply");
    const PONG: [KeyDesc; 4] = [
        key("y", NodeType::Str, 1, 0),
        key("t", NodeType::Str, 2, 0),
        key("r", NodeType::Dict, 0, PARSE_CHILDREN),
        key("id", NodeType::Str, 20, LAST_CHILD),
    ];
    let k = verify_message(r.root().unwrap(), &PONG).unwrap();
    assert_eq!(k[0].unwrap().string_value(), b"r");
    assert_eq!(k[1].unwrap().string_value(), b"10");
    // BEP 42: the reply tells the querier its address
    assert_eq!(r.root().unwrap().dict_find_string_value("ip"), Some(&endpoint_to_bytes(&src)[..]));
}

#[test]
fn invalid_message() {
    let mut t = Setup::new(SocketAddr::new(rand_v4(), 20));
    let src = t.source;
    let r = t.request("find_node", src, args(), "10").expect("a reply");
    assert_eq!(error_text(&r), "missing 'target' key");
    let r = t.request("put", src, args(), "10").expect("a reply");
    assert_eq!(error_text(&r), "unknown message");
}

#[test]
fn get_peers_announce() {
    let mut t = Setup::new(SocketAddr::new(rand_v4(), 20));
    let src = t.source;
    let r = t.request("get_peers", src, with(args(), "info_hash", "01010101010101010101"), "10").unwrap();
    let k = verify_message(r.root().unwrap(), &PEER1_DESC).unwrap();
    assert_eq!(k[0].unwrap().string_value(), b"r");
    let token = k[2].unwrap().string_value().to_vec();

    let a = with(args(), "info_hash", "01010101010101010101");
    let a = with(with(with(a, "n", "test"), "token", Entry::Bytes(token)), "port", 8080);
    let r = t.request("announce_peer", src, a, "10").unwrap();
    const ANN: [KeyDesc; 3] = [
        key("y", NodeType::Str, 1, 0),
        key("r", NodeType::Dict, 0, PARSE_CHILDREN),
        key("id", NodeType::Str, 20, LAST_CHILD),
    ];
    let k = verify_message(r.root().unwrap(), &ANN).unwrap();
    assert_eq!(k[0].unwrap().string_value(), b"r");
    assert_eq!(t.storage.counters().peers, 1);

    // a bad token is refused
    let a = with(args(), "info_hash", "01010101010101010101");
    let a = with(with(a, "token", "xxxx"), "port", 8080);
    let r = t.request("announce_peer", src, a, "10").unwrap();
    assert_eq!(error_text(&r), "invalid token");
}

fn test_scrape(rand_addr: fn() -> IpAddr) {
    let mut t = Setup::new(SocketAddr::new(rand_addr(), 20));
    // 50 downloaders and 50 seeds announce
    for i in 0..100 {
        t.source = SocketAddr::new(rand_addr(), 6000);
        let src = t.source;
        let r = t.request("get_peers", src, with(args(), "info_hash", "01010101010101010101"), "10").unwrap();
        let k = verify_message(r.root().unwrap(), &PEER1_DESC).unwrap();
        let token = k[2].unwrap().string_value().to_vec();
        let a = with(args(), "info_hash", "01010101010101010101");
        let a = with(with(with(a, "n", "test"), "token", Entry::Bytes(token)), "port", 8080);
        let a = with(a, "seed", i64::from(i >= 50));
        t.request("announce_peer", src, a, "10").unwrap();
    }
    let src = t.source;
    let a = with(with(args(), "info_hash", "01010101010101010101"), "scrape", 1);
    let r = t.request("get_peers", src, a, "10").unwrap();
    const PEER2: [KeyDesc; 5] = [
        key("y", NodeType::Str, 1, 0),
        key("r", NodeType::Dict, 0, PARSE_CHILDREN),
        key("BFpe", NodeType::Str, 256, 0),
        key("BFsd", NodeType::Str, 256, 0),
        key("id", NodeType::Str, 20, LAST_CHILD),
    ];
    let k = verify_message(r.root().unwrap(), &PEER2).unwrap();
    assert_eq!(k[1].unwrap().dict_find_string_value("n"), Some(&b"test"[..]));
    let downloaders = BloomFilter::<256>::from_bytes(k[2].unwrap().string_value().try_into().unwrap());
    let seeds = BloomFilter::<256>::from_bytes(k[3].unwrap().string_value().try_into().unwrap());
    assert!((seeds.size() - 50.0).abs() <= 3.0, "seeds {}", seeds.size());
    assert!((downloaders.size() - 50.0).abs() <= 3.0, "downloaders {}", downloaders.size());
}

#[test]
fn scrape_v4() {
    test_scrape(rand_v4);
}

#[test]
fn scrape_v6() {
    test_scrape(rand_v6);
}

fn test_id_enforcement(v4: bool) {
    let s = DhtSettings { enforce_node_id: true, ..test_settings() };
    let mut t = Setup::with(SocketAddr::new(if v4 { rand_v4() } else { rand_v6() }, 20), s);
    // test vectors from http://libtorrent.org/dht_sec.html
    let (src, mut nid) = if v4 {
        ("124.31.75.21:1".parse().unwrap(), to_hash("5fbfbff10c5d6a4ec8a88e4c6ab4c28b95eee401"))
    } else {
        (
            "[2001:b829:2123:be84:e16c:d6ae:5290:49f1]:1".parse().unwrap(),
            to_hash("0a8ad123be84e16cd6ae529049f1f1bbe9ebb304"),
        )
    };
    // an invalid id for this source
    nid.0[0] = 0x18;
    let nodes_num = t.node.table.size().1;
    let a = with(args(), "target", "01010101010101010101");
    let r = t.request("find_node", src, with(a, "id", bytes(&nid.0)), "10").unwrap();
    assert_eq!(error_text(&r), "invalid node ID");
    assert_eq!(t.node.table.size().1, nodes_num);

    // now valid
    nid.0[0] = if v4 { 0x5f } else { 0x0a };
    let a = with(args(), "target", "01010101010101010101");
    let r = t.request("find_node", src, with(a, "id", bytes(&nid.0)), "10").unwrap();
    assert_eq!(r.root().unwrap().dict_find_string_value("y"), Some(&b"r"[..]));
    assert_eq!(t.node.table.size().1, nodes_num + 1);
}

#[test]
fn id_enforcement_v4() {
    test_id_enforcement(true);
}

#[test]
fn id_enforcement_v6() {
    test_id_enforcement(false);
}

const FIND_NODE_DESC: [KeyDesc; 7] = [
    key("y", NodeType::Str, 1, 0),
    key("t", NodeType::Str, 2, 0),
    key("q", NodeType::Str, 9, 0),
    key("a", NodeType::Dict, 0, PARSE_CHILDREN),
    key("id", NodeType::Str, 20, 0),
    key("target", NodeType::Str, 20, OPTIONAL),
    key("info_hash", NodeType::Str, 20, OPTIONAL | LAST_CHILD),
];

fn check_lookup_query(e: &Entry) -> Bdecoded<Vec<u8>> {
    let d = bdecode(e.bencode()).unwrap();
    let k = verify_message(d.root().unwrap(), &FIND_NODE_DESC).unwrap();
    assert_eq!(k[0].unwrap().string_value(), b"q");
    let q = k[2].unwrap().string_value();
    assert!(q == b"find_node" || q == b"get_peers");
    d
}

fn test_bootstrap(rand_addr: fn() -> IpAddr) {
    let mut t = Setup::new(SocketAddr::new(rand_addr(), 20));
    let now = t.now;
    let initial_node = SocketAddr::new(rand_addr(), 1234);
    t.node.bootstrap(&mut t.sm, &[initial_node], false, now);
    assert_eq!(t.sm.sent.len(), 1);
    assert_eq!(t.sm.sent[0].0, initial_node);
    let req = t.sm.sent.remove(0).1;
    check_lookup_query(&req);
    // router nodes are told we are bootstrapping
    assert_eq!(req.get("a").and_then(|a| a.get("bs")).and_then(Entry::as_int), Some(1));

    let found_node = SocketAddr::new(rand_addr(), 2235);
    let nodes = [NodeEntry::from_endpoint(found_node)];
    let key = if initial_node.is_ipv4() { "nodes" } else { "nodes6" };
    t.respond(&req, initial_node, with(args(), key, nodes_entry(&nodes)));
    assert_eq!(t.sm.sent.len(), 1);
    assert_eq!(t.sm.sent[0].0, found_node);
    let req = t.sm.sent.remove(0).1;
    check_lookup_query(&req);

    t.respond(&req, found_node, args());
    assert!(t.sm.sent.is_empty());
    assert_eq!(t.node.table.num_global_nodes(), 3);
    assert_eq!(t.node.num_running_lookups(), 0);
    t.node.check_invariant();
}

#[test]
fn bootstrap_v4() {
    test_bootstrap(rand_v4);
}

#[test]
fn bootstrap_v6() {
    test_bootstrap(rand_v6);
}

fn test_bootstrap_want(v4: bool) {
    let mut t = Setup::new(SocketAddr::new(if v4 { rand_v4() } else { rand_v6() }, 20));
    let now = t.now;
    let other = rand_ep(if v4 { rand_v6 } else { rand_v4 });
    t.node.bootstrap(&mut t.sm, &[other], true, now);
    assert_eq!(t.sm.sent.len(), 1);
    assert_eq!(t.sm.sent[0].0, other);
    let want = t.sm.sent[0].1.get("a").and_then(|a| a.get("want")).and_then(Entry::as_list).unwrap().to_vec();
    assert_eq!(want, [Entry::from(if v4 { "n4" } else { "n6" })]);
}

#[test]
fn bootstrap_want_v4() {
    test_bootstrap_want(true);
}

#[test]
fn bootstrap_want_v6() {
    test_bootstrap_want(false);
}

fn test_short_nodes(rand_addr: fn() -> IpAddr) {
    let mut t = Setup::new(SocketAddr::new(rand_addr(), 20));
    let now = t.now;
    let initial_node = SocketAddr::new(rand_addr(), 1234);
    t.node.bootstrap(&mut t.sm, &[initial_node], false, now);
    assert_eq!(t.sm.sent.len(), 1);
    let req = t.sm.sent.remove(0).1;
    check_lookup_query(&req);
    let found_node = SocketAddr::new(rand_addr(), 2235);
    let mut nodes = write_nodes_entry(&[NodeEntry::from_endpoint(found_node)]);
    // chop one byte off the nodes string
    nodes.remove(0);
    let key = if initial_node.is_ipv4() { "nodes" } else { "nodes6" };
    t.respond(&req, initial_node, with(args(), key, Entry::Bytes(nodes)));
    assert!(t.sm.sent.is_empty());
}

#[test]
fn short_nodes_v4() {
    test_short_nodes(rand_v4);
}

#[test]
fn short_nodes_v6() {
    test_short_nodes(rand_v6);
}

const GET_PEERS_DESC: [KeyDesc; 6] = [
    key("y", NodeType::Str, 1, 0),
    key("t", NodeType::Str, 2, 0),
    key("q", NodeType::Str, 9, 0),
    key("a", NodeType::Dict, 0, PARSE_CHILDREN),
    key("id", NodeType::Str, 20, 0),
    key("info_hash", NodeType::Str, 20, LAST_CHILD),
];

fn peers_from(events: &[DhtEvent]) -> Vec<SocketAddr> {
    events
        .iter()
        .filter_map(|e| match e {
            DhtEvent::Peers { peers, .. } => Some(peers.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn test_get_peers(rand_addr: fn() -> IpAddr) {
    let mut t = Setup::new(SocketAddr::new(rand_addr(), 20));
    let now = t.now;
    let target = to_hash("1234876923549721020394873245098347598635");
    let initial_node = SocketAddr::new(rand_addr(), 1234);
    let initial_id = to_hash("1111111111222222222233333333334444444444");
    t.node.table.add_node(NodeEntry::new(initial_id, initial_node, 10, true, now), now);
    let args_ann = AnnounceArgs { port: 1234, seed: false, implied_port: false };
    t.node.announce(&mut t.sm, target, args_ann, now);
    assert_eq!(t.sm.sent.len(), 1);
    assert_eq!(t.sm.sent[0].0, initial_node);
    let req = t.sm.sent.remove(0).1;
    let d = bdecode(req.bencode()).unwrap();
    let k = verify_message(d.root().unwrap(), &GET_PEERS_DESC).unwrap();
    assert_eq!(k[2].unwrap().string_value(), b"get_peers");
    assert_eq!(k[5].unwrap().string_value(), &target.0);

    let peers0: BTreeSet<SocketAddr> = (4111..4114).map(|p| SocketAddr::new(rand_addr(), p)).collect();
    let next_node = SocketAddr::new(rand_addr(), 2235);
    let key = if initial_node.is_ipv4() { "nodes" } else { "nodes6" };
    let a = with(args(), key, nodes_entry(&[NodeEntry::from_endpoint(next_node)]));
    let a = with(with(with(a, "token", "10"), "port", 1234), "values", write_peers(&peers0));
    t.respond(&req, initial_node, a);
    assert_eq!(t.sm.sent.len(), 1);
    assert_eq!(t.sm.sent[0].0, next_node);
    let req = t.sm.sent.remove(0).1;
    let d = bdecode(req.bencode()).unwrap();
    let k = verify_message(d.root().unwrap(), &GET_PEERS_DESC).unwrap();
    assert_eq!(k[5].unwrap().string_value(), &target.0);

    let peers1: BTreeSet<SocketAddr> = (4114..4117).map(|p| SocketAddr::new(rand_addr(), p)).collect();
    let a = with(with(with(args(), "token", "11"), "port", 1234), "values", write_peers(&peers1));
    t.respond(&req, next_node, a);
    // the lookup is done: announces go out
    assert!(!t.sm.sent.is_empty());
    for (_, p) in &t.sm.sent {
        assert_eq!(p.get("q").and_then(Entry::as_str), Some("announce_peer"));
        assert_eq!(p.get("a").and_then(|a| a.get("port")).and_then(Entry::as_int), Some(1234));
    }
    let events = t.node.take_events();
    let got = peers_from(&events);
    for p in peers0.iter().chain(peers1.iter()) {
        assert!(got.contains(p), "{p} missing");
    }
    assert!(events.iter().any(|e| matches!(e, DhtEvent::LookupDone { .. })));
    t.node.check_invariant();
}

#[test]
fn get_peers_v4() {
    test_get_peers(rand_v4);
}

#[test]
fn get_peers_v6() {
    test_get_peers(rand_v6);
}

/// Nodes at increasing closeness to `target` (libtorrent `build_nodes`).
fn build_nodes(target: &NodeId, now: Instant) -> Vec<NodeEntry> {
    let ips =
        ["1.1.1.1", "2.2.2.2", "3.3.3.3", "4.4.4.4", "5.5.5.5", "6.6.6.6", "7.7.7.7", "8.8.8.8", "9.9.9.9"];
    ips.iter()
        .enumerate()
        .map(|(i, ip)| {
            let mut id = *target;
            id.0[i] = !id.0[i];
            NodeEntry::new(id, SocketAddr::new(ip.parse().unwrap(), 1231 + i as u16), 10, true, now)
        })
        .collect()
}

// libtorrent traversal_done: done() runs once, even when the farthest node answers last
#[test]
fn traversal_done() {
    let s = DhtSettings { search_branching: 8, ..test_settings() };
    let mut t = Setup::with(SocketAddr::new(rand_v4(), 20), s);
    let now = t.now;
    let target = to_hash("1234876923549721020394873245098347598635");
    let nodes = build_nodes(&target, now);
    for n in &nodes[..8] {
        t.node.table.add_node(n.clone(), now);
    }
    let args_ann = AnnounceArgs { port: 1234, seed: false, implied_port: false };
    t.node.announce(&mut t.sm, target, args_ann, now);
    assert_eq!(t.sm.sent.len(), 8);
    let mut i = 1;
    loop {
        if i == nodes.len() {
            i = 0;
        }
        let req = t.sm.take(&nodes[i].ep()).expect("request");
        let d = bdecode(req.bencode()).unwrap();
        verify_message(d.root().unwrap(), &GET_PEERS_DESC).unwrap();
        let mut a = with(with(args(), "token", format!("{i:02}").as_str()), "port", 1234);
        a = with(a, "id", bytes(&nodes[i].id.0));
        if i == 1 {
            a = with(a, "nodes", nodes_entry(&nodes[8..9]));
        }
        t.respond(&req, nodes[i].ep(), a);
        if i == 0 {
            break;
        }
        i += 1;
    }
    let done = t.node.take_events().iter().filter(|e| matches!(e, DhtEvent::LookupDone { .. })).count();
    assert_eq!(done, 1);
    // the k closest nodes got an announce
    assert_eq!(t.sm.sent.len(), 8);
    assert!(t.sm.sent.iter().all(|(_, e)| e.get("q").and_then(Entry::as_str) == Some("announce_peer")));
    assert!(t.sm.find(&nodes[0].ep()).is_none(), "the farthest node is not announced to");
    t.node.check_invariant();
}

#[test]
fn dht_dual_stack() {
    let now = Instant::now();
    let mut node4 = Node::new(true, NodeId::default(), Some("236.0.0.1".parse().unwrap()), test_settings());
    let mut node6 = Node::new(false, NodeId::default(), Some("2002::1".parse().unwrap()), test_settings());
    let mut sm = MockSocket::default();
    let mut storage = DhtStorage::default();
    let id = to_hash("3123456789abcdef01232456789abcdef0123456");
    node4.table.node_seen(&id, "4.4.4.4:4440".parse().unwrap(), 10, now);
    node6.table.node_seen(&id, "[4::4]:4441".parse().unwrap(), 10, now);

    let mut ask = |node: &mut Node, other: &mut Node, q: &str, a: Entry, src: SocketAddr| {
        let mut e = Entry::new_dict();
        *e.key("q") = Entry::from(q);
        *e.key("t") = Entry::from("10");
        *e.key("y") = Entry::from("q");
        let mut a = a;
        *a.key("id") = Entry::Bytes(generate_random_id().0.to_vec());
        *e.key("a") = a;
        let buf = e.bencode();
        let d = bdecode(&buf[..]).unwrap();
        node.incoming(
            &mut sm,
            &mut storage,
            &Msg { message: d.root().unwrap(), addr: src },
            Some(other),
            now,
        );
        bdecode(sm.take(&src).unwrap().bencode()).unwrap()
    };
    let want =
        |a: Entry, w: &[&str]| with(a, "want", Entry::List(w.iter().map(|&s| Entry::from(s)).collect()));
    let target = "01010101010101010101";

    // the v4 node asked for v6 nodes
    let r = ask(
        &mut node4,
        &mut node6,
        "find_node",
        want(with(args(), "target", target), &["n6"]),
        "10.0.0.1:20".parse().unwrap(),
    );
    let n6 = r.root().unwrap().dict_find_dict("r").unwrap().dict_find_string_value("nodes6").unwrap();
    assert_eq!(n6.len(), 38);
    assert_eq!(&n6[..20], &id.0);
    assert_eq!(read_endpoint(&n6[20..]), Some("[4::4]:4441".parse().unwrap()));

    // the v6 node asked for v4 nodes
    let r = ask(
        &mut node6,
        &mut node4,
        "get_peers",
        want(with(args(), "info_hash", target), &["n4"]),
        "[10::1]:20".parse().unwrap(),
    );
    let n4 = r.root().unwrap().dict_find_dict("r").unwrap().dict_find_string_value("nodes").unwrap();
    assert_eq!(n4.len(), 26);
    assert_eq!(read_endpoint(&n4[20..]), Some("4.4.4.4:4440".parse().unwrap()));

    // both
    let r = ask(
        &mut node6,
        &mut node4,
        "find_node",
        want(with(args(), "target", target), &["n4", "n6"]),
        "[10::1]:20".parse().unwrap(),
    );
    let rr = r.root().unwrap().dict_find_dict("r").unwrap();
    assert_eq!(rr.dict_find_string_value("nodes").unwrap().len(), 26);
    assert_eq!(rr.dict_find_string_value("nodes6").unwrap().len(), 38);
}

#[test]
fn node_set_id() {
    let mut t = Setup::new(SocketAddr::new(rand_v4(), 20));
    let old = t.node.nid();
    t.node.update_node_id("237.0.0.1".parse().unwrap(), t.now);
    assert_ne!(old, t.node.nid());
    assert!(verify_id(&t.node.nid(), &"237.0.0.1".parse().unwrap()));
    // replies carry the new id
    let src = t.source;
    let r = t.request("ping", src, args(), "10").unwrap();
    let id = r.root().unwrap().dict_find_dict("r").unwrap().dict_find_string_value("id").unwrap();
    assert_eq!(id, &t.node.nid().0);
}

#[test]
fn read_only_node() {
    let s = DhtSettings { read_only: true, ..test_settings() };
    let mut t = Setup::with("192.168.4.1:6881".parse().unwrap(), s);
    let now = t.now;
    let source: SocketAddr = "10.0.0.1:20".parse().unwrap();
    // a read-only node answers nothing
    assert!(t.request("ping", source, args(), "10").is_none());
    assert!(t.request("get", source, with(args(), "target", "01010101010101010101"), "10").is_none());
    // and doesn't add the sender
    assert_eq!(t.node.table.size().0, 0);

    // its queries carry ro=1
    let initial_node: SocketAddr = "4.4.4.4:1234".parse().unwrap();
    let initial_id = to_hash("1111111111222222222233333333334444444444");
    t.node.table.add_node(NodeEntry::new(initial_id, initial_node, 10, true, now), now);
    t.node.get_peers(&mut t.sm, generate_random_id(), false, now);
    assert_eq!(t.sm.sent.len(), 1);
    assert_eq!(t.sm.sent[0].0, initial_node);
    assert_eq!(t.sm.sent[0].1.get("ro").and_then(Entry::as_int), Some(1));
    assert_eq!(t.node.table.size(), (1, 0, 1));

    t.sm.sent.clear();
    let mut s = t.node.settings.clone();
    s.read_only = false;
    t.node.set_settings(s);
    t.request("get", source, with(args(), "target", "01010101010101010101"), "10").expect("a reply");
    // the sender goes to the replacement bucket
    assert_eq!(t.node.table.size().1, 1);
}

#[test]
fn invalid_error_msg() {
    // a malformed error message is ignored without a reply
    let mut t = Setup::new("192.168.4.1:6881".parse().unwrap());
    let mut e = Entry::new_dict();
    *e.key("y") = Entry::from("e");
    *e.key("e") = Entry::from("Malformed Error");
    t.deliver(&e, "10.0.0.1:20".parse().unwrap());
    assert!(t.sm.sent.is_empty());
}

#[test]
fn unsorted_traversal_results() {
    let mut t = Setup::new(SocketAddr::new(rand_v4(), 20));
    let now = t.now;
    let our_id = t.node.nid();
    let tid = t.node.traversals.insert(Traversal::new(our_id, TraversalKind::Bootstrap));
    let eps: Vec<SocketAddr> = (0..10).map(|_| rand_ep(rand_v4)).collect();
    for ep in &eps {
        t.node.add_entry(tid, NodeId::default(), *ep, FLAG_INITIAL, now);
    }
    let tr = t.node.traversals.get(tid);
    assert_eq!(tr.sorted_results, 0);
    let results = tr.results.clone();
    assert_eq!(results.len(), eps.len());
    for (i, &o) in results.iter().enumerate() {
        assert_eq!(t.node.observers.get(o).addr, eps[i]);
    }
    // setting an id sorts the observer to the front
    t.node.observer_set_id(results[5], NodeId::from_slice(b"abababababababababab").unwrap());
    let tr = t.node.traversals.get(tid);
    assert_eq!(tr.sorted_results, 1);
    assert_eq!(t.node.observers.get(tr.results[0]).addr, eps[5]);
    t.node.running += 1;
    t.node.traversal_done(tid);
    assert_eq!(t.node.observers.len, 0);
}

#[test]
fn rpc_invalid_error_msg() {
    // an error reply to an outstanding request fails it
    let mut t = Setup::new("192.168.4.1:6881".parse().unwrap());
    let now = t.now;
    let source: SocketAddr = "10.0.0.1:20".parse().unwrap();
    t.node.add_node(&mut t.sm, source, now);
    assert_eq!(t.node.num_transactions(), 1);
    let tid = t.sm.sent[0].1.get("t").cloned().unwrap();
    let mut err = Entry::new_dict();
    *err.key("y") = Entry::from("e");
    *err.key("e") = Entry::from("Malformed Error");
    *err.key("t") = tid;
    t.deliver(&err, source);
    assert_eq!(t.node.num_transactions(), 0);
    assert_eq!(t.node.observers.len, 0);
    assert_eq!(t.node.table.size().0, 0);
}

#[test]
fn multi_family_ping_reply_adds_node() {
    // a node that answers a ping enters the routing table
    let mut t = Setup::new("192.168.4.1:6881".parse().unwrap());
    let now = t.now;
    let source: SocketAddr = "10.0.0.1:20".parse().unwrap();
    t.node.add_node(&mut t.sm, source, now);
    let req = t.sm.sent.remove(0).1;
    t.respond(&req, source, args());
    assert_eq!(t.node.table.size().0, 1);
    assert_eq!(t.node.observers.len, 0);
}

#[test]
fn timeouts_fail_requests() {
    let mut t = Setup::new(SocketAddr::new(rand_v4(), 20));
    let now = t.now;
    let target = generate_random_id();
    let nodes = build_nodes(&target, now);
    for n in &nodes[..8] {
        t.node.table.add_node(n.clone(), now);
    }
    t.node.get_peers(&mut t.sm, target, false, now);
    let first = t.sm.sent.len();
    assert_eq!(first, 5, "branch factor");
    // after a second, a short timeout opens more slots
    t.node.connection_timeout(&mut t.sm, now + Duration::from_millis(1100));
    assert_eq!(t.sm.sent.len(), 8);
    // 15 seconds after the last request everything has failed and the lookup ends
    t.node.connection_timeout(&mut t.sm, now + Duration::from_secs(17));
    assert_eq!(t.node.num_running_lookups(), 0);
    assert_eq!(t.node.num_transactions(), 0);
    assert_eq!(t.node.observers.len, 0);
    assert!(t.node.traversals.len == 0);
    let events = t.node.take_events();
    assert!(events.contains(&DhtEvent::LookupDone { info_hash: target, responses: 0 }));
}

#[test]
fn tick_refreshes() {
    let mut t = Setup::new(SocketAddr::new(rand_v4(), 20));
    let now = t.now;
    t.node.add_router_node("1.2.3.4:6881".parse().unwrap());
    // an empty table: the self-refresh bootstrap asks the router
    t.node.tick(&mut t.sm, now);
    assert_eq!(t.sm.sent.len(), 1);
    assert_eq!(t.sm.sent[0].0, "1.2.3.4:6881".parse::<SocketAddr>().unwrap());
    // the router is never added to the table
    let req = t.sm.sent.remove(0).1;
    t.respond(&req, "1.2.3.4:6881".parse().unwrap(), args());
    assert_eq!(t.node.table.size().0, 0);
}

#[test]
fn obfuscated_get_peers() {
    let s = DhtSettings { privacy_lookups: true, ..test_settings() };
    let mut t = Setup::with(SocketAddr::new(rand_v4(), 20), s);
    let now = t.now;
    let target = to_hash("1234876923549721020394873245098347598635");
    let initial_node = SocketAddr::new(rand_v4(), 1234);
    let initial_id = to_hash("1111111111222222222233333333334444444444");
    t.node.table.add_node(NodeEntry::new(initial_id, initial_node, 10, true, now), now);
    t.node.get_peers(&mut t.sm, target, false, now);
    assert_eq!(t.sm.sent.len(), 1);
    let req = t.sm.sent.remove(0).1;
    let ih = req.get("a").and_then(|a| a.get("info_hash")).and_then(Entry::as_bytes).unwrap().to_vec();
    // only the shared prefix (plus 3 bits) of the target is sent
    let shared = 160 - distance_exp(&initial_id, &target);
    let mask = generate_prefix_mask(shared + 3);
    assert_eq!(NodeId::from_slice(&ih).unwrap() & mask, target & mask);
    // its reply ends the obfuscated phase; the real lookup asks the same node again
    t.respond(&req, initial_node, with(args(), "id", bytes(&initial_id.0)));
    assert_eq!(t.sm.sent.len(), 1);
    let req = t.sm.sent.remove(0).1;
    assert_eq!(req.get("a").and_then(|a| a.get("info_hash")).and_then(Entry::as_bytes), Some(&target.0[..]));
    t.node.check_invariant();
}
