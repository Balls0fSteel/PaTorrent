// Ported from libtorrent-rasterbar 2.0.11: src/peer_list.cpp, src/torrent_peer.cpp
// (peer_priority / BEP 40)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a Vec plus an address index instead of a sorted vector of pooled `torrent_peer`
// pointers. Connect candidates are chosen by a scan with libtorrent's ordering (fewest failures,
// then BEP 40 rank); the list is capped at `max_peerlist_size` by evicting failed peers first.

//! Every peer a torrent knows about, connected or not.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use crate::crypto::crc32c::Crc32c;
use crate::status::PeerSource;

/// libtorrent `max_failcount` default.
const MAX_FAILCOUNT: u8 = 3;
/// libtorrent `min_reconnect_time` default, multiplied by the fail count.
const MIN_RECONNECT_TIME: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(crate) struct KnownPeer {
    pub addr: SocketAddr,
    pub source: PeerSource,
    pub failcount: u8,
    /// Connection id while connected.
    pub connection: Option<u64>,
    pub last_connected: Option<Instant>,
    pub seed: bool,
    pub banned: bool,
    /// Try an encrypted handshake first (libtorrent `torrent_peer::pe_support`): set by a PEX
    /// `e` flag or a completed encrypted handshake, toggled as `EncPolicy::Enabled` alternates.
    pub pe_support: bool,
    /// BEP 40 rank against our external address (higher connects first).
    rank: u32,
}

#[derive(Default)]
pub(crate) struct PeerList {
    peers: Vec<KnownPeer>,
    index: HashMap<SocketAddr, usize>,
}

impl PeerList {
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn num_seeds(&self) -> usize {
        self.peers.iter().filter(|p| p.seed).count()
    }

    pub fn get(&self, addr: &SocketAddr) -> Option<&KnownPeer> {
        self.index.get(addr).map(|&i| &self.peers[i])
    }

    fn get_mut(&mut self, addr: &SocketAddr) -> Option<&mut KnownPeer> {
        self.index.get(addr).map(|&i| &mut self.peers[i])
    }

    /// libtorrent `peer_list::add_peer`. Returns false if the peer was rejected (full list,
    /// invalid address).
    pub fn add(
        &mut self,
        addr: SocketAddr,
        source: PeerSource,
        external_ip: Option<IpAddr>,
        max_size: usize,
    ) -> bool {
        if addr.port() == 0 || addr.ip().is_unspecified() || addr.ip().is_multicast() {
            return false;
        }
        if let Some(p) = self.get_mut(&addr) {
            p.source |= source;
            return true;
        }
        if self.peers.len() >= max_size && !self.evict_one() {
            return false;
        }
        // libtorrent ranks against our external address with the peer's port family.
        let rank = external_ip.map_or(0, |ip| peer_priority(SocketAddr::new(ip, 0), addr));
        self.index.insert(addr, self.peers.len());
        self.peers.push(KnownPeer {
            addr,
            source,
            failcount: 0,
            connection: None,
            last_connected: None,
            seed: false,
            banned: false,
            pe_support: false,
            rank,
        });
        true
    }

    /// Drops the least useful unconnected peer (libtorrent `erase_peers`).
    fn evict_one(&mut self) -> bool {
        let victim = self
            .peers
            .iter()
            .enumerate()
            .filter(|(_, p)| p.connection.is_none())
            .max_by_key(|(_, p)| (p.banned, p.failcount, std::cmp::Reverse(p.rank)))
            .map(|(i, _)| i);
        match victim {
            Some(i) => {
                self.remove_at(i);
                true
            }
            None => false,
        }
    }

    fn remove_at(&mut self, i: usize) {
        let p = self.peers.swap_remove(i);
        self.index.remove(&p.addr);
        if i < self.peers.len() {
            self.index.insert(self.peers[i].addr, i);
        }
    }

    /// libtorrent `find_connect_candidates`: up to `n` peers to connect to now.
    pub fn connect_candidates(&self, n: usize, now: Instant) -> Vec<SocketAddr> {
        let mut c: Vec<&KnownPeer> = self
            .peers
            .iter()
            .filter(|p| p.connection.is_none() && !p.banned && p.failcount < MAX_FAILCOUNT)
            .filter(|p| {
                p.last_connected
                    .is_none_or(|t| now.duration_since(t) >= MIN_RECONNECT_TIME * (p.failcount as u32 + 1))
            })
            .collect();
        c.sort_by_key(|p| (p.failcount, std::cmp::Reverse(p.rank), p.last_connected));
        c.into_iter().take(n).map(|p| p.addr).collect()
    }

