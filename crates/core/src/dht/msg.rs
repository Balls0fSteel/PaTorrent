// Ported from libtorrent-rasterbar 2.0.11: src/kademlia/msg.cpp, include/libtorrent/kademlia/msg.hpp,
// src/kademlia/node.cpp (write_nodes_entry), src/kademlia/traversal_algorithm.cpp (look_for_nodes),
// include/libtorrent/socket_io.hpp (compact endpoints), include/libtorrent/kademlia/node_entry.hpp
// (read_node_endpoint)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: `verify_message` returns the found nodes or the error text instead of filling arrays.

//! KRPC message helpers: key validation and compact node / endpoint encodings.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::node_id::NodeId;
use super::routing_table::NodeEntry;
use crate::bencode::{Node, NodeType};

/// A key a message must (or may) have (libtorrent `key_desc_t`).
#[derive(Clone, Copy, Debug)]
pub struct KeyDesc {
    pub name: &'static str,
    /// `None` accepts any type.
    pub ty: Option<NodeType>,
    /// Required string length (or divisor with `SIZE_DIVISIBLE`); 0 for any.
    pub size: usize,
    pub flags: u8,
}

pub const OPTIONAL: u8 = 1;
/// The following keys, up to and including the next `LAST_CHILD`, are inside this dictionary.
pub const PARSE_CHILDREN: u8 = 2;
pub const LAST_CHILD: u8 = 4;
pub const SIZE_DIVISIBLE: u8 = 8;

pub const fn key(name: &'static str, ty: NodeType, size: usize, flags: u8) -> KeyDesc {
    KeyDesc { name, ty: Some(ty), size, flags }
}

/// libtorrent `verify_message`: the node for each key (`None` for a missing optional key), or
/// the error message.
pub fn verify_message<'t>(message: Node<'t>, desc: &[KeyDesc]) -> Result<Vec<Option<Node<'t>>>, String> {
    let mut ret: Vec<Option<Node<'t>>> = vec![None; desc.len()];
    if !message.is_dict() {
        return Err("not a dictionary".into());
    }
    let mut stack: Vec<Node<'t>> = vec![message];
    let mut msg = message;
    let mut i = 0;
    while i < desc.len() {
        let k = &desc[i];
        ret[i] = msg.dict_find(k.name);
        if let (Some(n), Some(ty)) = (ret[i], k.ty) {
            if n.node_type() != ty {
                ret[i] = None;
            }
        }
        if ret[i].is_none() && k.flags & OPTIONAL == 0 {
            return Err(format!("missing '{}' key", k.name));
        }
        if k.size > 0 && k.ty == Some(NodeType::Str) {
            if let Some(n) = ret[i] {
                let invalid = if k.flags & SIZE_DIVISIBLE != 0 {
                    n.string_length() % k.size != 0
                } else {
                    n.string_length() != k.size
                };
                if invalid {
                    ret[i] = None;
                    if k.flags & OPTIONAL == 0 {
                        return Err(format!("invalid value for '{}'", k.name));
                    }
                }
            }
        }
        if k.flags & PARSE_CHILDREN != 0 {
            debug_assert!(k.ty == Some(NodeType::Dict));
            match ret[i] {
                Some(n) => {
                    stack.push(n);
                    msg = n;
                }
                None => {
                    // skip all children
                    while i < desc.len() && desc[i].flags & LAST_CHILD == 0 {
                        i += 1;
                    }
                }
            }
        } else if k.flags & LAST_CHILD != 0 {
            if stack.len() <= 1 {
                return Err("unbalanced key description".into());
            }
            stack.pop();
            msg = *stack.last().expect("non-empty");
        }
        i += 1;
    }
    Ok(ret)
}

/// Compact address bytes (4 or 16).
pub fn write_address(a: &IpAddr, out: &mut Vec<u8>) {
    match a {
        IpAddr::V4(a) => out.extend_from_slice(&a.octets()),
        IpAddr::V6(a) => out.extend_from_slice(&a.octets()),
    }
}

/// Compact endpoint: address bytes then the port, big-endian.
pub fn write_endpoint(ep: &SocketAddr, out: &mut Vec<u8>) {
    write_address(&ep.ip(), out);
    out.extend_from_slice(&ep.port().to_be_bytes());
}

pub fn endpoint_to_bytes(ep: &SocketAddr) -> Vec<u8> {
    let mut v = Vec::with_capacity(18);
    write_endpoint(ep, &mut v);
    v
}

