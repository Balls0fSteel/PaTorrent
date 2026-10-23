// Ported from libtorrent-rasterbar 2.0.11: include/libtorrent/peer_class.hpp, src/peer_class.cpp,
// include/libtorrent/peer_class_set.hpp, src/peer_class_set.cpp,
// include/libtorrent/peer_class_type_filter.hpp, the peer class parts of src/session_impl.cpp
// (the default classes, init_peer_class_filter, set_peer_classes, ignore_unchoke_slots_set,
// copy_pertinent_channels, create/delete/get/set_peer_class, set_rate_limit, the mixed mode block
// of on_tick, the connection_limit_factor of incoming_connection) and src/peer_connection.cpp
// (get_priority, ignore_unchoke_slots, the channels of request_bandwidth)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a class's bandwidth channels are the engine's token buckets (`net::Bucket`); the
// global class's two channels are the engine's session buckets and a torrent's own class is its
// pair of buckets in `Torrent` (no pool entry), so the torrent code reads them where it always
// did. Bandwidth is not queued per request: a peer or web seed connection (`Requester`) reads or
// writes what the smallest of its channels allows, and the bandwidth tick splits each channel
// over the connections waiting for it by priority (`Engine::quota_shares`). The peer class filter is a plain `IpFilter` whose
// flags are class masks, as in libtorrent.

//! Peer classes: groups of peers sharing rate limits, bandwidth priorities and choking and
//! connection-limit exemptions (libtorrent `peer_class`, `peer_class_set`,
//! `peer_class_type_filter`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::Engine;
use super::peer::PeerConn;
use super::torrent::Torrent;
use crate::net::Bucket;
use crate::net::ip_filter::IpFilter;
use crate::session::PeerClassCommand;
use crate::settings::MixedModeAlgorithm;

/// libtorrent `peer_connection::upload_channel`.
pub(crate) const UPLOAD: usize = 0;
/// libtorrent `peer_connection::download_channel`.
pub(crate) const DOWNLOAD: usize = 1;

/// A peer class (libtorrent `peer_class_t`). Classes 0 to 31 can be named by the peer class
/// filter and the type filter, which hold class masks of 32 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerClassId(pub u32);

impl PeerClassId {
    /// Every peer is in it by default; its limits are the session's `upload_rate_limit` and
    /// `download_rate_limit` (libtorrent `session::global_peer_class_id`).
    pub const GLOBAL: PeerClassId = PeerClassId(0);
    /// TCP peers are in it by default; `mixed_mode_algorithm` sets its limits
    /// (libtorrent `session::tcp_peer_class_id`).
    pub const TCP: PeerClassId = PeerClassId(1);
    /// Labelled "local": it ignores the unchoke slots and lets its peers exceed the connection
    /// limits by half (libtorrent `session::local_peer_class_id`). No address maps to it by
    /// default here (see [`SessionHandle::set_peer_class_filter`](crate::SessionHandle::set_peer_class_filter)).
    pub const LOCAL: PeerClassId = PeerClassId(2);

    /// The bit of this class in a peer class filter or type filter mask (0 for ids past 31).
    pub fn mask(self) -> u32 {
        if self.0 < 32 { 1 << self.0 } else { 0 }
    }
}

/// The settings of a peer class (libtorrent `peer_class_info`), read with
/// [`SessionHandle::get_peer_class`](crate::SessionHandle::get_peer_class) and written with
/// [`SessionHandle::set_peer_class`](crate::SessionHandle::set_peer_class).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerClassInfo {
    /// Peers in a class with this set are always unchoked, outside the unchoke slots.
    pub ignore_unchoke_slots: bool,
    /// Percent of the connection limits (the session's and each torrent's) that applies to
    /// the class's peers: 200 accepts them while there are fewer than twice the limit.
    pub connection_limit_factor: u32,
    /// Not used by the engine: a name for the user.
    pub label: String,
    /// Bytes per second for all the class's peers together; 0 = unlimited (1 to 9 become 10).
    pub upload_limit: u32,
    pub download_limit: u32,
    /// Bandwidth priority of the class's peers when a limit is shared, 1 to 255.
    pub upload_priority: u32,
    pub download_priority: u32,
}

/// One class (libtorrent `peer_class`).
pub(crate) struct PeerClass {
    /// The bandwidth channels, upload and download (unused for the global class, whose
    /// channels are the engine's session buckets).
    pub channel: [Bucket; 2],
    pub ignore_unchoke_slots: bool,
    pub connection_limit_factor: u32,
    /// Priority in the rate limiter, per channel.
    pub priority: [u32; 2],
    pub label: String,
    in_use: bool,
    references: u32,
}

impl PeerClass {
    fn new(label: String) -> PeerClass {
        PeerClass {
            channel: [Bucket::new(0), Bucket::new(0)],
            ignore_unchoke_slots: false,
            connection_limit_factor: 100,
            priority: [1, 1],
            label,
            in_use: true,
            references: 1,
        }
    }