    /// A connection to or from `addr` started. Incoming peers are added on the fly.
    pub fn connecting(&mut self, addr: SocketAddr, conn: u64, source: PeerSource, now: Instant) {
        if self.get(&addr).is_none() {
            // Bypass the size cap for connected peers, as libtorrent does for incoming ones.
            self.index.insert(addr, self.peers.len());
            self.peers.push(KnownPeer {
                addr,
                source,
                failcount: 0,
                connection: None,
                last_connected: None,
                seed: false,
                banned: false,
                pe_support: false,
                rank: 0,
            });
        }
        let p = self.get_mut(&addr).expect("just added");
        p.connection = Some(conn);
        p.last_connected = Some(now);
    }

    /// The connection closed. `failed` counts against the peer (could not connect, protocol
    /// error); a clean disconnect resets its fail count. With `fast_reconnect` (an encrypted
    /// attempt the peer closed on us, libtorrent `connection_closed`) a clean close also forgets
    /// the connect time, so the peer is a candidate again at once, for the plaintext retry.
    pub fn disconnected(&mut self, addr: &SocketAddr, failed: bool, seed: bool, fast_reconnect: bool) {
        if let Some(p) = self.get_mut(addr) {
            p.connection = None;
            p.seed |= seed;
            if failed {
                p.failcount = p.failcount.saturating_add(1);
            } else {
                p.failcount = 0;
                if fast_reconnect {
                    p.last_connected = None;
                }
            }
        }
    }

    pub fn pe_support(&self, addr: &SocketAddr) -> bool {
        self.get(addr).is_some_and(|p| p.pe_support)
    }

    pub fn set_pe_support(&mut self, addr: &SocketAddr, pe_support: bool) {
        if let Some(p) = self.get_mut(addr) {
            p.pe_support = pe_support;
        }
    }

    /// Never connect to this peer again (bad hashes, or it turned out to be ourselves).
    pub fn ban(&mut self, addr: &SocketAddr) {
        if let Some(p) = self.get_mut(addr) {
            p.banned = true;
        }
    }

    pub fn set_seed(&mut self, addr: &SocketAddr, seed: bool) {
        if let Some(p) = self.get_mut(addr) {
            p.seed = seed;
        }
    }

    /// Up to `n` peers worth persisting in resume data (connectable, not failing).
    pub fn resume_peers(&self, n: usize) -> Vec<SocketAddr> {
        self.peers
            .iter()
            .filter(|p| !p.banned && p.failcount == 0 && !p.source.contains(PeerSource::INCOMING))
            .take(n)
            .map(|p| p.addr)
            .collect()
    }

    /// Drops unconnected peers beyond `max` (libtorrent `max_paused_peerlist_size`).
    pub fn trim(&mut self, max: usize) {
        while self.peers.len() > max && self.evict_one() {}
    }
}

