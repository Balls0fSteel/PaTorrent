// Ported from libtorrent-rasterbar 2.0.11: src/peer_list.cpp, src/torrent_peer.cpp
// (peer_priority / BEP 40, the trust points and hash-fail fields)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a Vec plus an address index instead of a sorted vector of pooled `torrent_peer`
// pointers. Connect candidates are chosen by a scan with libtorrent's ordering (fewest failures,
// then BEP 40 rank); the list is capped at `max_peerlist_size` by evicting failed peers first.
// Peers restored from resume data stay in their compact saved bytes until the torrent starts.

//! Every peer a torrent knows about, connected or not.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use super::smart_ban::Trust;
use crate::crypto::crc32c::Crc32c;
use crate::net::ip_voter::ExternalIp;
use crate::resume::SavedPeers;
use crate::status::PeerSource;

/// When an unconnected peer may be tried (again): libtorrent's `max_failcount` and
/// `min_reconnect_time` settings.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reconnect {
    /// Peers that failed this often are no longer tried.
    pub max_failcount: u8,
    /// How long after its last connection closed a peer waits, times its fail count plus one.
    pub min_reconnect_time: Duration,
}

impl Reconnect {
    pub fn from_settings(s: &crate::Settings) -> Reconnect {
        Reconnect {
            max_failcount: s.max_failcount.clamp(0, i32::from(FAILCOUNT_CAP)) as u8,
            min_reconnect_time: Duration::from_secs(u64::from(s.min_reconnect_time)),
        }
    }
}

impl Default for Reconnect {
    /// libtorrent's defaults: 3 failures, 60 s.
    fn default() -> Reconnect {
        Reconnect { max_failcount: 3, min_reconnect_time: Duration::from_secs(60) }
    }
}

/// libtorrent's `torrent_peer::failcount` is five bits.
const FAILCOUNT_CAP: u8 = 31;

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
    /// Hash failures, trust points and parole (libtorrent `torrent_peer::trust_points`,
    /// `hashfails`, `on_parole`).
    pub trust: Trust,
    /// Try an encrypted handshake first (libtorrent `torrent_peer::pe_support`): set by a PEX
    /// `e` flag or a completed encrypted handshake, toggled as `EncPolicy::Enabled` alternates.
    pub pe_support: bool,
    /// We think this peer supports uTP (libtorrent `torrent_peer::supports_utp`): assumed at
    /// first, set by a PEX `utp` flag, cleared when a uTP connect fails or succeeds (then
    /// `confirmed_supports_utp` says so).
    pub supports_utp: bool,
    /// We have been connected to this peer over uTP at least once.
    pub confirmed_supports_utp: bool,
    /// Learned under a hybrid's v2 hash (a v2 tracker announce, the DHT, PEX `lt_v2`): we
    /// connect with that hash (libtorrent `torrent_peer::protocol_v2`).
    pub protocol_v2: bool,
    /// BEP 40 rank against our external address (higher connects first).
    rank: u32,
}

/// A torrent's known peers (libtorrent `peer_list`). Most torrents of a large library are
/// stopped, so the list takes the least room it can for what it holds:
///
/// * `Saved`: only the peers restored from resume data, kept as their compact bytes (6 or 18
///   each, as saved) until the torrent starts: a torrent that ran saves up to 100, which as
///   `KnownPeer`s and index entries take about 15 KB. They read as the list they stand for
///   (fresh peers from resume data: no failures, not connected, not seeds), so the status,
///   the resume data and the filters see no difference; anything that changes a peer makes the
///   list `Live` first. No peers at all is an empty `Saved`, which allocates nothing
///   (libtorrent makes `torrent::m_peer_list` in `need_peer_list`, with the first peer).
/// * `Live`: the full list, boxed.
///
/// 16 bytes (a third, `Empty` variant would make it 24).
pub(crate) enum PeerList {
    Saved(SavedPeers),
    Live(Box<Peers>),
}

impl Default for PeerList {
    fn default() -> PeerList {
        PeerList::Saved(SavedPeers::default())
    }
}

impl PeerList {
    fn live(&self) -> Option<&Peers> {
        match self {
            PeerList::Live(l) => Some(l),
            _ => None,
        }
    }