    fn clear(&mut self) {
        self.in_use = false;
        self.label.clear();
    }

    /// libtorrent `peer_class::get_info`; `limits` stands in for the channels of the global
    /// class.
    pub fn get_info(&self, limits: [u32; 2]) -> PeerClassInfo {
        PeerClassInfo {
            ignore_unchoke_slots: self.ignore_unchoke_slots,
            connection_limit_factor: self.connection_limit_factor,
            label: self.label.clone(),
            upload_limit: limits[UPLOAD],
            download_limit: limits[DOWNLOAD],
            upload_priority: self.priority[UPLOAD],
            download_priority: self.priority[DOWNLOAD],
        }
    }

    /// libtorrent `peer_class::set_info`, without the limits (the caller sets them, see
    /// `Engine::set_peer_class`).
    fn set_info(&mut self, pci: &PeerClassInfo) {
        self.ignore_unchoke_slots = pci.ignore_unchoke_slots;
        self.connection_limit_factor = pci.connection_limit_factor;
        self.label = pci.label.clone();
        self.priority[UPLOAD] = pci.upload_priority.clamp(1, 255);
        self.priority[DOWNLOAD] = pci.download_priority.clamp(1, 255);
    }
}

/// libtorrent `peer_class::set_upload_limit`: 1 to 9 bytes per second are 10.
fn clamp_limit(limit: u32) -> u32 {
    if limit > 0 && limit < 10 { 10 } else { limit }
}

/// The session's classes (libtorrent `peer_class_pool`): ids are reused once a class has no
/// references left.
#[derive(Default)]
pub(crate) struct PeerClassPool {
    classes: Vec<PeerClass>,
    free: Vec<PeerClassId>,
}

impl PeerClassPool {
    /// The pool with libtorrent's default classes: global, tcp and local (`session_impl`'s
    /// constructor).
    pub fn with_defaults() -> PeerClassPool {
        let mut pool = PeerClassPool::default();
        let global = pool.new_peer_class("global".into());
        let tcp = pool.new_peer_class("tcp".into());
        let local = pool.new_peer_class("local".into());
        debug_assert_eq!((global, tcp, local), (PeerClassId::GLOBAL, PeerClassId::TCP, PeerClassId::LOCAL));
        let l = pool.at_mut(local).expect("just created");
        // local peers are always unchoked
        l.ignore_unchoke_slots = true;
        // local peers are allowed to exceed the normal connection limit by 50%
        l.connection_limit_factor = 150;
        pool
    }

    /// A new class holding one reference (libtorrent `new_peer_class`).
    pub fn new_peer_class(&mut self, label: String) -> PeerClassId {
        if let Some(id) = self.free.pop() {
            self.classes[id.0 as usize] = PeerClass::new(label);
            id
        } else {
            self.classes.push(PeerClass::new(label));
            PeerClassId(self.classes.len() as u32 - 1)
        }
    }

    pub fn decref(&mut self, c: PeerClassId) {
        let Some(pc) = self.at_mut(c) else {
            debug_assert!(false, "decref of a peer class not in use");
            return;
        };
        debug_assert!(pc.references > 0);
        pc.references -= 1;
        if pc.references > 0 {
            return;
        }
        pc.clear();
        self.free.push(c);
    }

    pub fn incref(&mut self, c: PeerClassId) {
        let Some(pc) = self.at_mut(c) else {
            debug_assert!(false, "incref of a peer class not in use");
            return;
        };
        pc.references += 1;
    }

    pub fn at(&self, c: PeerClassId) -> Option<&PeerClass> {
        self.classes.get(c.0 as usize).filter(|pc| pc.in_use)
    }

    pub fn at_mut(&mut self, c: PeerClassId) -> Option<&mut PeerClass> {
        self.classes.get_mut(c.0 as usize).filter(|pc| pc.in_use)
    }

    /// Refills the channels of every class with a limit (the bandwidth tick).
    pub fn refill(&mut self, dt: f64) {
        for pc in self.classes.iter_mut().filter(|pc| pc.in_use) {
            pc.channel[UPLOAD].refill(dt);
            pc.channel[DOWNLOAD].refill(dt);
        }
    }

    pub fn any_limited(&self) -> bool {
        self.classes
            .iter()
            .any(|pc| pc.in_use && (pc.channel[UPLOAD].limit > 0 || pc.channel[DOWNLOAD].limit > 0))
    }
}

/// The classes a peer (or torrent) belongs to (libtorrent `peer_class_set`): at most 14, each
/// holding a reference in the pool.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PeerClassSet {
    size: u8,
    class: [PeerClassId; 15],
}

impl Default for PeerClassSet {
    fn default() -> PeerClassSet {
        PeerClassSet { size: 0, class: [PeerClassId::GLOBAL; 15] }
    }
}

