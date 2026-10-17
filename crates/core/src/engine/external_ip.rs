// Ported from libtorrent-rasterbar 2.0.11: src/session_impl.cpp (set_external_address,
// external_address), src/torrent.cpp (new_external_ip)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: one voter per address family instead of one per listen socket (`SessionVoter`).
// Router votes go through `set_external_address` too, so they post `ExternalIp` and move the
// DHT id (libtorrent casts them on the listen socket's voter directly). The alert carries the
// newly voted address rather than the address of the vote that tipped it.

//! The session's external address: trackers, peers, the DHT and the routers vote on it.

use std::net::IpAddr;
use std::time::Instant;

use super::Engine;
use crate::alert::Alert;

impl Engine {
    /// libtorrent `session_impl::set_external_address`: `source` (of kind `source_type`, see
    /// `net::ip_voter::source`) says we are `ip`. When the vote changes our address, the
    /// torrents re-rank their peers (BEP 40) and the DHT node of that family takes a new id.
    pub(super) fn set_external_address(&mut self, ip: IpAddr, source_type: u8, source: IpAddr) {
        let now = Instant::now();
        let Some(ext) = self.ip_voter.cast_vote(ip, source_type, source, now) else { return };
        self.alert(Alert::Log {
            id: None,
            message: format!(
                "external address updated [ new-ip: {ext} type: {source_type} last-voter: {source} ]"
            ),
        });
        self.alert(Alert::ExternalIp { ip: ext });
        self.update_external_ip();
        let e = self.external_ip;
        for s in &mut self.slots {
            if let Some(t) = s.torrent.as_mut() {
                t.peer_list.clear_peer_prio(&e);
            }
        }
        if let Some(t) = self.dht.tracker.as_mut() {
            t.update_node_id(ext.is_ipv4(), ext, now);
        }
    }

    /// Recomputes `external_ip` (libtorrent `session_impl::external_address`) from the votes
    /// and the listen addresses.
    pub(super) fn update_external_ip(&mut self) {
        let listen: Vec<IpAddr> =
            self.listeners.iter().filter_map(|l| l.local_addr().ok()).map(|a| a.ip()).collect();
        self.external_ip = self.ip_voter.external_ip(listen);
    }
}
