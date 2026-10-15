// Ported from libtorrent-rasterbar 2.0.11: src/ip_voter.cpp, include/libtorrent/ip_voter.hpp,
// src/ip_helpers.cpp (is_local), src/socket_io.cpp (hash_address)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the clock is passed in (sans-IO); the vote source is a plain bit mask.

//! External-IP voting: peers and DHT nodes tell us which address they see us at, and the
//! address with a clear majority wins.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use crate::crypto::Sha1;
use crate::types::Sha1Hash;
use crate::util::bloom_filter::BloomFilter;

/// Where a vote came from (libtorrent `session_interface::ip_source_t`).
pub(crate) mod source {
    pub const DHT: u8 = 2;
}

/// libtorrent `aux::is_local`: private, loopback, link-local and CGNAT ranges.
pub(crate) fn is_local(a: &IpAddr) -> bool {
    match a {
        IpAddr::V4(a4) => {
            let ip = u32::from(*a4);
            (ip & 0xff00_0000) == 0x0a00_0000
                || (ip & 0xfff0_0000) == 0xac10_0000
                || (ip & 0xffff_0000) == 0xc0a8_0000
                || (ip & 0xffff_0000) == 0xa9fe_0000
                || (ip & 0xff00_0000) == 0x7f00_0000
                || (ip & 0xffc0_0000) == 0x6440_0000
        }
        IpAddr::V6(a6) => {
            let s = a6.segments();
            let b = a6.octets();
            a6.is_loopback()
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || (s[0] & 0xffc0) == 0xfec0 // site local
                || (b[0] == 0xff && (b[1] & 0x0f) == 0x02) // multicast link local
                || (b[0] == 0xff && (b[1] & 0x0f) == 0x05) // multicast site local
                || (b[0] & 0xfe) == 0xfc
        }
    }
}

/// libtorrent `hash_address`: SHA-1 of the address bytes.
pub(crate) fn hash_address(a: &IpAddr) -> Sha1Hash {
    match a {
        IpAddr::V4(a4) => Sha1::digest(&a4.octets()),
        IpAddr::V6(a6) => Sha1::digest(&a6.octets()),
    }
}

#[derive(Clone, Debug)]
struct ExternalIp {
    voters: BloomFilter<16>,
    addr: IpAddr,
    sources: u8,
    num_votes: u16,
}

impl ExternalIp {
    fn add_vote(&mut self, k: &Sha1Hash, source: u8) -> bool {
        self.sources |= source;
        if self.voters.find(k) {
            return false;
        }
        self.voters.set(k);
        self.num_votes += 1;
        true
    }

    /// Sorts descending by votes, then by sources.
    fn order(&self, o: &ExternalIp) -> std::cmp::Ordering {
        o.num_votes.cmp(&self.num_votes).then(o.sources.cmp(&self.sources))
    }
}

/// libtorrent `ip_voter`.
#[derive(Clone, Debug)]
pub(crate) struct IpVoter {
    external_address_voters: BloomFilter<32>,
    external_addresses: Vec<ExternalIp>,
    external_address: Option<IpAddr>,
    total_votes: u32,
    valid_external: bool,
    last_rotate: Instant,
}

impl IpVoter {
    pub fn new(now: Instant) -> IpVoter {
        IpVoter {
            external_address_voters: BloomFilter::default(),
            external_addresses: Vec::new(),
            external_address: None,
            total_votes: 0,
            valid_external: false,
            last_rotate: now,
        }
    }

    pub fn external_address(&self) -> Option<IpAddr> {
        self.external_address
    }