impl PeerClassSet {
    pub fn add_class(&mut self, pool: &mut PeerClassPool, c: PeerClassId) {
        if self.has_class(c) {
            return;
        }
        // libtorrent keeps the last of its 15 entries unused
        if self.size as usize >= self.class.len() - 1 {
            debug_assert!(false, "too many peer classes");
            return;
        }
        self.class[self.size as usize] = c;
        pool.incref(c);
        self.size += 1;
    }

    pub fn has_class(&self, c: PeerClassId) -> bool {
        self.iter().any(|x| x == c)
    }

    // ported with libtorrent's set; no caller shrinks a set yet (a torrent's own class is its
    // pair of buckets here, not a pool entry it would drop)
    #[allow(dead_code)]
    pub fn remove_class(&mut self, pool: &mut PeerClassPool, c: PeerClassId) {
        let Some(idx) = self.iter().position(|x| x == c) else { return };
        let last = self.size as usize - 1;
        // place the last element in the slot of the erased one
        self.class[idx] = self.class[last];
        self.size -= 1;
        pool.decref(c);
    }

    pub fn num_classes(&self) -> usize {
        self.size as usize
    }

    pub fn iter(&self) -> impl Iterator<Item = PeerClassId> + '_ {
        self.class[..self.size as usize].iter().copied()
    }

    /// Drops every class's reference.
    pub fn clear(&mut self, pool: &mut PeerClassPool) {
        for c in self.iter() {
            pool.decref(c);
        }
        self.size = 0;
    }
}

/// The kinds of socket a [`PeerClassTypeFilter`] tells apart (libtorrent
/// `peer_class_type_filter::socket_type_t`). PaTorrent has no SSL torrents or I2P, so only
/// `Tcp` and `Utp` sockets occur; the others are kept for libtorrent's numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketType {
    Tcp = 0,
    Utp = 1,
    SslTcp = 2,
    SslUtp = 3,
    I2p = 4,
}

const NUM_SOCKET_TYPES: usize = 5;

/// Rules adding and removing peer classes by socket type, applied after the peer class filter
/// (libtorrent `peer_class_type_filter`). A new filter changes nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerClassTypeFilter {
    /// Per socket type, the classes kept from the address's mask.
    mask: [u32; NUM_SOCKET_TYPES],
    /// Per socket type, the classes added.
    add: [u32; NUM_SOCKET_TYPES],
}

impl Default for PeerClassTypeFilter {
    fn default() -> PeerClassTypeFilter {
        PeerClassTypeFilter { mask: [u32::MAX; NUM_SOCKET_TYPES], add: [0; NUM_SOCKET_TYPES] }
    }
}

impl PeerClassTypeFilter {
    pub fn new() -> PeerClassTypeFilter {
        PeerClassTypeFilter::default()
    }

    /// New peers on `st` sockets join `class` (ids past 31 are ignored).
    pub fn add(&mut self, st: SocketType, class: PeerClassId) {
        self.add[st as usize] |= class.mask();
    }

    /// Undoes [`PeerClassTypeFilter::add`].
    pub fn remove(&mut self, st: SocketType, class: PeerClassId) {
        self.add[st as usize] &= !class.mask();
    }

    /// New peers on `st` sockets never join `class`, whatever their address maps to.
    pub fn disallow(&mut self, st: SocketType, class: PeerClassId) {
        self.mask[st as usize] &= !class.mask();
    }

    /// Undoes [`PeerClassTypeFilter::disallow`].
    pub fn allow(&mut self, st: SocketType, class: PeerClassId) {
        self.mask[st as usize] |= class.mask();
    }

    /// The class mask of a peer on an `st` socket whose address maps to `peer_class_mask`.
    pub fn apply(&self, st: SocketType, peer_class_mask: u32) -> u32 {
        (peer_class_mask & self.mask[st as usize]) | self.add[st as usize]
    }
}

/// The default peer class filter: every address in the global class. This is libtorrent's
/// `init_peer_class_filter(false)`; libtorrent's default (`true`) puts the local networks and
/// loopback in the local class instead, which exempts them from the global rate limits and the
/// unchoke slots. We keep the limits on LAN peers, as µTorrent does (docs/PORTING.md).
pub(crate) fn default_peer_class_filter() -> IpFilter {
    let g = PeerClassId::GLOBAL.mask();
    let mut f = IpFilter::new();
    f.add_rule(IpAddr::V4(Ipv4Addr::UNSPECIFIED), IpAddr::V4(Ipv4Addr::BROADCAST), g);
    f.add_rule(IpAddr::V6(Ipv6Addr::UNSPECIFIED), IpAddr::V6(Ipv6Addr::from_bits(u128::MAX)), g);
    f
}

/// libtorrent's default type filter: TCP (and SSL/TCP and I2P) sockets join the tcp class.
pub(crate) fn default_peer_class_type_filter() -> PeerClassTypeFilter {
    let mut f = PeerClassTypeFilter::new();
    f.add(SocketType::Tcp, PeerClassId::TCP);
    f.add(SocketType::SslTcp, PeerClassId::TCP);
    f.add(SocketType::I2p, PeerClassId::TCP);
    f
}

