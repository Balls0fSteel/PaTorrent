// Ported from libtorrent-rasterbar 2.0.11: src/kademlia/dht_state.cpp,
// include/libtorrent/kademlia/dht_state.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: none in format; the session keeps it in its own file (`dht_state.dat`).

//! The DHT state kept across restarts: our node ids and the nodes we knew.

use std::net::{IpAddr, SocketAddr};

use super::msg::{read_endpoint_list, write_address, write_endpoint};
use super::node_id::NodeId;
use crate::bencode::Entry;
use crate::bencode::{Node, NodeType};

/// libtorrent `dht_state`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DhtState {
    /// Node id per local interface address (unspecified for an old-style single id).
    pub nids: Vec<(IpAddr, NodeId)>,
    pub nodes: Vec<SocketAddr>,
    pub nodes6: Vec<SocketAddr>,
}

impl DhtState {
    pub fn clear(&mut self) {
        *self = DhtState::default();
    }
}

/// libtorrent `extract_node_ids`.
pub fn extract_node_ids(e: Node<'_>, key: &str) -> Vec<(IpAddr, NodeId)> {
    if !e.is_dict() {
        return Vec::new();
    }
    // an old-style single id
    if let Some(old) = e.dict_find_string_value(key) {
        if old.len() == 20 {
            return vec![(
                IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                NodeId::from_slice(old).expect("20 bytes"),
            )];
        }
    }
    let Some(nids) = e.dict_find_list(key) else { return Vec::new() };
    let mut ret = Vec::new();
    for nid in nids.list_iter() {
        if nid.node_type() != NodeType::Str {
            continue;
        }
        let s = nid.string_value();
        let addr: IpAddr = match s.len() {
            24 => <[u8; 4]>::try_from(&s[20..]).expect("4 bytes").into(),
            36 => <[u8; 16]>::try_from(&s[20..]).expect("16 bytes").into(),
            _ => continue,
        };
        ret.push((addr, NodeId::from_slice(&s[..20]).expect("20 bytes")));
    }
    ret
}

pub fn read_dht_state(e: Option<Node<'_>>) -> DhtState {
    let mut ret = DhtState::default();
    let Some(e) = e.filter(Node::is_dict) else { return ret };
    ret.nids = extract_node_ids(e, "node-id");
    if let Some(n) = e.dict_find_list("nodes") {
        ret.nodes = read_endpoint_list(n);
    }
    if let Some(n) = e.dict_find_list("nodes6") {
        ret.nodes6 = read_endpoint_list(n);
    }
    ret
}

fn save_nodes(nodes: &[SocketAddr]) -> Entry {
    Entry::List(
        nodes
            .iter()
            .map(|ep| {
                let mut b = Vec::with_capacity(18);
                write_endpoint(ep, &mut b);
                Entry::Bytes(b)
            })
            .collect(),
    )
}

pub fn save_dht_state(state: &DhtState) -> Entry {
    let mut ret = Entry::new_dict();
    let nids = ret.key("node-id").list_mut();
    for (addr, id) in &state.nids {
        let mut nid = id.0.to_vec();
        write_address(addr, &mut nid);
        nids.push(Entry::Bytes(nid));
    }
    if !state.nodes.is_empty() {
        *ret.key("nodes") = save_nodes(&state.nodes);
    }
    if !state.nodes6.is_empty() {
        *ret.key("nodes6") = save_nodes(&state.nodes6);
    }
    ret
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::bdecode;
    use crate::dht::node_id::tests::to_hash;

    // libtorrent test_dht.cpp: dht_state
    #[test]
    fn dht_state() {
        let s = DhtState {
            nids: vec![
                ("1.1.1.1".parse().unwrap(), to_hash("0000000000000000000000000000000000000001")),
                ("1::1".parse().unwrap(), to_hash("0000000000000000000000000000000000000002")),
            ],
            nodes: vec!["1.1.1.1:1".parse().unwrap(), "2.2.2.2:2".parse().unwrap()],
            nodes6: vec!["[1::1]:3".parse().unwrap(), "[2::2]:4".parse().unwrap()],
        };
        let enc = save_dht_state(&s).bencode();
        let d = bdecode(&enc[..]).unwrap();
        assert_eq!(read_dht_state(d.root()), s);
        assert_eq!(read_dht_state(None), DhtState::default());
        // an old-style single id
        let d = bdecode(&b"d7:node-id20:aaaaaaaaaaaaaaaaaaaae"[..]).unwrap();
        assert_eq!(read_dht_state(d.root()).nids.len(), 1);
    }
}
