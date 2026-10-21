// Ported from libtorrent-rasterbar 2.0.11: src/kademlia/dht_storage.cpp (the peer part of
// dht_default_storage), include/libtorrent/kademlia/dht_storage.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: only announced peers are stored; BEP 44 items come with Phase 8, so `get` / `put`
// queries are answered like `find_node`. BEP 51 samples (`get_infohashes_sample`) are ported.

//! Peers other nodes announced to us, served from `get_peers`.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use super::msg::write_endpoint;
use super::node_id::random_inclusive;
use super::settings::DhtSettings;
use crate::bencode::Entry;
use crate::net::ip_voter::hash_address;
use crate::types::Sha1Hash;
use crate::util::bloom_filter::BloomFilter;

/// How long an announce stays valid (libtorrent `announce_interval` in dht_storage.cpp).
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// BEP 51 bounds (libtorrent `sample_infohashes_interval_max`, `infohashes_sample_count_max`).
const SAMPLE_INTERVAL_MAX: i32 = 21600;
const SAMPLE_COUNT_MAX: i32 = 20;

/// The info-hashes handed out in `sample_infohashes` replies until the sample is redrawn
/// (libtorrent `infohashes_sample`).
#[derive(Debug, Default)]
struct InfohashesSample {
    samples: Vec<Sha1Hash>,
    created: Option<Instant>,
}

#[derive(Clone, Debug)]
struct PeerEntry {
    added: Instant,
    addr: SocketAddr,
    seed: bool,
}

/// Peers sort by address, then port.
fn peer_key(a: &SocketAddr) -> (IpAddr, u16) {
    (a.ip(), a.port())
}

#[derive(Clone, Debug, Default)]
struct TorrentEntry {
    name: String,
    peers4: Vec<PeerEntry>,
    peers6: Vec<PeerEntry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StorageCounters {
    pub torrents: i32,
    pub peers: i32,
}

/// libtorrent `dht_default_storage`, peers only.
#[derive(Debug, Default)]
pub struct DhtStorage {
    map: BTreeMap<Sha1Hash, TorrentEntry>,
    counters: StorageCounters,
    sample: InfohashesSample,
}

impl DhtStorage {
    #[cfg(test)]
    pub fn counters(&self) -> StorageCounters {
        self.counters
    }

    /// Adds `values` (or the `BFpe` / `BFsd` scrape filters) and the torrent name to `peers`.
    /// Returns true if we are full and should withhold the write token, so the announce spills
    /// over to our neighbours.
    pub fn get_peers(
        &self,
        s: &DhtSettings,
        info_hash: &Sha1Hash,
        noseed: bool,
        scrape: bool,
        requester: &IpAddr,
        peers: &mut Entry,
    ) -> bool {
        let Some(v) = self.map.get(info_hash) else {
            return self.map.len() as i32 >= s.max_torrents;
        };
        let peersv = if requester.is_ipv4() { &v.peers4 } else { &v.peers6 };
        if !v.name.is_empty() {
            *peers.key("n") = Entry::Bytes(v.name.clone().into_bytes());
        }
        if scrape {
            let mut downloaders = BloomFilter::<256>::default();
            let mut seeds = BloomFilter::<256>::default();
            for p in peersv {
                let iphash = hash_address(&p.addr.ip());
                if p.seed {
                    seeds.set(&iphash);
                } else {
                    downloaders.set(&iphash);
                }
            }
            *peers.key("BFpe") = Entry::Bytes(downloaders.to_bytes().to_vec());
            *peers.key("BFsd") = Entry::Bytes(seeds.to_bytes().to_vec());
        } else {
            let mut to_pick = s.max_peers_reply.max(0);
            // IPv6 addresses are 4x as big: return a quarter as many
            if !peersv.is_empty() && requester.is_ipv6() {
                to_pick /= 4;
            }
            let pe = peers.key("values").list_mut();
            let mut candidates = peersv.iter().filter(|e| !(noseed && e.seed)).count() as i32;
            to_pick = to_pick.min(candidates);
            let mut iter = peersv.iter();
            while to_pick > 0 {
                let Some(p) = iter.next() else { break };
                // a seed asking: skip seeds
                if noseed && p.seed {
                    continue;
                }
                // pick with probability <left to pick> / <left in the set>
                let r = random_inclusive(candidates as u32);
                candidates -= 1;
                if r > to_pick as u32 {
                    continue;
                }
                let mut b = Vec::with_capacity(18);
                write_endpoint(&p.addr, &mut b);
                pe.push(Entry::Bytes(b));
                to_pick -= 1;
            }
        }
        if (peersv.len() as i32) < s.max_peers {
            return false;
        }
        // at the peer limit: only a requester already in the set gets a token (matched by IP
        // only, its DHT port may differ from the announced one)
        let pos = peersv.partition_point(|p| p.addr.ip() < *requester);
        pos == peersv.len() || peersv[pos].addr.ip() != *requester
    }