    /// Returns true if our external IP changed.
    fn maybe_rotate(&mut self, now: Instant) -> bool {
        if self.total_votes < 50
            && (now.duration_since(self.last_rotate) < Duration::from_secs(300) || self.total_votes == 0)
            && self.valid_external
        {
            return false;
        }
        if self.external_addresses.is_empty() {
            return false;
        }
        if self.external_addresses.len() == 1 {
            // avoid flapping: we need more votes to change our mind
            if self.external_addresses[0].num_votes < 2 {
                return false;
            }
        } else {
            self.external_addresses.sort_by(|a, b| a.order(b));
            if self.external_addresses[0].num_votes as u32 * 2 / 3
                <= self.external_addresses[1].num_votes as u32
            {
                return false;
            }
        }
        let addr = self.external_addresses[0].addr;
        let changed = self.external_address != Some(addr);
        self.external_address = Some(addr);
        self.external_address_voters.clear();
        self.total_votes = 0;
        self.external_addresses.clear();
        self.last_rotate = now;
        self.valid_external = true;
        changed
    }

    /// Returns true if a different IP is the top vote now.
    pub fn cast_vote(&mut self, ip: IpAddr, source_type: u8, source: IpAddr, now: Instant) -> bool {
        if ip.is_unspecified() || is_local(&ip) || ip.is_loopback() {
            return false;
        }
        // don't trust sources on a different address family than the IP they claim we have
        if ip.is_ipv4() != source.is_ipv4() {
            return false;
        }
        let k = hash_address(&source);
        let i = match self.external_addresses.iter().position(|e| e.addr == ip) {
            Some(i) => i,
            None => {
                // each IP only gets to add a new IP once
                if self.external_address_voters.find(&k) {
                    return self.maybe_rotate(now);
                }
                if self.external_addresses.len() > 40 {
                    if crate::util::random::random_below(2) == 1 {
                        return self.maybe_rotate(now);
                    }
                    // stable: oldest of the lowest-voted go last
                    self.external_addresses.sort_by(|a, b| a.order(b));
                    self.external_addresses.pop();
                }
                self.external_addresses.push(ExternalIp {
                    voters: BloomFilter::default(),
                    addr: ip,
                    sources: 0,
                    num_votes: 0,
                });
                self.external_addresses.len() - 1
            }
        };
        if !self.external_addresses[i].add_vote(&k, source_type) {
            return self.maybe_rotate(now);
        }
        self.total_votes += 1;
        if self.valid_external {
            return self.maybe_rotate(now);
        }
        let best = self
            .external_addresses
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.order(b.1).then(a.0.cmp(&b.0)))
            .map(|(_, e)| e.addr)
            .expect("non-empty");
        if Some(best) == self.external_address {
            return self.maybe_rotate(now);
        }
        if self.external_address.is_some() {
            // a temporary external address: decide after 25 votes
            return if self.total_votes >= 25 { self.maybe_rotate(now) } else { false };
        }
        self.external_address = Some(best);
        true
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::util::random::random_u64;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// A random public IPv4 address (libtorrent test `rand_v4`).
    pub(crate) fn rand_v4() -> IpAddr {
        loop {
            let ip = IpAddr::V4(Ipv4Addr::from(random_u64() as u32));
            if !is_local(&ip) && !ip.is_unspecified() && !ip.is_multicast() {
                return ip;
            }
        }
    }

