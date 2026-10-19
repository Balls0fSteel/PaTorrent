// Ported from libtorrent-rasterbar 2.0.11: src/bt_peer_connection.cpp (on_connected's encryption
// policy, the read_pe_* dispatch of on_receive_impl, init_bt_handshake's pe_support toggle,
// attach_to_torrent for incoming connections)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: engine methods on a peer slot driving the sans-IO `proto::pe_handshake` state machine;
// the torrent lookup goes through the engine's obfuscated-hash map.

//! MSE/PE policy and glue: when an outgoing connection starts encrypted, and how an incoming
//! encrypted handshake finds and joins its torrent.

use std::time::Instant;

use super::Engine;
use crate::proto::pe_handshake::{PeHandshake, PeStep};
use crate::proto::peer_wire;
use crate::settings::EncPolicy;
use crate::status::PeerSource;
use crate::types::{Sha1Hash, TorrentId};

impl Engine {
    /// An outgoing TCP connection is up (libtorrent `bt_peer_connection::on_connected`): start
    /// the encrypted handshake or the plain one, as `out_enc_policy` says. Under `Enabled` the
    /// two alternate per peer: a plaintext attempt first (`pe_support` starts false), encrypted
    /// after that one fails, and so on, each success pinning the kind that worked.
    pub(super) fn start_outgoing_handshake(&mut self, slot: usize) {
        let policy = self.settings.out_enc_policy;
        let (level, prefer_rc4) = (self.settings.allowed_enc_level, self.settings.prefer_rc4);
        let peer_id = self.peer_id;
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        let encrypted = match policy {
            EncPolicy::Forced => true,
            EncPolicy::Disabled => false,
            EncPolicy::Enabled => {
                let supported = t.peer_list.pe_support(&p.addr);
                // Toggled; toggled back once this handshake completes.
                t.peer_list.set_pe_support(&p.addr, !supported);
                // If this fails, we need to reconnect fast, the other way. libtorrent flags only
                // the encrypted attempt, because its peer list retries a cleanly closed peer at
                // once anyway; ours waits `min_reconnect_time` unless told not to.
                p.fast_reconnect = true;
                supported
            }
        };
        if !encrypted {
            self.send_handshake(slot);
            return;
        }
        let ih = t.info_hashes.best_v1_compatible();
        let mut ia = Vec::with_capacity(peer_wire::HANDSHAKE_LEN);
        peer_wire::encode_handshake(&mut ia, &ih, &peer_id);
        // The plain handshake goes inside the encrypted one (libtorrent writes it with pe3).
        p.sent_handshake = true;
        let hs = PeHandshake::outgoing(ih, ia, level, prefer_rc4, p.out());
        p.pe = Some(Box::new(hs));
        self.to_flush.push(slot);
    }

    /// Bytes arrived during the encrypted handshake. Returns true when progress was made and the
    /// buffer should be looked at again.
    pub(super) fn on_pe_receive(&mut self, slot: usize) -> bool {
        let out_policy = self.settings.out_enc_policy;
        let step = {
            let by_obfuscated = &self.by_obfuscated;
            let Some(p) = self.peers.get_mut(slot) else { return false };
            let mut find = |obfuscated: &Sha1Hash| by_obfuscated.get(obfuscated).copied();
            p.pe_step(&mut find)
        };
        match step {
            Ok(PeStep::Wait) => false,
            Ok(PeStep::Continue) => {
                self.to_flush.push(slot);
                true
            }
            Ok(PeStep::Torrent(ih)) => {
                let Some(&id) = self.by_hash.get(&ih) else {
                    self.peers.get_mut(slot).expect("exists").close("unknown torrent", true);
                    return false;
                };
                self.attach_incoming(slot, id)
            }
            Ok(PeStep::Done { rc4: _ }) => {
                // The encrypted part completed: remember that this peer speaks MSE/PE
                // (libtorrent `init_bt_handshake`).
                if let Some((p, Some(t))) = self.peer_and_torrent(slot) {
                    if p.outgoing && out_policy == EncPolicy::Enabled {
                        t.peer_list.set_pe_support(&p.addr, true);
                    }
                }
                self.to_flush.push(slot);
                true
            }
            Err(e) => {
                self.peers.get_mut(slot).expect("exists").close(e.to_string(), true);
                false
            }
        }
    }

    /// An incoming connection named one of our torrents (libtorrent `attach_to_torrent`):
    /// joins it if the torrent takes peers and this address is not connected already.
    pub(super) fn attach_incoming(&mut self, slot: usize, id: TorrentId) -> bool {
        let accept = self.torrent_ref(id).is_some_and(|t| t.is_active() && !t.checking && self.is_running(t));
        let Some(p) = self.peers.get(slot) else { return false };
        let (addr, pid) = (p.addr, p.id);
        let duplicate = self
            .torrent_ref(id)
            .is_some_and(|t| t.peers.iter().any(|&s| self.peers.get(s).is_some_and(|o| o.addr == addr)));
        if !accept || duplicate {
            self.peers.get_mut(slot).expect("exists").close("torrent not accepting peers", false);
            return false;
        }
        // libtorrent `torrent::attach_peer`: `too_many_connections` at the torrent's own limit
        if self
            .torrent_ref(id)
            .is_some_and(|t| t.max_connections.is_some_and(|m| t.peers.len() >= m as usize))
        {
            self.peers.get_mut(slot).expect("exists").close("too many connections", false);
            return false;
        }
        if self.attach_blocked(id, addr) {
            self.peers.get_mut(slot).expect("exists").close("blocked by the IP filter", false);
            return false;
        }
        self.peers.get_mut(slot).expect("exists").torrent = Some(id);
        let t = self.torrent(id).expect("exists");
        t.peers.push(slot);
        t.peer_list.connecting(addr, pid, PeerSource::INCOMING, Instant::now());
        true
    }
}