    pub fn announce_peer(
        &mut self,
        s: &DhtSettings,
        info_hash: &Sha1Hash,
        endp: SocketAddr,
        name: &[u8],
        seed: bool,
        now: Instant,
    ) {
        if !self.map.contains_key(info_hash) {
            if self.map.len() as i32 >= s.max_torrents {
                return; // at capacity
            }
            self.counters.torrents += 1;
        }
        let v = self.map.entry(*info_hash).or_default();
        // keep the first name we hear
        if !name.is_empty() && v.name.is_empty() {
            let n = &name[..name.len().min(100)];
            v.name = String::from_utf8_lossy(n).into_owned();
        }
        let peersv = if endp.is_ipv4() { &mut v.peers4 } else { &mut v.peers6 };
        let peer = PeerEntry { added: now, addr: endp, seed };
        let pos = peersv.partition_point(|p| peer_key(&p.addr) < peer_key(&endp));
        if pos < peersv.len() && peersv[pos].addr == endp {
            peersv[pos] = peer;
        } else if peersv.len() as i32 >= s.max_peers {
            // at capacity: drop the announce
        } else {
            peersv.insert(pos, peer);
            self.counters.peers += 1;
        }
    }

    /// BEP 51 (libtorrent `get_infohashes_sample`): the reply's `interval`, `num` (the
    /// info-hashes stored) and `samples` (up to `max_infohashes_sample_count` of them, drawn at
    /// random and kept for `sample_infohashes_interval` seconds, concatenated). Returns how
    /// many samples it holds.
    pub fn get_infohashes_sample(&mut self, s: &DhtSettings, now: Instant, item: &mut Entry) -> usize {
        let interval = s.sample_infohashes_interval.clamp(0, SAMPLE_INTERVAL_MAX);
        *item.key("interval") = Entry::Int(i64::from(interval));
        *item.key("num") = Entry::Int(self.map.len() as i64);
        self.refresh_infohashes_sample(s, now);
        let samples: Vec<u8> = self.sample.samples.iter().flat_map(|h| *h.as_bytes()).collect();
        *item.key("samples") = Entry::Bytes(samples);
        self.sample.samples.len()
    }

    /// libtorrent `refresh_infohashes_sample`: a new sample once the interval is over, or while
    /// the sample is short of `max_infohashes_sample_count`.
    fn refresh_infohashes_sample(&mut self, s: &DhtSettings, now: Instant) {
        let interval = s.sample_infohashes_interval.clamp(0, SAMPLE_INTERVAL_MAX);
        let max_count = s.max_infohashes_sample_count.clamp(0, SAMPLE_COUNT_MAX) as usize;
        let count = max_count.min(self.map.len());
        let fresh = self.sample.created.is_some_and(|c| c + Duration::from_secs(interval as u64) > now);
        if interval > 0 && fresh && self.sample.samples.len() >= max_count {
            return;
        }
        let samples = &mut self.sample.samples;
        samples.clear();
        samples.reserve(count);
        let mut to_pick = count as u32;
        let mut candidates = self.map.len() as u32;
        for k in self.map.keys() {
            if to_pick == 0 {
                break;
            }
            // pick with probability <left to pick> / <left in the set>
            let r = random_inclusive(candidates);
            candidates -= 1;
            if r > to_pick {
                continue;
            }
            samples.push(*k);
            to_pick -= 1;
        }
        debug_assert_eq!(samples.len(), count);
        self.sample.created = Some(now);
    }

    /// Drops announces older than 45 minutes, and torrents left without peers.
    pub fn tick(&mut self, now: Instant) {
        let mut removed_peers = 0;
        self.map.retain(|_, t| {
            for v in [&mut t.peers4, &mut t.peers6] {
                let before = v.len();
                v.retain(|e| e.added + ANNOUNCE_INTERVAL * 3 / 2 >= now);
                removed_peers += (before - v.len()) as i32;
                if !v.is_empty() && v.capacity() / v.len() >= 4 {
                    v.shrink_to_fit();
                }
            }
            !t.peers4.is_empty() || !t.peers6.is_empty()
        });
        self.counters.peers -= removed_peers;
        self.counters.torrents = self.map.len() as i32;
    }