/// A compact endpoint of 6 (IPv4) or 18 (IPv6) bytes.
pub fn read_endpoint(b: &[u8]) -> Option<SocketAddr> {
    match b.len() {
        6 => Some(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3])),
            u16::from_be_bytes([b[4], b[5]]),
        )),
        18 => {
            let a: [u8; 16] = b[..16].try_into().expect("length checked");
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(a)), u16::from_be_bytes([b[16], b[17]])))
        }
        _ => None,
    }
}

/// libtorrent `read_endpoint_list`: the list's 6- and 18-byte strings as endpoints.
pub fn read_endpoint_list(n: Node<'_>) -> Vec<SocketAddr> {
    if !n.is_list() {
        return Vec::new();
    }
    n.list_iter().filter(Node::is_string).filter_map(|e| read_endpoint(e.string_value())).collect()
}

/// libtorrent `write_nodes_entry`: concatenated (id, compact endpoint) records.
pub fn write_nodes_entry(nodes: &[NodeEntry]) -> Vec<u8> {
    let mut r = Vec::with_capacity(nodes.len() * 38);
    for n in nodes {
        r.extend_from_slice(&n.id.0);
        write_endpoint(&n.ep(), &mut r);
    }
    r
}

/// libtorrent `look_for_nodes`: the (id, endpoint) records in `r[key]`. `v6` selects the
/// record size (26 or 38 bytes); a trailing partial record is ignored.
pub fn read_nodes(r: Node<'_>, key: &str, v6: bool) -> Vec<(NodeId, SocketAddr)> {
    let Some(n) = r.dict_find_string(key) else { return Vec::new() };
    let size = if v6 { 38 } else { 26 };
    n.string_value()
        .chunks_exact(size)
        .filter_map(|c| Some((NodeId::from_slice(&c[..20])?, read_endpoint(&c[20..])?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::bdecode;

    // libtorrent test_dht.cpp: verify_message
    #[test]
    fn verify() {
        let desc = [
            key("A", NodeType::Str, 4, 0),
            key("B", NodeType::Dict, 0, OPTIONAL | PARSE_CHILDREN),
            key("B1", NodeType::Str, 0, 0),
            key("B2", NodeType::Str, 0, LAST_CHILD),
            key("C", NodeType::Dict, 0, OPTIONAL | PARSE_CHILDREN),
            key("C1", NodeType::Str, 0, 0),
            key("C2", NodeType::Str, 0, LAST_CHILD),
        ];
        let d = bdecode(&b"d1:A4:test1:Bd2:B15:test22:B25:test3ee"[..]).unwrap();
        let k = verify_message(d.root().unwrap(), &desc).unwrap();
        assert_eq!(k[0].unwrap().string_value(), b"test");
        assert!(k[1].is_some());
        assert_eq!(k[2].unwrap().string_value(), b"test2");
        assert_eq!(k[3].unwrap().string_value(), b"test3");
        assert!(k[4].is_none() && k[5].is_none() && k[6].is_none());

        let d = bdecode(&b"d1:A4:test1:Cd2:C15:test22:C25:test3ee"[..]).unwrap();
        let k = verify_message(d.root().unwrap(), &desc).unwrap();
        assert_eq!(k[0].unwrap().string_value(), b"test");
        assert!(k[1].is_none() && k[2].is_none() && k[3].is_none());
        assert!(k[4].is_some());
        assert_eq!(k[5].unwrap().string_value(), b"test2");
        assert_eq!(k[6].unwrap().string_value(), b"test3");

        let d = bdecode(&b"d1:Cd2:C15:test22:C25:test3ee"[..]).unwrap();
        assert_eq!(verify_message(d.root().unwrap(), &desc).unwrap_err(), "missing 'A' key");
        let d = bdecode(&b"d1:A6:foobare"[..]).unwrap();
        assert_eq!(verify_message(d.root().unwrap(), &desc).unwrap_err(), "invalid value for 'A'");
        let d = bdecode(&b"d1:A4:test1:Cd2:C15:test2ee"[..]).unwrap();
        assert_eq!(verify_message(d.root().unwrap(), &desc).unwrap_err(), "missing 'C2' key");

        let d = bdecode(&b"ld0:i1ee0:e"[..]).unwrap();
        let r = d.root().unwrap();
        assert_eq!(r.list_size(), 2);
        assert_eq!(r.list_at(0).unwrap().dict_find_int_value(""), Some(1));
        assert_eq!(r.list_at(1).unwrap().string_value(), b"");
    }

    #[test]
    fn endpoints() {
        for s in ["1.2.3.4:6881", "[2001:db8::1]:51413"] {
            let ep: SocketAddr = s.parse().unwrap();
            assert_eq!(read_endpoint(&endpoint_to_bytes(&ep)), Some(ep));
        }
        assert_eq!(read_endpoint(&[1, 2, 3]), None);
    }
}
