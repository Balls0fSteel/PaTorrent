// Ported from libtorrent-rasterbar 2.0.11: src/ip_voter.cpp, include/libtorrent/ip_voter.hpp,
// src/ip_helpers.cpp (is_local), src/socket_io.cpp (hash_address), src/session_impl.cpp
// (external_address, set_external_address: the per-listen-socket voters)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the clock is passed in (sans-IO); the vote source is a plain bit mask. The session
// keeps one voter per address family (`SessionVoter`) where libtorrent keeps one per listen
// socket; an unknown address is `None` instead of the unspecified address.

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
    pub const PEER: u8 = 4;
    pub const TRACKER: u8 = 8;
    pub const ROUTER: u8 = 16;
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
struct ExternalIpEntry {
    voters: BloomFilter<16>,
    addr: IpAddr,
    sources: u8,
    num_votes: u16,
}

impl ExternalIpEntry {
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
    fn order(&self, o: &ExternalIpEntry) -> std::cmp::Ordering {
        o.num_votes.cmp(&self.num_votes).then(o.sources.cmp(&self.sources))
    }
}

/// libtorrent `ip_voter`.
#[derive(Clone, Debug)]
pub(crate) struct IpVoter {
    external_address_voters: BloomFilter<32>,
    external_addresses: Vec<ExternalIpEntry>,
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
                self.external_addresses.push(ExternalIpEntry {
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

/// libtorrent `external_ip`: our global (voted) and local (listen) address per family. A peer
/// is ranked against the address it should see us at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExternalIp {
    /// `[v4, v6]` for global peers, then for local-network peers.
    addresses: [[Option<IpAddr>; 2]; 2],
}

impl ExternalIp {
    pub fn new(
        local4: Option<IpAddr>,
        global4: Option<IpAddr>,
        local6: Option<IpAddr>,
        global6: Option<IpAddr>,
    ) -> ExternalIp {
        ExternalIp { addresses: [[global4, global6], [local4, local6]] }
    }

    /// The address `ip` sees us at: our external address of its family, or our local address
    /// if it is on a local network.
    pub fn external_address(&self, ip: IpAddr) -> Option<IpAddr> {
        self.addresses[is_local(&ip) as usize][ip.is_ipv6() as usize]
    }
}

/// The session's external-address votes (libtorrent's `listen_socket_t::external_address`):
/// one `IpVoter` per address family. The engine has one TCP listener and at most one UDP
/// socket per family (and one DHT node per family), so a voter per family is what one per
/// listen socket comes to.
#[derive(Clone, Debug)]
pub(crate) struct SessionVoter {
    voters: [IpVoter; 2],
}

impl SessionVoter {
    pub fn new(now: Instant) -> SessionVoter {
        SessionVoter { voters: [IpVoter::new(now), IpVoter::new(now)] }
    }

    /// Forgets a family's votes, as libtorrent does when the listen socket holding them closes
    /// (the engine calls this when the internet-facing addresses of the family change).
    pub fn reset(&mut self, v4: bool, now: Instant) {
        self.voters[!v4 as usize] = IpVoter::new(now);
    }

    /// The voted external address of a family.
    pub fn external_address(&self, v4: bool) -> Option<IpAddr> {
        self.voters[!v4 as usize].external_address()
    }

    /// libtorrent `session_impl::set_external_address`'s vote: `source` (of kind
    /// `source_type`) says we are `ip`. IPv4-mapped addresses count as IPv4. Returns the
    /// family's new external address if it changed.
    pub fn cast_vote(&mut self, ip: IpAddr, source_type: u8, source: IpAddr, now: Instant) -> Option<IpAddr> {
        let ip = ip.to_canonical();
        let source = source.to_canonical();
        let voter = &mut self.voters[ip.is_ipv6() as usize];
        let before = voter.external_address();
        if !voter.cast_vote(ip, source_type, source, now) {
            return None;
        }
        voter.external_address().filter(|&a| Some(a) != before)
    }

    /// libtorrent `session_impl::external_address`: the voted addresses, with the listen
    /// addresses filling in a family without one (a listener bound to a public address) and
    /// the local ones.
    pub fn external_ip(&self, listen: impl IntoIterator<Item = IpAddr>) -> ExternalIp {
        let mut ips = [[None; 2]; 2];
        for (f, v) in self.voters.iter().enumerate() {
            ips[0][f] = v.external_address();
        }
        for a in listen {
            let a = a.to_canonical();
            if a.is_unspecified() {
                continue;
            }
            let slot = &mut ips[is_local(&a) as usize][a.is_ipv6() as usize];
            if slot.is_none() {
                *slot = Some(a);
            }
        }
        ExternalIp::new(ips[1][0], ips[0][0], ips[1][1], ips[0][1])
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

    // The session voter: libtorrent's per-listen-socket voters, one per family here.
    #[test]
    fn session_voter_keeps_the_families_apart() {
        let now = Instant::now();
        let mut sv = SessionVoter::new(now);
        let (a4, a6): (IpAddr, IpAddr) = ("51.1.1.1".parse().unwrap(), "2f80::1".parse().unwrap());
        assert_eq!(sv.cast_vote(a4, source::TRACKER, rand_v4(), now), Some(a4));
        assert_eq!(sv.external_address(true), Some(a4));
        assert_eq!(sv.external_address(false), None);
        assert_eq!(sv.cast_vote(a6, source::PEER, rand_v6(), now), Some(a6));
        assert_eq!(sv.external_address(true), Some(a4), "an IPv6 vote leaves IPv4 alone");
        assert_eq!(sv.external_address(false), Some(a6));
        // a vote for the address we have changes nothing
        assert_eq!(sv.cast_vote(a4, source::PEER, rand_v4(), now), None);
        // an IPv6 source can't vote on our IPv4 address
        assert_eq!(sv.cast_vote(v4("53.3.3.3"), source::PEER, rand_v6(), now), None);
    }

    #[test]
    fn session_voter_reset_forgets_one_family() {
        let now = Instant::now();
        let mut sv = SessionVoter::new(now);
        let (a4, a6): (IpAddr, IpAddr) = ("51.1.1.1".parse().unwrap(), "2f80::1".parse().unwrap());
        sv.cast_vote(a4, source::TRACKER, rand_v4(), now);
        sv.cast_vote(a6, source::TRACKER, rand_v6(), now);
        sv.reset(true, now);
        assert_eq!(sv.external_address(true), None);
        assert_eq!(sv.external_address(false), Some(a6));
        // the next vote decides at once again
        assert_eq!(sv.cast_vote(v4("52.2.2.2"), source::PEER, rand_v4(), now), Some(v4("52.2.2.2")));
    }

    #[test]
    fn session_voter_takes_mapped_addresses_as_ipv4() {
        let now = Instant::now();
        let mut sv = SessionVoter::new(now);
        let mapped: IpAddr = "::ffff:51.1.1.1".parse().unwrap();
        let src: IpAddr = "::ffff:8.8.8.8".parse().unwrap();
        assert_eq!(sv.cast_vote(mapped, source::PEER, src, now), Some(v4("51.1.1.1")));
        assert_eq!(sv.external_address(true), Some(v4("51.1.1.1")));
        assert_eq!(sv.external_address(false), None);
    }

    #[test]
    fn session_voter_ignores_local_claims() {
        let now = Instant::now();
        let mut sv = SessionVoter::new(now);
        for a in ["192.168.1.10", "10.0.0.2", "127.0.0.1", "0.0.0.0", "100.64.0.1"] {
            assert_eq!(sv.cast_vote(v4(a), source::PEER, rand_v4(), now), None, "{a}");
        }
        assert_eq!(sv.external_address(true), None);
    }

    /// A router's address is taken at once while nothing else is known, then the votes of
    /// trackers and peers decide (the "trackers overwrite it" rule is gone).
    #[test]
    fn router_address_is_temporary_until_the_votes_decide() {
        let now = Instant::now();
        let mut sv = SessionVoter::new(now);
        let (router, real) = (v4("51.1.1.1"), v4("53.3.3.3"));
        let anyone = IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED);
        assert_eq!(sv.cast_vote(router, source::ROUTER, anyone, now), Some(router));
        // all routers are one voter: a second router vote adds nothing
        assert_eq!(sv.cast_vote(router, source::ROUTER, anyone, now), None);
        // one tracker does not overturn it
        assert_eq!(sv.cast_vote(real, source::TRACKER, rand_v4(), now), None);
        assert_eq!(sv.external_address(true), Some(router));
        // 25 votes in all, a clear majority for the peers' address: it wins
        let mut changed = None;
        for _ in 0..30 {
            if let Some(a) = sv.cast_vote(real, source::PEER, rand_v4(), now) {
                changed = Some(a);
                break;
            }
        }
        assert_eq!(changed, Some(real));
        assert_eq!(sv.external_address(true), Some(real));
    }

    /// After a settled address, a new one needs 50 votes (or 5 minutes) and a 3:2 majority.
    #[test]
    fn a_settled_address_rotates_after_enough_votes() {
        let now = Instant::now();
        let mut sv = SessionVoter::new(now);
        let (a, b) = (v4("51.1.1.1"), v4("53.3.3.3"));
        sv.cast_vote(a, source::TRACKER, rand_v4(), now);
        for _ in 0..30 {
            sv.cast_vote(a, source::PEER, rand_v4(), now);
        }
        assert_eq!(sv.external_address(true), Some(a));
        // the address moved: everyone reports b now
        let mut n = 0;
        let mut changed = None;
        while changed.is_none() && n < 200 {
            changed = sv.cast_vote(b, source::PEER, rand_v4(), now);
            n += 1;
        }
        assert_eq!(changed, Some(b));
        assert!(n > 2, "a single vote can't move a settled address ({n})");
        // after five minutes a few votes are enough
        let later = now + Duration::from_secs(301);
        let mut changed = None;
        for _ in 0..3 {
            changed = changed.or(sv.cast_vote(a, source::PEER, rand_v4(), later));
        }
        assert_eq!(changed, Some(a));
    }

    #[test]
    fn external_ip_picks_the_address_a_peer_sees() {
        let now = Instant::now();
        let mut sv = SessionVoter::new(now);
        let ext = v4("51.1.1.1");
        sv.cast_vote(ext, source::TRACKER, rand_v4(), now);
        let lan = v4("192.168.1.5");
        let e = sv.external_ip([v4("0.0.0.0"), lan, "::".parse().unwrap()]);
        assert_eq!(e.external_address(v4("8.8.8.8")), Some(ext));
        assert_eq!(e.external_address(v4("192.168.1.9")), Some(lan), "LAN peers see our LAN address");
        assert_eq!(e.external_address("2001:db8::1".parse().unwrap()), None);
        // a listener bound to a public address stands in for a family without votes
        let pub6: IpAddr = "2001:db8::5".parse().unwrap();
        let e = sv.external_ip([pub6]);
        assert_eq!(e.external_address("2001:db8::1".parse().unwrap()), Some(pub6));
        assert_eq!(ExternalIp::default().external_address(v4("8.8.8.8")), None);
    }
}