    #[cfg(test)]
    pub fn num_torrents(&self) -> usize {
        self.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::bdecode;

    fn ih(s: &str) -> Sha1Hash {
        Sha1Hash::from_slice(s.as_bytes()).unwrap()
    }

    fn ep(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn settings() -> DhtSettings {
        DhtSettings { max_torrents: 2, max_dht_items: 2, ..DhtSettings::default() }
    }

    fn get(st: &DhtStorage, s: &DhtSettings, h: &Sha1Hash, requester: &str) -> (Entry, bool) {
        let mut peers = Entry::new_dict();
        let full = st.get_peers(s, h, false, false, &requester.parse().unwrap(), &mut peers);
        (peers, full)
    }

    // libtorrent test_dht_storage.cpp: infohashes_sample
    #[test]
    fn infohashes_sample() {
        let mut s = DhtSettings {
            max_torrents: 5,
            sample_infohashes_interval: 10,
            max_infohashes_sample_count: 2,
            ..DhtSettings::default()
        };
        let mut st = DhtStorage::default();
        let now = Instant::now();
        let hashes = [
            ih("n1111111111111111111"),
            ih("n2222222222222222222"),
            ih("n3333333333333333333"),
            ih("n4444444444444444444"),
        ];
        for (i, h) in hashes.iter().enumerate() {
            st.announce_peer(&s, h, ep(&format!("124.31.75.{}:1", 21 + i)), b"torrent_name", false, now);
        }
        let mut item = Entry::new_dict();
        assert_eq!(st.get_infohashes_sample(&s, now, &mut item), 2);
        assert_eq!(item.get("interval").and_then(Entry::as_int), Some(10));
        assert_eq!(item.get("num").and_then(Entry::as_int), Some(4));
        assert_eq!(item.get("samples").and_then(Entry::as_bytes).map(<[u8]>::len), Some(2 * 20));

        // get all of them
        s.max_infohashes_sample_count = 5;
        let mut item = Entry::new_dict();
        assert_eq!(st.get_infohashes_sample(&s, now, &mut item), 4);
        assert_eq!(item.get("interval").and_then(Entry::as_int), Some(10));
        assert_eq!(item.get("num").and_then(Entry::as_int), Some(4));
        let samples = item.get("samples").and_then(Entry::as_bytes).unwrap().to_vec();
        assert_eq!(samples.len(), 4 * 20);
        for h in &hashes {
            assert!(samples.chunks(20).any(|c| c == h.as_bytes()), "{h:?}");
        }
    }

    // libtorrent test_dht_storage.cpp: infohashes_sample_dist
    #[test]
    fn infohashes_sample_dist() {
        let s = DhtSettings {
            max_torrents: 1000,
            // a new sample at every call
            sample_infohashes_interval: 0,
            max_infohashes_sample_count: 1,
            ..DhtSettings::default()
        };
        let mut st = DhtStorage::default();
        let now = Instant::now();
        for i in 0..1000u32 {
            let mut h = [0u8; 20];
            h[..4].copy_from_slice(&i.to_be_bytes());
            h[4..8].copy_from_slice(&i.wrapping_mul(0x9e37_79b9).to_be_bytes());
            let addr = SocketAddr::from(([10, (i >> 8) as u8, i as u8, 1], 6881));
            st.announce_peer(&s, &Sha1Hash::from_slice(&h).unwrap(), addr, b"torrent_name", false, now);
        }
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..1000 {
            let mut item = Entry::new_dict();
            assert_eq!(st.get_infohashes_sample(&s, now, &mut item), 1);
            assert_eq!(item.get("interval").and_then(Entry::as_int), Some(0));
            assert_eq!(item.get("num").and_then(Entry::as_int), Some(1000));
            let sample = item.get("samples").and_then(Entry::as_bytes).unwrap().to_vec();
            assert_eq!(sample.len(), 20);
            seen.insert(sample);
        }
        assert!(seen.len() > 500, "{} different samples", seen.len());
    }

    // libtorrent test_dht_storage.cpp: announce_peer, dual_stack, counters, peer_limit,
    // torrent_limit, get_peers_dist
    #[test]
    fn announce_peer() {
        let s = settings();
        let mut st = DhtStorage::default();
        let now = Instant::now();
        let (n1, n2, n3, n4) = (
            ih("01010101010101010101"),
            ih("5555555555555555555a"),
            ih("0123456789abcdef0123"),
            ih("4444444444444444444a"),
        );
        let (peers, _) = get(&st, &s, &n1, "124.31.75.21");
        assert!(peers.get("n").is_none());
        assert!(peers.get("values").is_none());

        st.announce_peer(&s, &n1, ep("124.31.75.21:1"), b"torrent_name", false, now);
        let (peers, _) = get(&st, &s, &n1, "124.31.75.21");
        assert_eq!(peers.get("n").and_then(Entry::as_str), Some("torrent_name"));
        assert_eq!(peers.get("values").and_then(Entry::as_list).map(<[Entry]>::len), Some(1));

        st.announce_peer(&s, &n2, ep("124.31.75.21:3"), b"torrent_name1", false, now);
        st.announce_peer(&s, &n2, ep("124.31.75.21:4"), b"torrent_name1", false, now);
        st.announce_peer(&s, &n3, ep("[abcd:ef01::1]:1"), b"torrent_name2", false, now);
        assert_eq!(st.num_torrents(), 2); // n3 is over the limit
        st.announce_peer(&s, &n4, ep("124.31.75.21:5"), b"torrent_name1", false, now);
        assert_eq!(st.counters(), StorageCounters { torrents: 2, peers: 3 });

        st.tick(now + Duration::from_secs(46 * 60));
        assert_eq!(st.counters(), StorageCounters { torrents: 0, peers: 0 });
    }

    #[test]
    fn dual_stack() {
        let s = settings();
        let mut st = DhtStorage::default();
        let now = Instant::now();
        let n1 = ih("01010101010101010101");
        st.announce_peer(&s, &n1, ep("124.31.75.21:1"), b"", false, now);
        st.announce_peer(&s, &n1, ep("[123::1]:1"), b"", false, now);
        st.announce_peer(&s, &n1, ep("[123::2]:1"), b"", false, now);
        let (p4, _) = get(&st, &s, &n1, "124.31.75.21");
        assert_eq!(p4.get("values").and_then(Entry::as_list).map(<[Entry]>::len), Some(1));
        let (p6, _) = get(&st, &s, &n1, "123::1");
        assert_eq!(p6.get("values").and_then(Entry::as_list).map(<[Entry]>::len), Some(2));
    }

    #[test]
    fn peer_limit() {
        let s = DhtSettings { max_peers: 42, ..settings() };
        let mut st = DhtStorage::default();
        let n1 = ih("01010101010101010101");
        let now = Instant::now();
        for i in 0..200u32 {
            let a = std::net::Ipv4Addr::from(0x0101_0101 + i * 7919);
            st.announce_peer(&s, &n1, SocketAddr::new(a.into(), (i + 1) as u16), b"", false, now);
        }
        assert_eq!(st.counters().peers, 42);
        // full: a newcomer gets no token
        assert!(get(&st, &s, &n1, "200.1.1.1").1);
    }

    #[test]
    fn torrent_limit() {
        let s = DhtSettings { max_torrents: 42, ..settings() };
        let mut st = DhtStorage::default();
        let now = Instant::now();
        for i in 0..100u32 {
            let mut h = Sha1Hash::default();
            h.0[..4].copy_from_slice(&i.to_be_bytes());
            st.announce_peer(&s, &h, ep("1.2.3.4:5"), b"", false, now);
        }
        assert_eq!(st.counters().torrents, 42);
    }

    #[test]
    fn get_peers_dist() {
        // the peers returned are a fair sample of the stored ones
        let s = settings();
        let mut st = DhtStorage::default();
        let n1 = ih("01010101010101010101");
        let now = Instant::now();
        for i in 0..150u32 {
            let a = std::net::Ipv4Addr::from(0x0b00_0001 + (i << 8));
            st.announce_peer(&s, &n1, SocketAddr::new(a.into(), 6881), b"", false, now);
        }
        let mut seen = std::collections::HashSet::new();
        for _ in 0..30 {
            let (p, _) = get(&st, &s, &n1, "1.2.3.4");
            let v = p.get("values").and_then(Entry::as_list).unwrap();
            assert_eq!(v.len(), 100);
            for e in v {
                seen.insert(e.as_bytes().unwrap().to_vec());
            }
        }
        assert_eq!(seen.len(), 150);
        let enc = get(&st, &s, &n1, "1.2.3.4").0.bencode();
        assert!(bdecode(&enc[..]).is_ok());
    }
}