/// BEP 40 canonical peer priority (libtorrent `peer_priority`): CRC32-C of the two masked
/// addresses in ascending order (or of the two ports, if the addresses are equal), so both ends
/// compute the same rank.
pub(crate) fn peer_priority(e1: SocketAddr, e2: SocketAddr) -> u32 {
    let (e1, e2) = if e1 > e2 { (e2, e1) } else { (e1, e2) };
    let mut c = Crc32c::new();
    if e1.ip() == e2.ip() {
        let (p1, p2) = (e1.port().min(e2.port()), e1.port().max(e2.port()));
        c.update(&p1.to_be_bytes());
        c.update(&p2.to_be_bytes());
        return c.checksum();
    }
    match (e1.ip(), e2.ip()) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            const V4MASK: [[u8; 4]; 3] = [[0xff, 0xff, 0x55, 0x55], [0xff, 0xff, 0xff, 0x55], [0xff; 4]];
            let (mut b1, mut b2) = (a.octets(), b.octets());
            let mask = if b1[..2] != b2[..2] {
                0
            } else if b1[..3] != b2[..3] {
                1
            } else {
                2
            };
            for i in 0..4 {
                b1[i] &= V4MASK[mask][i];
                b2[i] &= V4MASK[mask][i];
            }
            c.update(&b1);
            c.update(&b2);
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let (mut b1, mut b2) = (a.octets(), b.octets());
            // The first 6 bytes are never masked; masking starts after the first differing
            // byte (libtorrent's loop, verbatim).
            let mut offset = usize::MAX;
            for i in 0..16 {
                if offset == usize::MAX && b1[i] != b2[i] {
                    offset = (i + 1).max(5);
                } else if i > offset {
                    b1[i] &= 0x55;
                    b2[i] &= 0x55;
                }
            }
            c.update(&b1);
            c.update(&b2);
        }
        _ => return 0,
    }
    c.checksum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(s: &str, port: u16) -> SocketAddr {
        SocketAddr::new(s.parse().unwrap(), port)
    }

    fn hash_buffer(hex: &str) -> u32 {
        let mut buf = vec![0u8; hex.len() / 2];
        assert!(crate::util::hex::decode_into(hex, &mut buf));
        crate::crypto::crc32c(&buf)
    }

    // libtorrent test_peer_priority.cpp
    #[test]
    fn bep40_peer_priority() {
        assert_eq!(
            peer_priority(ep("230.12.123.3", 0x4d2), ep("230.12.123.3", 0x12c)),
            hash_buffer("012c04d2")
        );
        assert_eq!(
            peer_priority(ep("230.12.123.1", 0x4d2), ep("230.12.123.3", 0x12c)),
            hash_buffer("e60c7b01e60c7b03")
        );
        assert_eq!(
            peer_priority(ep("230.12.23.1", 0x4d2), ep("230.12.123.3", 0x12c)),
            hash_buffer("e60c1701e60c7b01")
        );
        assert_eq!(
            peer_priority(ep("230.120.23.1", 0x4d2), ep("230.12.123.3", 0x12c)),
            hash_buffer("e60c5101e6781501")
        );
        assert_eq!(peer_priority(ep("123.213.32.10", 0), ep("98.76.54.32", 0)), 0xec2d7224);
        assert_eq!(peer_priority(ep("123.213.32.10", 0), ep("123.213.32.234", 0)), 0x99568189);

        let all = "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff";
        assert_eq!(peer_priority(ep(all, 0x4d2), ep(all, 0x12c)), hash_buffer("012c04d2"));
        assert_eq!(peer_priority(ep(all, 0x12c), ep(all, 0x4d2)), hash_buffer("012c04d2"));
        let cases = [
            (
                "ffff:0fff:ffff:ffff:ffff:ffff:ffff:ffff",
                "ffff0fffffff55555555555555555555ffffffffffff55555555555555555555",
            ),
            (
                "ffff:ffff:0fff:ffff:ffff:ffff:ffff:ffff",
                "ffffffff0fff55555555555555555555ffffffffffff55555555555555555555",
            ),
            (
                "ffff:ffff:ff0f:ffff:ffff:ffff:ffff:ffff",
                "ffffffffff0fff555555555555555555ffffffffffffff555555555555555555",
            ),
            (
                "ffff:ffff:ffff:0fff:ffff:ffff:ffff:ffff",
                "ffffffffffff0fff5555555555555555ffffffffffffffff5555555555555555",
            ),
        ];
        for (other, expected) in cases {
            assert_eq!(peer_priority(ep(all, 0x4d2), ep(other, 0x12c)), hash_buffer(expected), "{other}");
        }
    }

    #[test]
    fn candidates_failures_and_cap() {
        let mut pl = PeerList::default();
        let now = Instant::now();
        let a: SocketAddr = "10.0.0.1:1".parse().unwrap();
        let b: SocketAddr = "10.0.0.2:1".parse().unwrap();
        assert!(pl.add(a, PeerSource::TRACKER, None, 2));
        assert!(pl.add(b, PeerSource::TRACKER, None, 2));
        assert!(pl.add(a, PeerSource::PEX, None, 2), "duplicates merge sources");
        assert!(pl.get(&a).unwrap().source.contains(PeerSource::PEX));
        assert!(!pl.add("0.0.0.0:5".parse().unwrap(), PeerSource::TRACKER, None, 9));
        assert_eq!(pl.connect_candidates(5, now).len(), 2);

        pl.connecting(a, 1, PeerSource::TRACKER, now);
        assert_eq!(pl.connect_candidates(5, now), [b]);
        pl.disconnected(&a, true, false, false);
        assert_eq!(pl.connect_candidates(5, now), [b], "reconnects wait");
        assert_eq!(pl.connect_candidates(5, now + Duration::from_secs(121)), [b, a], "failed peers last");

        // Full list: the failed peer is evicted for a new one.
        let c: SocketAddr = "10.0.0.3:1".parse().unwrap();
        assert!(pl.add(c, PeerSource::TRACKER, None, 2));
        assert!(pl.get(&a).is_none() && pl.get(&b).is_some() && pl.get(&c).is_some());
        pl.ban(&b);
        assert_eq!(pl.connect_candidates(5, now), [c]);
        pl.trim(1);
        assert_eq!(pl.len(), 1);
    }
}
