// Ported from libtorrent-rasterbar 2.0.11: src/kademlia/node_id.cpp, include/libtorrent/kademlia/node_id.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the process-wide id secret is a `OnceLock`; `generate_prefix_mask` clamps instead of
// asserting on more than 160 bits.

//! Node ids, the XOR metric and BEP 42 (node ids derived from the external IP).

use std::net::IpAddr;
use std::sync::OnceLock;

use crate::crypto::{Sha1, crc32c};
use crate::net::ip_voter::is_local;
use crate::types::Sha1Hash;
use crate::util::random::{crypto_random_bytes, random_below, random_u64};

pub type NodeId = Sha1Hash;

/// The XOR distance between two ids.
pub fn distance(n1: &NodeId, n2: &NodeId) -> NodeId {
    *n1 ^ *n2
}

/// True if `distance(n1, ref) < distance(n2, ref)`.
pub fn compare_ref(n1: &NodeId, n2: &NodeId, r: &NodeId) -> bool {
    (*n1 ^ *r) < (*n2 ^ *r)
}

/// `n` in `2^n <= distance(n1, n2) < 2^(n+1)` (0 for equal ids, as libtorrent).
pub fn distance_exp(n1: &NodeId, n2: &NodeId) -> i32 {
    (159 - distance(n1, n2).count_leading_zeroes() as i32).max(0)
}

#[cfg(test)]
pub fn min_distance_exp(n1: &NodeId, ids: &[NodeId]) -> i32 {
    debug_assert!(!ids.is_empty());
    ids.iter().map(|id| distance_exp(n1, id)).min().unwrap_or(160).min(160)
}

/// libtorrent `random(max)`: uniform in `0..=max`.
pub(crate) fn random_inclusive(max: u32) -> u32 {
    random_below(max as u64 + 1) as u32
}

/// BEP 42: the id prefix derived from `ip` and the random byte `r`.
pub fn generate_id_impl(ip: &IpAddr, r: u32) -> NodeId {
    const V4MASK: [u8; 4] = [0x03, 0x0f, 0x3f, 0xff];
    const V6MASK: [u8; 8] = [0x01, 0x03, 0x07, 0x0f, 0x1f, 0x3f, 0x7f, 0xff];
    let mut buf = [0u8; 8];
    let n = match ip {
        IpAddr::V4(a) => {
            buf[..4].copy_from_slice(&a.octets());
            for i in 0..4 {
                buf[i] &= V4MASK[i];
            }
            4
        }
        IpAddr::V6(a) => {
            buf.copy_from_slice(&a.octets()[..8]);
            for i in 0..8 {
                buf[i] &= V6MASK[i];
            }
            8
        }
    };
    buf[0] |= ((r & 0x7) << 5) as u8;
    let c = crc32c(&buf[..n]);
    let mut id = NodeId::default();
    id.0[0] = (c >> 24) as u8;
    id.0[1] = (c >> 16) as u8;
    id.0[2] = (((c >> 8) & 0xf8) | random_inclusive(0x7)) as u8;
    for b in &mut id.0[3..19] {
        *b = random_u64() as u8;
    }
    id.0[19] = r as u8;
    id
}

fn secret() -> u32 {
    static SECRET: OnceLock<u32> = OnceLock::new();
    *SECRET.get_or_init(|| random_inclusive(0xffff_fffe) + 1)
}

/// Signs the last 8 bytes of `id`, so `verify_secret_id` recognises ids we made.
pub fn make_id_secret(id: &mut NodeId) {
    let s = secret();
    let rand = random_u64() as u32;
    let mut h = Sha1::new();
    h.update(&s.to_ne_bytes());
    h.update(&rand.to_ne_bytes());
    let secret_hash = h.finalize();
    id.0[16..20].copy_from_slice(&secret_hash.0[..4]);
    id.0[12..16].copy_from_slice(&rand.to_ne_bytes());
}

pub fn generate_random_id() -> NodeId {
    let mut r = [0u8; 20];
    crypto_random_bytes(&mut r);
    Sha1::digest(&r)
}