    /// The full list, made on first use; saved peers are expanded into it (unranked, as
    /// `clear_peer_prio` would rank them).
    fn live_mut(&mut self) -> &mut Peers {
        match self {
            PeerList::Saved(s) if s.is_empty() => *self = PeerList::Live(Box::default()),
            PeerList::Saved(_) => self.expand(&ExternalIp::default()),
            PeerList::Live(_) => {}
        }
        match self {
            PeerList::Live(l) => l,
            _ => unreachable!("expanded above"),
        }
    }

    /// The torrent starts: saved peers become the full list, ranked against our external
    /// address (libtorrent computes `torrent_peer::rank` on first use).
    pub fn expand(&mut self, external_ip: &ExternalIp) {
        if let PeerList::Saved(saved) = self {
            if saved.is_empty() {
                return;
            }
            let saved = std::mem::take(saved);
            let mut l = Box::<Peers>::default();
            l.peers.reserve_exact(saved.len());
            l.index.reserve(saved.len());
            for addr in saved.iter() {
                l.index.insert(addr, l.peers.len());
                l.peers.push(KnownPeer::new(addr, PeerSource::RESUME_DATA, rank(external_ip, addr)));
            }
            *self = PeerList::Live(l);
        }
    }

    /// Peers from resume data (libtorrent `add_torrent_params::peers`, added with
    /// `peer_info::resume_data` as the source). Into an empty list they go as they are.
    pub fn restore(&mut self, saved: SavedPeers) {
        if saved.is_empty() {
            return;
        }
        match self {
            PeerList::Saved(s) if s.is_empty() => *s = saved,
            _ => {
                for addr in saved.iter() {
                    self.add(addr, PeerSource::RESUME_DATA, ExternalIp::default(), usize::MAX);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        match self {
            PeerList::Saved(s) => s.len(),
            PeerList::Live(l) => l.peers.len(),
        }
    }

    pub fn num_seeds(&self) -> usize {
        self.live().map_or(0, Peers::num_seeds)
    }

    /// The peer at `addr`. Saved peers are not found until the list is expanded (the torrent
    /// starting does that): nothing asks for a peer of a stopped torrent.
    pub fn get(&self, addr: &SocketAddr) -> Option<&KnownPeer> {
        self.live()?.get(addr)
    }

    pub fn get_mut(&mut self, addr: &SocketAddr) -> Option<&mut KnownPeer> {
        match self {
            PeerList::Saved(s) if !s.iter().any(|a| a == *addr) => None,
            _ => self.live_mut().get_mut(addr),
        }
    }

    /// libtorrent `peer_list::add_peer`. Returns false if the peer was rejected (full list,
    /// invalid address).
    pub fn add(
        &mut self,
        addr: SocketAddr,
        source: PeerSource,
        external_ip: ExternalIp,
        max_size: usize,
    ) -> bool {
        if !SavedPeers::usable(&addr) {
            return false;
        }
        if matches!(self, PeerList::Saved(_)) {
            self.expand(&external_ip);
        }
        let added = self.live_mut().add(addr, source, external_ip, max_size);
        self.drop_if_empty();
        added
    }

    pub fn set_protocol_v2(&mut self, addr: &SocketAddr) {
        if let Some(p) = self.get_mut(addr) {
            p.protocol_v2 = true;
        }
    }

    /// Forgets every peer `f` picks (libtorrent `apply_ip_filter` / `apply_port_filter`).
    /// Returns how many went. A connected peer's connection is the caller's to close. Saved
    /// peers stay saved.
    pub fn remove_if(&mut self, f: impl Fn(&KnownPeer) -> bool) -> usize {
        let removed = match self {
            PeerList::Saved(s) => s.retain(|addr| !f(&KnownPeer::new(addr, PeerSource::RESUME_DATA, 0))),
            PeerList::Live(l) => l.remove_if(f),
        };
        self.drop_if_empty();
        removed
    }

    /// libtorrent `peer_list::num_connect_candidates` (`is_connect_candidate`): peers that could
    /// be connected to, now or once their reconnect delay is over. A finished torrent does not
    /// connect to seeds.
    pub fn num_connect_candidates(&self, finished: bool, policy: Reconnect) -> usize {
        match self {
            // fresh peers: none failed, none connected, none known as a seed
            PeerList::Saved(s) if policy.max_failcount > 0 => s.len(),
            PeerList::Saved(_) => 0,
            PeerList::Live(l) => l.num_connect_candidates(finished, policy),
        }
    }

    /// libtorrent `find_connect_candidates`: up to `n` peers to connect to now. A peer waits
    /// `min_reconnect_time` times its fail count plus one after its last connection.
    pub fn connect_candidates(&self, n: usize, now: Instant, policy: Reconnect) -> Vec<SocketAddr> {
        match self {
            // all equal in the ordering, so in their saved order
            PeerList::Saved(s) if policy.max_failcount > 0 => s.iter().take(n).collect(),
            PeerList::Saved(_) => Vec::new(),
            PeerList::Live(l) => l.connect_candidates(n, now, policy),
        }
    }

    /// A connection to or from `addr` started. Incoming peers are added on the fly.
    pub fn connecting(&mut self, addr: SocketAddr, conn: u64, source: PeerSource, now: Instant) {
        self.live_mut().connecting(addr, conn, source, now);
    }

    /// Up to `n` peers worth persisting in resume data (connectable, not failing).
    pub fn resume_peers(&self, n: usize) -> SavedPeers {
        match self {
            PeerList::Saved(s) => s.first(n),
            PeerList::Live(l) => SavedPeers::from_addrs(l.resume_peers(n)),
        }
    }

    /// Drops unconnected peers beyond `max` (libtorrent `max_paused_peerlist_size`). Saved
    /// peers all rank the same, and the eviction takes the last of equals: the list is cut.
    pub fn trim(&mut self, max: usize) {
        match self {
            PeerList::Saved(s) => {
                if s.len() > max {
                    *s = s.first(max);
                }
            }
            PeerList::Live(l) => l.trim(max),
        }
        self.drop_if_empty();
    }

    /// Our external address changed: rank every peer again (libtorrent `clear_peer_prio`,
    /// which makes `torrent_peer::rank` recompute on next use). Saved peers are ranked when
    /// they are expanded.
    pub fn clear_peer_prio(&mut self, external_ip: &ExternalIp) {
        if let PeerList::Live(l) = self {
            l.clear_peer_prio(external_ip);
        }
    }

    /// Frees the list once it holds no peers (a stopped torrent trimmed to none).
    fn drop_if_empty(&mut self) {
        if self.len() == 0 {
            *self = PeerList::default();
        }
    }

    /// The connection closed at `now` (libtorrent `connection_closed`), which starts the peer's
    /// reconnect delay. `failed` counts against the peer (could not connect, protocol error); a
    /// clean disconnect resets its fail count. With `fast_reconnect` (an encrypted attempt the
    /// peer closed on us) a clean close instead forgets the connect time, so the peer is a
    /// candidate again at once for the plaintext retry (libtorrent rewinds it by
    /// `min_reconnect_time * max_failcount`).
    /// The connection closed at `now` (libtorrent `connection_closed`), which starts the peer's
    /// reconnect delay. `failed` counts against the peer (could not connect, protocol error); a
    /// clean disconnect resets its fail count. With `fast_reconnect` (an encrypted attempt the
    /// peer closed on us) a clean close instead forgets the connect time, so the peer is a
    /// candidate again at once for the plaintext retry (libtorrent rewinds it by
    /// `min_reconnect_time * max_failcount`).
    pub fn disconnected(
        &mut self,
        addr: &SocketAddr,
        failed: bool,
        seed: bool,
        fast_reconnect: bool,
        now: Instant,
    ) {
        if let Some(p) = self.get_mut(addr) {
            p.connection = None;
            p.seed |= seed;
            if failed {
                p.failcount = (p.failcount + 1).min(FAILCOUNT_CAP);
                p.last_connected = Some(now);
            } else {
                p.failcount = 0;
                p.last_connected = if fast_reconnect { None } else { Some(now) };
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

    /// Whether to try uTP with this peer (libtorrent `torrent::connect_to_peer`): assumed
    /// until a uTP connect failed, or known once one succeeded.
    pub fn wants_utp(&self, addr: &SocketAddr) -> bool {
        self.get(addr).is_none_or(|p| p.supports_utp || p.confirmed_supports_utp)
    }

    /// A uTP connect failed (libtorrent `on_connection_failed`): never assume uTP again.
    /// Returns whether it was assumed, i.e. whether a TCP retry is due.
    pub fn utp_failed(&mut self, addr: &SocketAddr) -> bool {
        match self.get_mut(addr) {
            Some(p) if p.supports_utp => {
                p.supports_utp = false;
                true
            }
            _ => false,
        }
    }

    /// A uTP connection to this peer was established (libtorrent `on_connection_complete`).
    pub fn utp_confirmed(&mut self, addr: &SocketAddr) {
        if let Some(p) = self.get_mut(addr) {
            p.confirmed_supports_utp = true;
            p.supports_utp = false;
        }
    }

    /// A PEX message flagged this peer as a uTP peer.
    pub fn set_supports_utp(&mut self, addr: &SocketAddr) {
        if let Some(p) = self.get_mut(addr) {
            p.supports_utp = true;
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
}

impl KnownPeer {
    fn new(addr: SocketAddr, source: PeerSource, rank: u32) -> KnownPeer {
        KnownPeer {
            addr,
            source,
            failcount: 0,
            connection: None,
            last_connected: None,
            seed: false,
            banned: false,
            trust: Trust::default(),
            pe_support: false,
            supports_utp: true,
            confirmed_supports_utp: false,
            protocol_v2: false,
            rank,
        }
    }
}

/// The full list.
#[derive(Default)]
pub(crate) struct Peers {
    peers: Vec<KnownPeer>,
    index: HashMap<SocketAddr, usize>,
}

impl Peers {
    pub fn num_seeds(&self) -> usize {
        self.peers.iter().filter(|p| p.seed).count()
    }

    pub fn get(&self, addr: &SocketAddr) -> Option<&KnownPeer> {
        self.index.get(addr).map(|&i| &self.peers[i])
    }

    pub fn get_mut(&mut self, addr: &SocketAddr) -> Option<&mut KnownPeer> {
        self.index.get(addr).map(|&i| &mut self.peers[i])
    }

    /// libtorrent `peer_list::add_peer`. Returns false if the peer was rejected (full list,
    /// invalid address).
    pub fn add(
        &mut self,
        addr: SocketAddr,
        source: PeerSource,
        external_ip: ExternalIp,
        max_size: usize,
    ) -> bool {
        if let Some(p) = self.get_mut(&addr) {
            p.source |= source;
            // libtorrent `update_peer`: a tracker listing a peer that failed us gives it another
            // try, since somebody apparently reaches it (only trackers are trusted with that)
            if source.contains(PeerSource::TRACKER) {
                p.failcount = p.failcount.saturating_sub(1);
            }
            return true;
        }
        if self.peers.len() >= max_size && !self.evict_one() {
            return false;
        }
        let rank = rank(&external_ip, addr);
        self.index.insert(addr, self.peers.len());
        self.peers.push(KnownPeer::new(addr, source, rank));
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

    /// Forgets every peer `f` picks (libtorrent `apply_ip_filter` / `apply_port_filter`).
    /// Returns how many went. A connected peer's connection is the caller's to close.
    pub fn remove_if(&mut self, f: impl Fn(&KnownPeer) -> bool) -> usize {
        let before = self.peers.len();
        self.peers.retain(|p| !f(p));
        if self.peers.len() != before {
            self.index = self.peers.iter().enumerate().map(|(i, p)| (p.addr, i)).collect();
        }
        before - self.peers.len()
    }

    /// libtorrent `peer_list::num_connect_candidates` (`is_connect_candidate`): peers that could
    /// be connected to, now or once their reconnect delay is over. A finished torrent does not
    /// connect to seeds.
    pub fn num_connect_candidates(&self, finished: bool, policy: Reconnect) -> usize {
        self.peers
            .iter()
            .filter(|p| {
                p.connection.is_none()
                    && !p.banned
                    && p.failcount < policy.max_failcount
                    && !(p.seed && finished)
            })
            .count()
    }

    /// libtorrent `find_connect_candidates`: up to `n` peers to connect to now. A peer waits
    /// `min_reconnect_time` times its fail count plus one after its last connection.
    pub fn connect_candidates(&self, n: usize, now: Instant, policy: Reconnect) -> Vec<SocketAddr> {
        let mut c: Vec<&KnownPeer> = self
            .peers
            .iter()
            .filter(|p| p.connection.is_none() && !p.banned && p.failcount < policy.max_failcount)
            .filter(|p| {
                p.last_connected.is_none_or(|t| {
                    now.duration_since(t) >= policy.min_reconnect_time * (u32::from(p.failcount) + 1)
                })
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
            self.peers.push(KnownPeer::new(addr, source, 0));
        }
        let p = self.get_mut(&addr).expect("just added");
        p.connection = Some(conn);
        p.last_connected = Some(now);
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

    /// Our external address changed: rank every peer again (libtorrent `clear_peer_prio`,
    /// which makes `torrent_peer::rank` recompute on next use).
    pub fn clear_peer_prio(&mut self, external_ip: &ExternalIp) {
        for p in &mut self.peers {
            p.rank = rank(external_ip, p.addr);
        }
    }
}

/// libtorrent `torrent_peer::rank`: BEP 40 priority against the address the peer sees us at
/// (0 while we don't know it).
fn rank(external_ip: &ExternalIp, addr: SocketAddr) -> u32 {
    external_ip.external_address(addr.ip()).map_or(0, |ip| peer_priority(SocketAddr::new(ip, 0), addr))
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
        assert!(pl.add(a, PeerSource::TRACKER, ExternalIp::default(), 2));
        assert!(pl.add(b, PeerSource::TRACKER, ExternalIp::default(), 2));
        assert!(pl.add(a, PeerSource::PEX, ExternalIp::default(), 2), "duplicates merge sources");
        assert!(pl.get(&a).unwrap().source.contains(PeerSource::PEX));
        assert!(!pl.add("0.0.0.0:5".parse().unwrap(), PeerSource::TRACKER, ExternalIp::default(), 9));
        let r = Reconnect::default();
        assert_eq!(pl.connect_candidates(5, now, r).len(), 2);

        pl.connecting(a, 1, PeerSource::TRACKER, now);
        assert_eq!(pl.connect_candidates(5, now, r), [b]);
        assert_eq!(pl.num_connect_candidates(false, r), 1, "a connected peer is no candidate");
        pl.set_seed(&b, true);
        assert_eq!(pl.num_connect_candidates(true, r), 0, "a finished torrent skips seeds");
        pl.set_seed(&b, false);
        pl.disconnected(&a, true, false, false, now);
        assert_eq!(pl.connect_candidates(5, now, r), [b], "reconnects wait");
        assert_eq!(pl.num_connect_candidates(false, r), 2, "but they count");
        assert_eq!(pl.connect_candidates(5, now + Duration::from_secs(121), r), [b, a], "failed peers last");

        // Full list: the failed peer is evicted for a new one.
        let c: SocketAddr = "10.0.0.3:1".parse().unwrap();
        assert!(pl.add(c, PeerSource::TRACKER, ExternalIp::default(), 2));
        assert!(pl.get(&a).is_none() && pl.get(&b).is_some() && pl.get(&c).is_some());
        pl.ban(&b);
        assert_eq!(pl.connect_candidates(5, now, r), [c]);
        pl.trim(1);
        assert_eq!(pl.len(), 1);
    }

    /// Peers restored from resume data stay compact until the torrent starts, and read as the
    /// list they stand for: the same peers added one by one.
    #[test]
    fn saved_peers_read_as_the_list_they_stand_for() {
        assert_eq!(std::mem::size_of::<PeerList>(), 16);
        let addrs: Vec<SocketAddr> =
            ["10.0.0.1:1", "[2001:db8::1]:2", "10.0.0.2:3", "10.0.0.3:4", "[2001:db8::2]:5"]
                .iter()
                .map(|a| a.parse().unwrap())
                .collect();
        let live = || {
            // resume data lists the IPv4 peers (`peers`), then the IPv6 ones (`peers6`)
            let mut l = PeerList::default();
            for &a in addrs.iter().filter(|a| a.is_ipv4()).chain(addrs.iter().filter(|a| a.is_ipv6())) {
                l.add(a, PeerSource::RESUME_DATA, ExternalIp::default(), usize::MAX);
            }
            l
        };
        let saved = || {
            let mut l = PeerList::default();
            l.restore(SavedPeers::from_addrs(addrs.iter().copied()));
            assert!(matches!(l, PeerList::Saved(_)));
            l
        };
        let now = Instant::now();
        let (a, b) = (live(), saved());
        let r = Reconnect::default();
        let none = Reconnect { max_failcount: 0, ..r };
        assert_eq!((a.len(), a.num_seeds()), (b.len(), b.num_seeds()));
        for policy in [r, none] {
            for finished in [false, true] {
                assert_eq!(
                    a.num_connect_candidates(finished, policy),
                    b.num_connect_candidates(finished, policy)
                );
            }
            for n in [0, 3, 9] {
                assert_eq!(a.connect_candidates(n, now, policy), b.connect_candidates(n, now, policy));
            }
        }
        for n in [0, 2, 3, 100] {
            assert_eq!(a.resume_peers(n), b.resume_peers(n));
        }
        for &addr in &addrs {
            assert_eq!(a.pe_support(&addr), b.pe_support(&addr));
            assert_eq!(a.wants_utp(&addr), b.wants_utp(&addr));
        }
        for max in [0, 1, 4, 9] {
            let (mut a, mut b) = (live(), saved());
            a.trim(max);
            b.trim(max);
            assert_eq!(a.resume_peers(100), b.resume_peers(100), "trim({max})");
            assert!(max > 0 || matches!(&b, PeerList::Saved(s) if s.is_empty()));
        }
        let (mut a, mut b) = (live(), saved());
        let v6 = |k: &KnownPeer| k.addr.is_ipv6();
        assert_eq!(a.remove_if(v6), b.remove_if(v6));
        assert!(matches!(b, PeerList::Saved(_)), "filtering keeps them saved");
        assert_eq!(a.resume_peers(100), b.resume_peers(100));

        // starting expands them, ranked; changing one expands them too
        let (a, mut b) = (live(), saved());
        b.expand(&ExternalIp::default());
        assert!(matches!(b, PeerList::Live(_)));
        for &addr in &addrs {
            assert_eq!(format!("{:?}", a.get(&addr)), format!("{:?}", b.get(&addr)));
        }
        let mut b = saved();
        b.ban(&"10.9.9.9:9".parse().unwrap());
        assert!(matches!(b, PeerList::Saved(_)), "an unknown peer expands nothing");
        b.ban(&addrs[0]);
        assert!(b.get(&addrs[0]).unwrap().banned);
        let mut b = saved();
        assert!(b.add("10.9.9.9:9".parse().unwrap(), PeerSource::TRACKER, ExternalIp::default(), 99));
        assert_eq!(b.len(), addrs.len() + 1);
        // an empty list stays unallocated when the torrent starts
        let mut e = PeerList::default();
        e.expand(&ExternalIp::default());
        e.restore(SavedPeers::default());
        assert!(matches!(&e, PeerList::Saved(s) if s.is_empty()));
    }

    #[test]
    fn the_reconnect_delay_runs_from_the_close() {
        // libtorrent `connection_closed` stamps `last_connected`: a peer connected for an hour
        // waits `min_reconnect_time` after it leaves, not after it came
        let mut pl = PeerList::default();
        let r = Reconnect::default();
        let t0 = Instant::now();
        let a: SocketAddr = "10.0.0.1:1".parse().unwrap();
        pl.connecting(a, 1, PeerSource::TRACKER, t0);
        let closed = t0 + Duration::from_secs(3600);
        pl.disconnected(&a, false, false, false, closed);
        assert!(pl.connect_candidates(5, closed + Duration::from_secs(59), r).is_empty());
        assert_eq!(pl.connect_candidates(5, closed + Duration::from_secs(60), r), [a]);
        // the plaintext retry after an encrypted attempt goes at once
        pl.connecting(a, 2, PeerSource::TRACKER, closed);
        pl.disconnected(&a, false, false, true, closed);
        assert_eq!(pl.connect_candidates(5, closed, r), [a]);
    }

    #[test]
    fn a_tracker_gives_a_failed_peer_another_try() {
        // libtorrent `update_peer`: only a tracker's word takes a failure off
        let mut pl = PeerList::default();
        let r = Reconnect { max_failcount: 2, min_reconnect_time: Duration::from_secs(1) };
        let now = Instant::now();
        let a: SocketAddr = "10.0.0.1:1".parse().unwrap();
        assert!(pl.add(a, PeerSource::DHT, ExternalIp::default(), 9));
        for _ in 0..2 {
            pl.connecting(a, 1, PeerSource::DHT, now);
            pl.disconnected(&a, true, false, false, now);
        }
        let later = now + Duration::from_secs(10);
        assert_eq!(pl.get(&a).unwrap().failcount, 2);
        assert!(pl.connect_candidates(5, later, r).is_empty(), "max_failcount reached");
        assert_eq!(pl.num_connect_candidates(false, r), 0);
        pl.add(a, PeerSource::PEX, ExternalIp::default(), 9);
        assert_eq!(pl.get(&a).unwrap().failcount, 2, "a peer's word is not enough");
        pl.add(a, PeerSource::TRACKER, ExternalIp::default(), 9);
        assert_eq!(pl.get(&a).unwrap().failcount, 1);
        assert_eq!(pl.connect_candidates(5, later, r), [a]);
        // the settings' names and defaults are libtorrent's
        let d = Reconnect::from_settings(&crate::Settings::default());
        assert_eq!((d.max_failcount, d.min_reconnect_time), (3, Duration::from_secs(60)));
    }
}