/// The session's peer classes and the filters that assign them.
pub(super) struct PeerClasses {
    pub pool: PeerClassPool,
    pub filter: IpFilter,
    pub type_filter: PeerClassTypeFilter,
}

impl Default for PeerClasses {
    fn default() -> PeerClasses {
        PeerClasses {
            pool: PeerClassPool::with_defaults(),
            filter: default_peer_class_filter(),
            type_filter: default_peer_class_type_filter(),
        }
    }
}

/// A torrent's own class: its pair of buckets (libtorrent `torrent::m_peer_class`, created on
/// the first limit).
fn torrent_channel(t: &Torrent, channel: usize) -> &Bucket {
    if channel == DOWNLOAD { &t.download_limit } else { &t.upload_limit }
}

fn torrent_channel_mut(t: &mut Torrent, channel: usize) -> &mut Bucket {
    if channel == DOWNLOAD { &mut t.download_limit } else { &mut t.upload_limit }
}

/// A rate-limited bandwidth channel a peer's transfers draw from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Channel {
    Class(PeerClassId),
    Torrent(crate::types::TorrentId),
}

/// A connection waiting for bandwidth (libtorrent's `bw_request` peer): a BitTorrent peer or a
/// web seed connection, by slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Requester {
    Peer(usize),
    Web(usize),
}

impl Engine {
    /// libtorrent `session_impl::set_peer_classes`: the classes of a new connection to `addr`
    /// over TCP or uTP, each holding a reference.
    pub(super) fn peer_classes_for(&mut self, addr: IpAddr, utp: bool) -> PeerClassSet {
        self.peer_classes_for_type(addr, if utp { SocketType::Utp } else { SocketType::Tcp })
    }

    /// [`Engine::peer_classes_for`] by socket type. Web seed connections are `peer_connection`s
    /// in libtorrent and get their classes the same way: `Tcp` for plain HTTP (its socket is a
    /// TCP, SOCKS5 or HTTP proxy stream, all `tcp_socket` to the type filter) and `SslTcp` for
    /// HTTPS.
    pub(super) fn peer_classes_for_type(&mut self, addr: IpAddr, st: SocketType) -> PeerClassSet {
        let pc = &mut self.peer_classes;
        let mut mask = pc.type_filter.apply(st, pc.filter.access(addr));
        let mut set = PeerClassSet::default();
        let mut i = 0;
        while mask != 0 {
            if mask & 1 != 0 && pc.pool.at(PeerClassId(i)).is_some() {
                set.add_class(&mut pc.pool, PeerClassId(i));
            }
            mask >>= 1;
            i += 1;
        }
        set
    }

    /// The largest `connection_limit_factor` of the classes, 100 if none has one (libtorrent
    /// `incoming_connection`, `torrent::attach_peer`).
    fn connection_limit_factor(&self, classes: impl Iterator<Item = PeerClassId>) -> u64 {
        let f = classes.filter_map(|c| self.peer_classes.pool.at(c)).map(|c| c.connection_limit_factor).max();
        match f {
            Some(f) if f > 0 => f as u64,
            _ => 100,
        }
    }

    /// `limit` weighed by the peer's classes (libtorrent: `limit * 100 / connection_limit_factor`).
    pub(super) fn weighed_limit(&self, set: &PeerClassSet, limit: usize) -> usize {
        (limit as u64 * 100 / self.connection_limit_factor(set.iter())) as usize
    }

    /// libtorrent `session_impl::incoming_connection`: the session's `connections_limit`
    /// weighed by the classes an incoming connection from `addr` would get is reached.
    pub(super) fn incoming_over_limit(&self, addr: SocketAddr, utp: bool) -> bool {
        self.incoming_over_limit_with(addr, utp, 0)
    }

    /// [`Engine::incoming_over_limit`] counting `pending` connections accepted but not yet
    /// peers (uTP connections accepted earlier in the same wakeup).
    pub(super) fn incoming_over_limit_with(&self, addr: SocketAddr, utp: bool, pending: usize) -> bool {
        let limit = self.settings.connections_limit;
        if limit < 0 {
            return false;
        }
        let pc = &self.peer_classes;
        let st = if utp { SocketType::Utp } else { SocketType::Tcp };
        let mask = pc.type_filter.apply(st, pc.filter.access(addr.ip()));
        let classes = (0..32).filter(|i| mask & (1 << i) != 0).map(PeerClassId);
        let limit = limit as u64 * 100 / self.connection_limit_factor(classes);
        (self.peers.len() + pending) as u64 >= limit
    }