pub fn generate_secret_id() -> NodeId {
    let mut ret = generate_random_id();
    make_id_secret(&mut ret);
    ret
}

#[cfg(test)]
pub fn verify_secret_id(nid: &NodeId) -> bool {
    let mut h = Sha1::new();
    h.update(&secret().to_ne_bytes());
    h.update(&nid.0[12..16]);
    let secret_hash = h.finalize();
    nid.0[16..20] == secret_hash.0[..4]
}

/// BEP 42: true if `nid` is acceptable from `source_ip` (local addresses always are).
pub fn verify_id(nid: &NodeId, source_ip: &IpAddr) -> bool {
    if is_local(source_ip) {
        return true;
    }
    let h = generate_id_impl(source_ip, nid.0[19] as u32);
    nid.0[0] == h.0[0] && nid.0[1] == h.0[1] && (nid.0[2] & 0xf8) == (h.0[2] & 0xf8)
}

pub fn generate_id(ip: &IpAddr) -> NodeId {
    generate_id_impl(ip, random_u64() as u32)
}

#[cfg(test)]
pub fn matching_prefix(nid: &NodeId, mask: u8, prefix: u8, offset: usize) -> bool {
    let mut id = *nid;
    id <<= offset;
    (id.0[0] & mask) == prefix
}