    pub(crate) fn rand_v6() -> IpAddr {
        loop {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&random_u64().to_le_bytes());
            b[8..].copy_from_slice(&random_u64().to_le_bytes());
            let ip = IpAddr::V6(Ipv6Addr::from(b));
            if !is_local(&ip) && !ip.is_unspecified() && !ip.is_multicast() {
                return ip;
            }
        }
    }

    fn v4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    // libtorrent test_ip_voter.cpp
    #[test]
    fn test_random() {
        let now = Instant::now();
        let mut ipv = IpVoter::new(now);
        let addr1 = v4("51.41.61.132");
        assert!(ipv.cast_vote(addr1, source::DHT, rand_v4(), now));
        assert_eq!(ipv.external_address(), Some(addr1));
        for _ in 0..1000 {
            assert!(!ipv.cast_vote(rand_v4(), source::DHT, rand_v4(), now));
        }
        assert_eq!(ipv.external_address(), Some(addr1));
    }

    #[test]
    fn two_ips() {
        let now = Instant::now();
        let mut ipv = IpVoter::new(now);
        let (addr1, addr2) = (v4("51.1.1.1"), v4("53.3.3.3"));
        assert!(ipv.cast_vote(addr1, source::DHT, rand_v4(), now));
        for _ in 0..1000 {
            assert!(!ipv.cast_vote(addr2, source::DHT, rand_v4(), now));
            assert!(!ipv.cast_vote(rand_v4(), source::DHT, rand_v4(), now));
            assert!(!ipv.cast_vote(addr1, source::DHT, rand_v4(), now));
            assert_eq!(ipv.external_address(), Some(addr1));
        }
    }

    #[test]
    fn one_ip() {
        let now = Instant::now();
        let mut ipv = IpVoter::new(now);
        let start = v4("93.12.63.174");
        let (addr1, addr2) = (v4("51.1.1.1"), v4("53.3.3.3"));
        assert!(ipv.cast_vote(start, source::DHT, rand_v4(), now));
        assert_eq!(ipv.external_address(), Some(start));
        for _ in 0..30 {
            if ipv.cast_vote(addr2, source::DHT, rand_v4(), now)
                || ipv.cast_vote(rand_v4(), source::DHT, rand_v4(), now)
                || ipv.cast_vote(addr1, source::DHT, rand_v4(), now)
                || ipv.cast_vote(addr1, source::DHT, rand_v4(), now)
            {
                break;
            }
        }
        assert_eq!(ipv.external_address(), Some(addr1));
        for _ in 0..500 {
            assert!(!ipv.cast_vote(addr2, source::DHT, rand_v4(), now));
            assert!(!ipv.cast_vote(rand_v4(), source::DHT, rand_v4(), now));
            assert!(!ipv.cast_vote(addr1, source::DHT, rand_v4(), now));
            assert!(!ipv.cast_vote(addr1, source::DHT, rand_v4(), now));
        }
        assert_eq!(ipv.external_address(), Some(addr1));
    }

    #[test]
    fn ip_voter_1() {
        let now = Instant::now();
        let mut ipv = IpVoter::new(now);
        let (real, malicious) = (v4("5.5.5.5"), v4("4.4.4.4"));
        for _ in 0..50 {
            ipv.cast_vote(real, source::DHT, rand_v4(), now);
            ipv.cast_vote(rand_v4(), source::DHT, malicious, now);
        }
        assert_eq!(ipv.external_address(), Some(real));
    }

    #[test]
    fn ip_voter_2() {
        let now = Instant::now();
        let (mut ipv4, mut ipv6) = (IpVoter::new(now), IpVoter::new(now));
        let (malicious, real, mal_ext) = (v4("4.4.4.4"), v4("5.5.5.5"), v4("3.3.3.3"));
        let malicious2: IpAddr = "2f90::".parse().unwrap();
        let real2: IpAddr = "2f80::".parse().unwrap();
        let mal_ext2: IpAddr = "2f70::".parse().unwrap();
        for _ in 0..50 {
            ipv4.cast_vote(real, source::DHT, rand_v4(), now);
            ipv4.cast_vote(mal_ext, source::DHT, malicious, now);
            ipv6.cast_vote(real2, source::DHT, rand_v6(), now);
            ipv6.cast_vote(mal_ext2, source::DHT, malicious2, now);
        }
        assert_eq!(ipv4.external_address(), Some(real));
        assert_eq!(ipv6.external_address(), Some(real2));
    }

    #[test]
    fn local_addresses() {
        for a in ["10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.0.1", "127.0.0.1", "100.64.1.1"] {
            assert!(is_local(&v4(a)), "{a}");
        }
        for a in ["8.8.8.8", "172.32.0.1", "100.128.0.1"] {
            assert!(!is_local(&v4(a)), "{a}");
        }
        for a in ["::1", "fe80::1", "fec0::1", "fd00::1", "ff02::1"] {
            assert!(is_local(&a.parse().unwrap()), "{a}");
        }
        assert!(!is_local(&"2001:db8::1".parse().unwrap()));
    }
}
