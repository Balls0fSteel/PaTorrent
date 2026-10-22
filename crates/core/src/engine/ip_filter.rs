// Ported from libtorrent-rasterbar 2.0.11: src/session_impl.cpp (set_ip_filter,
// set_port_filter, the filter check in incoming_connection), src/torrent.cpp (add_peer,
// attach_peer, ip_filter_updated, port_filter_updated, the tracker request filter),
// src/peer_list.cpp (apply_ip_filter, apply_port_filter)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: torrents read the session's filters instead of holding their own pointer; the
// `non_filter_torrents` counter is replaced by a scan of the torrents, done only when an
// incoming address is blocked. Peers from resume data are filtered when they are about to be
// connected rather than when they are loaded.

//! The session's IP and port filters applied to peers and trackers.

use std::net::SocketAddr;
use std::sync::Arc;

use super::Engine;
use crate::alert::{Alert, BlockReason};
use crate::net::ip_filter::{IpFilter, PortFilter};
use crate::params::TorrentFlags;
use crate::status::PeerSource;
use crate::types::TorrentId;

/// The session's filters (libtorrent `session_impl::m_ip_filter`, `m_port_filter`).
#[derive(Default)]
pub(super) struct Filters {
    pub ip: Arc<IpFilter>,
    pub port: PortFilter,
}

impl Filters {
    /// Why `addr` may not be a peer of a torrent with `flags` (libtorrent `torrent::add_peer`).
    /// The port filter is about listen ports, so `port` is false for a peer known only from
    /// its incoming connection, whose port is an ephemeral one.
    fn check(&self, flags: TorrentFlags, addr: &SocketAddr, port: bool) -> Option<BlockReason> {
        if flags.contains(TorrentFlags::APPLY_IP_FILTER) && self.ip.is_blocked(addr.ip()) {
            Some(BlockReason::IpFilter)
        } else if port && self.port.is_blocked(addr.port()) {
            Some(BlockReason::PortFilter)
        } else {
            None
        }
    }
}

impl Engine {
    /// libtorrent `session_impl::set_ip_filter`: every torrent drops the peers it now blocks.
    pub(super) fn set_ip_filter(&mut self, filter: Arc<IpFilter>) {
        self.filters.ip = filter;
        for id in self.torrent_ids() {
            self.ip_filter_updated(id);
        }
    }

    pub(super) fn set_port_filter(&mut self, filter: PortFilter) {
        self.filters.port = filter;
        for id in self.torrent_ids() {
            self.ip_filter_updated(id);
        }
    }

    /// libtorrent `torrent::ip_filter_updated` and `port_filter_updated`
    /// (`peer_list::apply_ip_filter`): closes the torrent's connections to blocked peers and
    /// forgets every blocked peer it knows.
    pub(super) fn ip_filter_updated(&mut self, id: TorrentId) {
        let Engine { filters, slots, peers, out, .. } = self;
        let Some(t) = slots
            .get_mut(id.index() as usize)
            .and_then(|s| (s.generation == id.generation()).then_some(s.torrent.as_deref_mut()).flatten())
        else {
            return;
        };
        let flags = t.flags;
        let removed = t
            .peer_list
            .remove_if(|k| filters.check(flags, &k.addr, k.source != PeerSource::INCOMING).is_some());
        if removed > 0 {
            t.status_dirty = true;
        }
        for &slot in &t.peers {
            let Some(p) = peers.get_mut(slot) else { continue };
            let Some(reason) = filters.check(flags, &p.addr, p.outgoing) else { continue };
            if p.closing.is_none() {
                p.close(format!("blocked by the {}", reason_name(reason)), false);
                out.push(Alert::PeerBlocked { id: Some(id), addr: p.addr, reason });
            }
        }
    }

    /// libtorrent `torrent::add_peer`: adds `addr` to the torrent's peer list unless a filter
    /// blocks it. Returns false if the peer was not added.
    pub(super) fn add_peer(&mut self, id: TorrentId, addr: SocketAddr, source: PeerSource) -> bool {
        let max = self.settings.max_peerlist_size.max(1) as usize;
        let ext = self.external_ip;
        let Some(t) = self.torrent(id) else { return false };
        let flags = t.flags;
        if let Some(reason) = self.filters.check(flags, &addr, true) {
            self.alert(Alert::PeerBlocked { id: Some(id), addr, reason });
            return false;
        }
        let t = self.torrent(id).expect("exists");
        t.peer_list.add(addr, source, ext, max)
    }

    /// The last check before connecting to `addr` (peers loaded from resume data were never
    /// filtered): a blocked peer is forgotten instead.
    pub(super) fn outgoing_blocked(&mut self, id: TorrentId, addr: SocketAddr) -> bool {
        let Some(t) = self.torrent_ref(id) else { return true };
        let Some(reason) = self.filters.check(t.flags, &addr, true) else { return false };
        let t = self.torrent(id).expect("exists");
        t.peer_list.remove_if(|k| k.addr == addr);
        self.alert(Alert::PeerBlocked { id: Some(id), addr, reason });
        true
    }

    /// libtorrent `session_impl::incoming_connection`: refuse a blocked address at once unless
    /// some torrent ignores the filter (the connection might be for that one; `attach_incoming`
    /// checks again once the torrent is known).
    pub(super) fn incoming_blocked(&mut self, addr: SocketAddr) -> bool {
        if !self.filters.ip.is_blocked(addr.ip()) {
            return false;
        }
        let exempt = self
            .slots
            .iter()
            .filter_map(|s| s.torrent.as_deref())
            .any(|t| !t.flags.contains(TorrentFlags::APPLY_IP_FILTER));
        if exempt {
            return false;
        }
        self.alert(Alert::PeerBlocked { id: None, addr, reason: BlockReason::IpFilter });
        true
    }

    /// libtorrent `torrent::attach_peer`: an incoming peer that turned out to be for a torrent
    /// that applies the filter.
    pub(super) fn attach_blocked(&mut self, id: TorrentId, addr: SocketAddr) -> bool {
        let Some(t) = self.torrent_ref(id) else { return false };
        if !(t.flags.contains(TorrentFlags::APPLY_IP_FILTER) && self.filters.ip.is_blocked(addr.ip())) {
            return false;
        }
        self.alert(Alert::PeerBlocked { id: Some(id), addr, reason: BlockReason::IpFilter });
        true
    }

    /// The filter a torrent's tracker requests drop blocked tracker addresses with (libtorrent
    /// `tracker_request::filter`), if `apply_ip_filter_to_trackers` is on and the torrent
    /// applies the filter.
    pub(super) fn tracker_filter(&self, id: TorrentId) -> Option<Arc<IpFilter>> {
        let t = self.torrent_ref(id)?;
        (self.settings.apply_ip_filter_to_trackers
            && t.flags.contains(TorrentFlags::APPLY_IP_FILTER)
            && !self.filters.ip.is_empty())
        .then(|| self.filters.ip.clone())
    }

    /// Adds peers from a tracker, the DHT, LSD or the user, then connects to some.
    pub(super) fn add_peers(&mut self, id: TorrentId, addrs: &[SocketAddr], source: PeerSource) {
        for &a in addrs {
            self.add_peer(id, a, source);
        }
        if let Some(t) = self.torrent(id) {
            t.status_dirty = true;
        }
    }
}

fn reason_name(r: BlockReason) -> &'static str {
    match r {
        BlockReason::IpFilter => "IP filter",
        BlockReason::PortFilter => "port filter",
    }
}