    /// libtorrent `peer_connection::ignore_unchoke_slots`: a peer in no class, or in a class
    /// (its own or its torrent's) that ignores the unchoke slots, is never choked by the
    /// choker.
    pub(super) fn ignore_unchoke_slots(&self, p: &PeerConn) -> bool {
        p.classes.num_classes() == 0
            || p.classes.iter().filter_map(|c| self.peer_classes.pool.at(c)).any(|c| c.ignore_unchoke_slots)
    }

    /// The bucket of class `c`'s `channel`: the session's buckets for the global class.
    pub(super) fn class_channel(&self, c: PeerClassId, channel: usize) -> Option<&Bucket> {
        let pc = self.peer_classes.pool.at(c)?;
        Some(match (c, channel) {
            (PeerClassId::GLOBAL, UPLOAD) => &self.upload_bucket,
            (PeerClassId::GLOBAL, _) => &self.download_bucket,
            _ => &pc.channel[channel],
        })
    }

    fn channel_bucket(&self, ch: Channel, channel: usize) -> Option<&Bucket> {
        match ch {
            Channel::Class(c) => self.class_channel(c, channel),
            Channel::Torrent(id) => self.torrent_ref(id).map(|t| torrent_channel(t, channel)),
        }
    }

    /// The classes and the torrent whose channels `r`'s transfers draw from.
    fn quota_owner(&self, r: Requester) -> Option<(PeerClassSet, Option<crate::types::TorrentId>)> {
        match r {
            Requester::Peer(slot) => self.peers.get(slot).map(|p| (p.classes, p.torrent)),
            Requester::Web(slot) => self.web.quota_owner(slot).map(|(c, t)| (c, Some(t))),
        }
    }

    /// libtorrent `copy_pertinent_channels` over the connection's classes and its torrent's:
    /// the channels with a limit that `r`'s transfers in `channel` draw from.
    pub(super) fn pertinent_channels(&self, r: Requester, channel: usize, out: &mut Vec<Channel>) {
        out.clear();
        let Some((classes, torrent)) = self.quota_owner(r) else { return };
        for c in classes.iter() {
            if self.class_channel(c, channel).is_some_and(|b| b.limit > 0) {
                out.push(Channel::Class(c));
            }
        }
        if let Some(id) = torrent {
            if self.torrent_ref(id).is_some_and(|t| torrent_channel(t, channel).limit > 0) {
                out.push(Channel::Torrent(id));
            }
        }
    }

    /// libtorrent `peer_connection::get_priority`: the highest priority of the connection's
    /// classes (a torrent's own class has priority 1).
    pub(super) fn peer_priority(&self, r: Requester, channel: usize) -> u32 {
        let Some((classes, _)) = self.quota_owner(r) else { return 1 };
        classes
            .iter()
            .filter_map(|c| self.peer_classes.pool.at(c))
            .map(|c| c.priority[channel])
            .fold(1, u32::max)
    }

    /// How many bytes peer `slot` may move in `channel` now: the least any of its channels has
    /// left (`usize::MAX` when none is limited).
    pub(super) fn peer_quota(&self, slot: usize, channel: usize) -> usize {
        self.quota(Requester::Peer(slot), channel)
    }

    /// [`Engine::peer_quota`] for any connection.
    pub(super) fn quota(&self, r: Requester, channel: usize) -> usize {
        let Some((classes, torrent)) = self.quota_owner(r) else { return 0 };
        let classes = classes.iter().filter_map(|c| self.class_channel(c, channel));
        let torrent = torrent.and_then(|id| self.torrent_ref(id)).map(|t| torrent_channel(t, channel));
        classes.chain(torrent).map(Bucket::available).min().unwrap_or(usize::MAX)
    }

    /// libtorrent `bandwidth_channel::use_quota` on every channel of peer `slot`.
    pub(super) fn use_quota(&mut self, slot: usize, channel: usize, n: usize) {
        self.use_quota_of(Requester::Peer(slot), channel, n);
    }

    /// [`Engine::use_quota`] for any connection.
    pub(super) fn use_quota_of(&mut self, r: Requester, channel: usize, n: usize) {
        let Some((classes, torrent)) = self.quota_owner(r) else { return };
        for c in classes.iter() {
            if self.peer_classes.pool.at(c).is_none() {
                continue;
            }
            match (c, channel) {
                (PeerClassId::GLOBAL, UPLOAD) => self.upload_bucket.consume(n),
                (PeerClassId::GLOBAL, _) => self.download_bucket.consume(n),
                _ => {
                    if let Some(pc) = self.peer_classes.pool.at_mut(c) {
                        pc.channel[channel].consume(n);
                    }
                }
            }
        }
        if let Some(t) = torrent.and_then(|id| self.torrent(id)) {
            torrent_channel_mut(t, channel).consume(n);
        }
    }