/// An id with the top `bits` bits set.
pub fn generate_prefix_mask(bits: i32) -> NodeId {
    let bits = bits.clamp(0, 160) as usize;
    let mut mask = NodeId::default();
    let mut b = 0;
    while b + 7 < bits {
        mask.0[b / 8] = 0xff;
        b += 8;
    }
    if bits < 160 {
        mask.0[b / 8] |= ((0xffu32 << (8 - (bits & 7))) & 0xff) as u8;
    }
    mask
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::net::ip_voter::tests::rand_v4;

    pub(crate) fn to_hash(s: &str) -> NodeId {
        NodeId::from_hex(s).expect("hex")
    }

    // libtorrent test_dht.cpp: node_id_testing
    #[test]
    fn node_id_testing() {
        let mut rnd = generate_secret_id();
        assert!(verify_secret_id(&rnd));
        rnd.0[19] ^= 0x55;
        assert!(!verify_secret_id(&rnd));
        rnd = generate_random_id();
        make_id_secret(&mut rnd);
        assert!(verify_secret_id(&rnd));
    }

    // test_dht.cpp: the BEP 42 vectors from test_routing_table
    #[test]
    fn bep42_vectors() {
        let ips = ["124.31.75.21", "21.75.31.124", "65.23.51.170", "84.124.73.14", "43.213.53.83"];
        let rs = [1u32, 86, 22, 65, 90];
        let prefixes = [
            [0x5f, 0xbf, 0xbf],
            [0x5a, 0x3c, 0xe9],
            [0xa5, 0xd4, 0x32],
            [0x1b, 0x03, 0x21],
            [0xe5, 0x6f, 0x6c],
        ];
        for i in 0..5 {
            let id = generate_id_impl(&ips[i].parse().unwrap(), rs[i]);
            assert_eq!(id.0[0], prefixes[i][0]);
            assert_eq!(id.0[1], prefixes[i][1]);
            assert_eq!(id.0[2] & 0xf8, prefixes[i][2] & 0xf8);
            assert_eq!(id.0[19] as u32, rs[i]);
        }
        // test_id_enforcement's vectors
        assert!(verify_id(
            &to_hash("5fbfbff10c5d6a4ec8a88e4c6ab4c28b95eee401"),
            &"124.31.75.21".parse().unwrap()
        ));
        assert!(!verify_id(
            &to_hash("18bfbff10c5d6a4ec8a88e4c6ab4c28b95eee401"),
            &"124.31.75.21".parse().unwrap()
        ));
        assert!(verify_id(
            &to_hash("0a8ad123be84e16cd6ae529049f1f1bbe9ebb304"),
            &"2001:b829:2123:be84:e16c:d6ae:5290:49f1".parse().unwrap()
        ));
        assert!(verify_id(&generate_id(&"8.8.8.8".parse().unwrap()), &"8.8.8.8".parse().unwrap()));
        assert!(verify_id(
            &to_hash("0000000000000000000000000000000000000000"),
            &"10.0.0.1".parse().unwrap()
        ));
    }

    #[test]
    fn node_id_bucket_distribution() {
        let mut per_bucket = [0i32; 160];
        let reference = generate_id(&rand_v4());
        let samples = 100_000;
        for _ in 0..samples {
            let nid = generate_id(&rand_v4());
            per_bucket[(159 - distance_exp(&reference, &nid)) as usize] += 1;
        }
        let mut expected = samples / 2;
        for (i, &n) in per_bucket.iter().enumerate().take(25) {
            assert!((n - expected).abs() < samples / 20, "bucket {i}: {n} vs {expected}");
            expected /= 2;
        }
    }

    #[test]
    fn node_id_min_distance_exp() {
        let n1 = to_hash("0000000000000000000000000000000000000002");
        let n2 = to_hash("0000000000000000000000000000000000000004");
        let n3 = to_hash("0000000000000000000000000000000000000008");
        let min = NodeId::default();
        assert_eq!(min_distance_exp(&min, &[n1]), 1);
        assert_eq!(min_distance_exp(&min, &[n1, n1, n2]), 1);
        assert_eq!(min_distance_exp(&min, &[n1, n1, n2, n1, n2, n3]), 1);
        assert_eq!(min_distance_exp(&min, &[n3, n2, n2]), 2);
    }

    #[test]
    fn prefix_mask() {
        let tests = [
            (0, "0000000000000000000000000000000000000000"),
            (1, "8000000000000000000000000000000000000000"),
            (2, "c000000000000000000000000000000000000000"),
            (11, "ffe0000000000000000000000000000000000000"),
            (17, "ffff800000000000000000000000000000000000"),
            (37, "fffffffff8000000000000000000000000000000"),
            (160, "ffffffffffffffffffffffffffffffffffffffff"),
        ];
        for (bits, hex) in tests {
            assert_eq!(generate_prefix_mask(bits), to_hash(hex), "{bits}");
        }
        assert_eq!(generate_prefix_mask(163), NodeId::max());
    }

    #[test]
    fn distance_exp_vectors() {
        // libtorrent's last two vectors contain a non-hex 'r'; '0' gives the same leading bits.
        let tests = [
            ("ffffffffffffffffffffffffffffffffffffffff", "0000000000000000000000000000000000000000", 159),
            ("ffffffffffffffffffffffffffffffffffffffff", "7fffffffffffffffffffffffffffffffffffffff", 159),
            ("ffffffffffffffffffffffffffffffffffffffff", "ffffffffffffffffffffffffffffffffffffffff", 0),
            ("ffffffffffffffffffffffffffffffffffffffff", "fffffffffffffffffffffffffffffffffffffffe", 0),
            ("8000000000000000000000000000000000000000", "fffffffffffffffffffffffffffffffffffffffe", 158),
            ("c000000000000000000000000000000000000000", "fffffffffffffffffffffffffffffffffffffffe", 157),
            ("e000000000000000000000000000000000000000", "fffffffffffffffffffffffffffffffffffffffe", 156),
            ("f000000000000000000000000000000000000000", "fffffffffffffffffffffffffffffffffffffffe", 155),
            ("f8f2340985723049587230495872304958703294", "f7435890430890f023980f90e203980d090c3840", 155),
            (
                "ffff740985723049587230495872304958703294",
                "ffff8890430890f023980f90e203980d090c3840",
                159 - 16,
            ),
        ];
        for (a, b, e) in tests {
            assert_eq!(distance_exp(&to_hash(a), &to_hash(b)), e, "{a} {b}");
        }
    }

    #[test]
    fn prefix_matching() {
        let id = to_hash("f000000000000000000000000000000000000000");
        assert!(matching_prefix(&id, 0xf0, 0xf0, 0));
        assert!(matching_prefix(&id, 0xc0, 0xc0, 2));
        assert!(!matching_prefix(&id, 0xf0, 0xf0, 2));
    }
}
