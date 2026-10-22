// Ported from libtorrent-rasterbar 2.0.11: src/ut_pex.cpp (send_peer, the plugins' tick,
// on_extension_handshake, on_extended), src/peer_list.cpp (pex flags in insert_peer /
// update_peer)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: engine methods taking a peer slot instead of plugin objects. Peers learned through
// PEX wait for the regular connect pacing instead of `do_connect_boost`. ut_pex is off as soon
// as a torrent is known to be private, including a magnet whose metadata turns out private
// (libtorrent decides once, when the torrent is created).

//! BEP 11: exchanging peer lists with connected peers.

use std::net::SocketAddr;
use std::time::Instant;

use super::Engine;
use super::peer::{PeerConn, PeerState};
use super::torrent::Torrent;
use crate::params::TorrentFlags;
use crate::proto::peer_wire;
use crate::proto::ut_pex::{self, Send, flags};
use crate::status::PeerSource;
use crate::types::TorrentId;

/// ut_pex is never used for private torrents.
pub(super) fn pex_allowed(t: &Torrent) -> bool {
    !t.ti.as_ref().is_some_and(|ti| ti.is_private())
}

/// Allowed, and not switched off for this torrent (`disable_pex`).
fn pex_enabled(t: &Torrent) -> bool {
    pex_allowed(t) && !t.flags.contains(TorrentFlags::DISABLE_PEX)
}

/// libtorrent `send_peer`: the address we tell others about, for connected peers whose listen
/// port we know (the one we connected to, or the `p` of an incoming peer's handshake), with
/// its flags.
fn advertised(p: &PeerConn) -> Option<(SocketAddr, u8)> {
    if p.state != PeerState::Active || p.closing.is_some() {
        return None;
    }
    let addr = if p.outgoing {
        p.addr
    } else {
        let port = p.ext.as_ref()?.listen_port.filter(|&port| port != 0)?;
        SocketAddr::new(p.addr.ip(), port)
    };
    let mut fl = if p.peer_seed { flags::SEED } else { 0 };
    if p.encrypted {
        fl |= flags::ENCRYPTION; // libtorrent `supports_encryption()`: the peer did MSE/PE
    }
    if p.sock.is_utp() {
        fl |= flags::UTP; // libtorrent `is_utp(p->get_socket())`
    }
    Some((addr, fl))
}

impl Engine {
    /// The peer's extension handshake arrived (libtorrent `on_extension_handshake`).
    pub(super) fn on_pex_handshake(&mut self, slot: usize) {
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        let id = p.ext.as_ref().and_then(|e| e.message_id(ut_pex::NAME));
        p.pex.message_id = if pex_allowed(t) { id.unwrap_or(0) } else { 0 };
    }

    /// An extended message with our ut_pex id (libtorrent `on_extended`).
    pub(super) fn on_pex_message(&mut self, slot: usize, payload: &[u8]) {
        let now = Instant::now();
        let max = self.settings.max_pex_peers.max(0) as usize;
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        let id = t.id;
        if !pex_enabled(t) {
            return;
        }
        let peers = match p.pex.on_message(payload, now, p.addr.ip(), max) {
            Ok(peers) => peers,
            Err(e) => {
                p.close(e.to_string(), true);
                return;
            }
        };
        for (addr, fl) in peers {
            if !self.add_peer(id, addr, PeerSource::PEX) {
                continue;
            }
            let Some(t) = self.torrent(id) else { return };
            // A connected peer already told us whether it is a seed.
            if fl & flags::SEED != 0 && t.peer_list.get(&addr).is_some_and(|k| k.connection.is_none()) {
                t.peer_list.set_seed(&addr, true);
            }
            // libtorrent `peer_list::update_peer`: try encryption first with this peer.
            if fl & flags::ENCRYPTION != 0 {
                t.peer_list.set_pe_support(&addr, true);
            }
            // a positive flag only: its absence says nothing
            if fl & flags::UTP != 0 {
                t.peer_list.set_supports_utp(&addr);
            }
            t.status_dirty = true;
        }
    }

    /// The torrent's and its peers' PEX tick: rebuilds the diff once a minute and sends each
    /// peer its full list first, then the diffs.
    pub(super) fn pex_tick(&mut self, id: TorrentId, now: Instant) {
        let Some(t) = self.torrent_ref(id) else { return };
        if !pex_enabled(t) {
            return;
        }
        let slots = t.peers.clone();
        let num_peers = slots.len();
        let list: Vec<(SocketAddr, u8)> =
            slots.iter().filter_map(|&s| self.peers.get(s).and_then(advertised)).collect();
        let t = self.torrent(id).expect("exists");
        let pex = t.pex.get_or_insert_with(Box::default);
        pex.tick(now, num_peers, list.iter().copied());
        let diff = pex.diff().map(<[u8]>::to_vec);
        let full = std::cell::OnceCell::new();
        for slot in slots {
            let Some(p) = self.peers.get_mut(slot) else { continue };
            if p.state != PeerState::Active || p.closing.is_some() {
                continue;
            }
            let ext_id = p.pex.message_id;
            match p.pex.tick(now, num_peers) {
                None => continue,
                Some(Send::Full) => {
                    let m = full.get_or_init(|| ut_pex::encode_full(list.iter().copied()));
                    peer_wire::encode_extended(p.out(), ext_id, m);
                }
                Some(Send::Diff) => {
                    let Some(m) = diff.as_ref() else { continue };
                    peer_wire::encode_extended(p.out(), ext_id, m);
                }
            }
            self.to_flush.push(slot);
        }
    }
}