    /// The share of `ch`'s quota a peer with `priority` gets when `waiting` is the sum of the
    /// priorities of the peers waiting on it (libtorrent `bw_request::assign_bandwidth`:
    /// `distribute_quota * priority / tmp`).
    pub(super) fn channel_share(&self, ch: Channel, channel: usize, priority: u32, waiting: u64) -> usize {
        match self.channel_bucket(ch, channel).map(Bucket::available) {
            None | Some(usize::MAX) => usize::MAX,
            Some(a) => (a as u64 * priority as u64 / waiting.max(1)) as usize,
        }
    }

    /// libtorrent `session_impl::set_rate_limit`.
    pub(super) fn set_rate_limit(&mut self, c: PeerClassId, channel: usize, limit: u32) {
        let limit = limit.min(i32::MAX as u32 - 1);
        if self.peer_classes.pool.at(c).is_none() {
            return;
        }
        match (c, channel) {
            (PeerClassId::GLOBAL, UPLOAD) => self.upload_bucket.set_limit(limit),
            (PeerClassId::GLOBAL, _) => self.download_bucket.set_limit(limit),
            _ => {
                if let Some(pc) = self.peer_classes.pool.at_mut(c) {
                    pc.channel[channel].set_limit(limit);
                }
            }
        }
    }

    pub(super) fn create_peer_class(&mut self, label: String) -> PeerClassId {
        self.peer_classes.pool.new_peer_class(label)
    }

    /// libtorrent `delete_peer_class`: drops the creator's reference; the class lives on until
    /// its last peer is gone. The three default classes are never deleted here (libtorrent
    /// lets them go, and a new class would then take over the global class's id).
    pub(super) fn delete_peer_class(&mut self, c: PeerClassId) {
        if c.0 <= PeerClassId::LOCAL.0 || self.peer_classes.pool.at(c).is_none() {
            return;
        }
        self.peer_classes.pool.decref(c);
    }

    pub(super) fn get_peer_class(&self, c: PeerClassId) -> Option<PeerClassInfo> {
        let pc = self.peer_classes.pool.at(c)?;
        let limit = |ch| self.class_channel(c, ch).map_or(0, |b| b.limit);
        Some(pc.get_info([limit(UPLOAD), limit(DOWNLOAD)]))
    }

    /// libtorrent `set_peer_class` (`peer_class::set_info`).
    pub(super) fn set_peer_class(&mut self, c: PeerClassId, info: PeerClassInfo) {
        let Some(pc) = self.peer_classes.pool.at_mut(c) else { return };
        pc.set_info(&info);
        self.set_rate_limit(c, UPLOAD, clamp_limit(info.upload_limit));
        self.set_rate_limit(c, DOWNLOAD, clamp_limit(info.download_limit));
    }

    /// The peer class calls of the session handle (libtorrent `session_handle`'s
    /// `create_peer_class` and the rest, which run on the network thread).
    pub(super) fn peer_class_command(&mut self, c: PeerClassCommand) {
        match c {
            PeerClassCommand::Create(label, reply) => {
                let id = self.create_peer_class(label);
                let _ = reply.send(id);
            }
            PeerClassCommand::Delete(id) => self.delete_peer_class(id),
            PeerClassCommand::Get(id, reply) => {
                let _ = reply.send(self.get_peer_class(id));
            }
            PeerClassCommand::Set(id, info) => self.set_peer_class(id, info),
            // libtorrent: the filters apply to connections made from now on
            PeerClassCommand::SetFilter(f) => self.peer_classes.filter = f,
            PeerClassCommand::GetFilter(reply) => {
                let _ = reply.send(self.peer_classes.filter.clone());
            }
            PeerClassCommand::SetTypeFilter(f) => self.peer_classes.type_filter = f,
            PeerClassCommand::GetTypeFilter(reply) => {
                let _ = reply.send(self.peer_classes.type_filter.clone());
            }
        }
    }

    /// The mixed mode block of libtorrent `session_impl::on_tick`: with `peer_proportional`,
    /// while uTP peers move data, TCP peers together may move at most four times their share
    /// (by peer count) of the session's current rate, never less than 5 kB/s up and 30 kB/s
    /// down; with fewer than five busy peers or no busy uTP peer the tcp class is unlimited.
    /// `prefer_tcp` leaves it unlimited.
    pub(super) fn update_mixed_mode(&mut self) {
        match self.settings.mixed_mode_algorithm {
            MixedModeAlgorithm::PreferTcp => {
                self.set_rate_limit(PeerClassId::TCP, UPLOAD, 0);
                self.set_rate_limit(PeerClassId::TCP, DOWNLOAD, 0);
            }
            MixedModeAlgorithm::PeerProportional => {
                // [protocol: tcp, utp][channel]
                let mut num_peers = [[0u64; 2]; 2];
                for (_, p) in self.peers.iter() {
                    if p.state != super::peer::PeerState::Active {
                        continue;
                    }
                    let protocol = p.sock.is_utp() as usize;
                    // libtorrent: more than one block requested and a read on the socket
                    // (`bw_network`), not waiting for quota
                    if p.requests.len() > 1 && !p.read_blocked {
                        num_peers[protocol][DOWNLOAD] += 1;
                    }
                    // requests to serve and bytes on their way out, not waiting for quota
                    if (!p.upload_queue.is_empty() || !p.reads_in_flight.is_empty())
                        && p.send_buffer_len() > 0
                        && !p.upload_quota_blocked
                    {
                        num_peers[protocol][UPLOAD] += 1;
                    }
                }
                let stat_rate = [self.stat.upload_rate() as u64, self.stat.download_rate() as u64];
                for (i, limit) in peer_proportional_limits(num_peers, stat_rate).into_iter().enumerate() {
                    self.set_rate_limit(PeerClassId::TCP, i, limit);
                }
            }
        }
    }
}

/// The tcp class's limits per channel under `peer_proportional` (libtorrent `on_tick`), from
/// the busy peers per protocol and channel (`[tcp, utp][channel]`) and the session's rates per
/// channel; 0 is unlimited.
fn peer_proportional_limits(mut num_peers: [[u64; 2]; 2], stat_rate: [u64; 2]) -> [u32; 2] {
    // never throttle below this
    let lower_limit = [5000u64, 30000];
    let mut out = [0; 2];
    for i in 0..2 {
        // if there are no uTP peers, don't throttle TCP
        let total_peers = num_peers[0][i] + num_peers[1][i];
        if num_peers[1][i] == 0 || total_peers < 5 {
            continue;
        }
        if num_peers[0][i] == 0 {
            num_peers[0][i] = 1;
        }
        let limit = (stat_rate[i] * num_peers[0][i] * 4 / total_peers).max(lower_limit[i]);
        out[i] = limit.min(u32::MAX as u64) as u32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class_name(id: PeerClassId, p: &PeerClassPool) -> String {
        p.at(id).map(|c| c.get_info([0, 0]).label).expect("class exists")
    }

    /// libtorrent test_peer_classes.cpp `peer_class`.
    #[test]
    fn peer_class() {
        let mut pool = PeerClassPool::default();
        let id1 = pool.new_peer_class("test1".into());
        let id2 = pool.new_peer_class("test2".into());

        // make sure there's no leak
        for _ in 0..1000 {
            let tmp = pool.new_peer_class("temp".into());
            pool.decref(tmp);
        }

        let id3 = pool.new_peer_class("test3".into());
        assert_eq!(id3, PeerClassId(id2.0 + 1));

        // make sure refcounting works
        assert_eq!(class_name(id3, &pool), "test3");
        pool.incref(id3);
        assert_eq!(class_name(id3, &pool), "test3");
        pool.decref(id3);
        assert_eq!(class_name(id3, &pool), "test3");
        pool.decref(id3);
        // it should have been deleted now
        assert!(pool.at(id3).is_none());

        // test setting and retrieving upload and download rates
        let c = pool.at_mut(id2).unwrap();
        c.channel[UPLOAD].set_limit(clamp_limit(1000));
        c.channel[DOWNLOAD].set_limit(clamp_limit(2000));
        let cls = c.get_info([c.channel[UPLOAD].limit, c.channel[DOWNLOAD].limit]);
        assert_eq!(cls.upload_limit, 1000);
        assert_eq!(cls.download_limit, 2000);

        // test peer_class_type_filter
        let mut filter = PeerClassTypeFilter::new();
        let types =
            [SocketType::Tcp, SocketType::Utp, SocketType::SslTcp, SocketType::SslUtp, SocketType::I2p];
        for st in types {
            assert_eq!(filter.apply(st, 0xffff_ffff), 0xffff_ffff);
        }
        filter.disallow(SocketType::Tcp, PeerClassId(0));
        assert_eq!(filter.apply(SocketType::Tcp, 0xffff_ffff), 0xffff_fffe);
        assert_eq!(filter.apply(SocketType::Utp, 0xffff_ffff), 0xffff_ffff);
        filter.allow(SocketType::Tcp, PeerClassId(0));
        assert_eq!(filter.apply(SocketType::Tcp, 0xffff_ffff), 0xffff_ffff);

        assert_eq!(filter.apply(SocketType::Tcp, 0), 0);
        filter.add(SocketType::Tcp, PeerClassId(0));
        assert_eq!(filter.apply(SocketType::Tcp, 0), 1);
        filter.remove(SocketType::Tcp, PeerClassId(0));
        assert_eq!(filter.apply(SocketType::Tcp, 0), 0);

        pool.decref(id2);
        pool.decref(id1);
        assert!(pool.at(id2).is_none());
        assert!(pool.at(id1).is_none());
    }

    #[test]
    fn limits_below_ten_become_ten_and_priorities_are_clamped() {
        let mut pool = PeerClassPool::default();
        let id = pool.new_peer_class("x".into());
        let c = pool.at_mut(id).unwrap();
        c.channel[UPLOAD].set_limit(clamp_limit(3));
        c.channel[DOWNLOAD].set_limit(clamp_limit(0));
        assert_eq!((c.channel[UPLOAD].limit, c.channel[DOWNLOAD].limit), (10, 0));
        c.set_info(&PeerClassInfo {
            ignore_unchoke_slots: true,
            connection_limit_factor: 150,
            label: "y".into(),
            upload_limit: 0,
            download_limit: 0,
            upload_priority: 0,
            download_priority: 1000,
        });
        assert_eq!(c.priority, [1, 255]);
        assert_eq!(c.label, "y");
    }

    #[test]
    fn default_classes_and_filters() {
        let pool = PeerClassPool::with_defaults();
        let names: Vec<String> = (0..3).map(|i| class_name(PeerClassId(i), &pool)).collect();
        assert_eq!(names, ["global", "tcp", "local"]);
        let local = pool.at(PeerClassId::LOCAL).unwrap();
        assert!(local.ignore_unchoke_slots);
        assert_eq!(local.connection_limit_factor, 150);
        assert!(!pool.at(PeerClassId::GLOBAL).unwrap().ignore_unchoke_slots);

        let f = default_peer_class_filter();
        for a in ["127.0.0.1", "192.168.1.1", "8.8.8.8", "::1", "2001:db8::1", "::ffff:10.0.0.1"] {
            assert_eq!(f.access(a.parse().unwrap()), 1, "{a}");
        }
        let t = default_peer_class_type_filter();
        assert_eq!(t.apply(SocketType::Tcp, 1), 0b11);
        assert_eq!(t.apply(SocketType::Utp, 1), 0b01);
        assert_eq!(t.apply(SocketType::SslTcp, 0), 0b10);
        assert_eq!(t.apply(SocketType::I2p, 0), 0b10);
        assert_eq!(t.apply(SocketType::SslUtp, 0), 0);
    }

    #[test]
    fn peer_class_set_refcounts_and_capacity() {
        let mut pool = PeerClassPool::default();
        let ids: Vec<PeerClassId> = (0..20).map(|i| pool.new_peer_class(format!("c{i}"))).collect();
        let mut set = PeerClassSet::default();
        set.add_class(&mut pool, ids[0]);
        set.add_class(&mut pool, ids[0]);
        assert_eq!(set.num_classes(), 1);
        // the creator's reference and the set's
        pool.decref(ids[0]);
        assert!(pool.at(ids[0]).is_some());
        set.remove_class(&mut pool, ids[0]);
        assert!(pool.at(ids[0]).is_none());
        assert_eq!(set.num_classes(), 0);
        // remove of a class not in the set is a no-op
        set.remove_class(&mut pool, ids[1]);
        assert!(pool.at(ids[1]).is_some());

        let mut set = PeerClassSet::default();
        for &id in &ids[1..15] {
            set.add_class(&mut pool, id);
        }
        assert_eq!(set.num_classes(), 14);
        set.remove_class(&mut pool, ids[3]);
        assert!(!set.has_class(ids[3]));
        assert!(set.has_class(ids[14]));
        assert_eq!(set.num_classes(), 13);
        set.clear(&mut pool);
        assert_eq!(set.num_classes(), 0);
        for &id in &ids[1..15] {
            // the creator's reference is left
            assert!(pool.at(id).is_some());
        }
    }

    #[test]
    fn peer_proportional_formula() {
        // no uTP peers, or fewer than five busy peers: unlimited
        assert_eq!(peer_proportional_limits([[10, 10], [0, 0]], [1_000_000, 1_000_000]), [0, 0]);
        assert_eq!(peer_proportional_limits([[2, 2], [2, 2]], [1_000_000, 1_000_000]), [0, 0]);
        // 2 TCP and 3 uTP peers at 1 MB/s: 4 * 2/5 of the rate
        assert_eq!(
            peer_proportional_limits([[2, 2], [3, 3]], [1_000_000, 1_000_000]),
            [1_600_000, 1_600_000]
        );
        // only uTP peers busy: one TCP peer is assumed
        assert_eq!(peer_proportional_limits([[0, 0], [8, 8]], [1_000_000, 2_000_000]), [500_000, 1_000_000]);
        // the floors: 5 kB/s up, 30 kB/s down
        assert_eq!(peer_proportional_limits([[1, 1], [9, 9]], [1000, 1000]), [5000, 30000]);
        // per channel
        assert_eq!(peer_proportional_limits([[2, 0], [3, 0]], [100_000, 100_000]), [160_000, 0]);
    }

    #[test]
    fn mask_of_ids_past_31_is_empty() {
        assert_eq!(PeerClassId(31).mask(), 1 << 31);
        assert_eq!(PeerClassId(32).mask(), 0);
        let mut f = PeerClassTypeFilter::new();
        f.add(SocketType::Tcp, PeerClassId(40));
        assert_eq!(f, PeerClassTypeFilter::new());
    }
}
