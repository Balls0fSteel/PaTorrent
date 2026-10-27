// Ported from libtorrent-rasterbar 2.0.11: src/bt_peer_connection.cpp (on_* message handlers,
// write_bitfield, write_extensions, write_dont_have, the lt_donthave branch of on_extended),
// src/peer_connection.cpp (incoming_request, incoming_piece, incoming_have, incoming_dont_have,
// update_interest, send_block_requests, snub handling, the read error of
// on_disk_read_complete, incoming_bitfield / incoming_have_all / incoming_have_none replacing an
// earlier piece set), src/request_blocks.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: handlers are engine methods taking a peer slot; messages are copied out of the
// receive buffer once (piece payloads go straight to the disk job). The encrypted handshake
// (`read_pe_*`) runs in `engine::pe` before the plain one.

//! Peer wire message handling.

use std::time::Instant;

use super::Engine;
use super::peer::{PeerState, Request};
use crate::alert::Alert;
use crate::disk::BLOCK_SIZE;
use crate::error::Error;
use crate::params::TorrentFlags;
use crate::proto::pe_handshake::PeHandshake;
use crate::proto::peer_wire::{self, HandshakeError, HashMsg, Message, lt_donthave, msg};
use crate::proto::ut_metadata;
use crate::proto::ut_pex;
use crate::settings::EncPolicy;
use crate::util::bitfield::Bitfield;

/// Largest block a peer may request from us (libtorrent rejects anything above 16 KiB too,
/// except it tolerates up to the block size; some clients ask for 32 KiB).
const MAX_REQUEST: u32 = 128 * 1024;

/// libtorrent `allowed_fast_set_size` default.
const ALLOWED_FAST_SET_SIZE: usize = 5;

/// libtorrent `max_piece_count` default: `have` messages beyond this are ignored while the
/// piece count is unknown.
const MAX_PIECE_COUNT: u32 = 0x20_0000;

enum Owned {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request(u32, u32, u32),
    Piece(u32, u32, Vec<u8>),
    Cancel(u32, u32, u32),
    HaveAll,
    HaveNone,
    Reject(u32, u32),
    AllowedFast(u32),
    Extended(u8, Vec<u8>),
    HashRequest(HashMsg),
    Hashes(HashMsg, Vec<u8>),
    HashReject(HashMsg),
    Ignored,
}

impl Owned {
    fn from(m: Message<'_>) -> Owned {
        match m {
            Message::KeepAlive => Owned::KeepAlive,
            Message::Choke => Owned::Choke,
            Message::Unchoke => Owned::Unchoke,
            Message::Interested => Owned::Interested,
            Message::NotInterested => Owned::NotInterested,
            Message::Have(i) => Owned::Have(i),
            Message::Bitfield(b) => Owned::Bitfield(b.to_vec()),
            Message::Request { piece, start, length } => Owned::Request(piece, start, length),
            Message::Piece { piece, start, data } => Owned::Piece(piece, start, data.to_vec()),
            Message::Cancel { piece, start, length } => Owned::Cancel(piece, start, length),
            Message::HaveAll => Owned::HaveAll,
            Message::HaveNone => Owned::HaveNone,
            Message::Reject { piece, start, .. } => Owned::Reject(piece, start),
            Message::AllowedFast(i) => Owned::AllowedFast(i),
            Message::Extended { id, payload } => Owned::Extended(id, payload.to_vec()),
            Message::HashRequest(m) => Owned::HashRequest(m),
            Message::Hashes(m, hashes) => Owned::Hashes(m, hashes.to_vec()),
            Message::HashReject(m) => Owned::HashReject(m),
            Message::Port(_) | Message::Suggest(_) | Message::Unknown { .. } => Owned::Ignored,
        }
    }
}

impl Engine {
    /// Handles every complete message in the peer's receive buffer. Returns the piece payload
    /// bytes received (the rest of the traffic counts as protocol overhead).
    pub(super) fn process_messages(&mut self, slot: usize) -> u64 {
        let mut payload = 0u64;
        loop {
            let Some(p) = self.peers.get_mut(slot) else { return payload };
            if p.closing.is_some() {
                return payload;
            }
            match p.state {
                PeerState::Connecting => return payload,
                PeerState::Handshake => {
                    let progressed =
                        if p.pe.is_some() { self.on_pe_receive(slot) } else { self.on_handshake(slot) };
                    if !progressed {
                        return payload;
                    }
                }
                PeerState::Active => {
                    let decoded = match peer_wire::decode(p.received()) {
                        Ok(Some((m, n))) => Some((Owned::from(m), n)),
                        Ok(None) => None,
                        Err(e) => {
                            p.close(format!("protocol error: {e}"), true);
                            return payload;
                        }
                    };
                    let Some((m, n)) = decoded else { return payload };
                    p.consume(n);
                    if let Owned::Piece(_, _, d) = &m {
                        payload += d.len() as u64;
                    }
                    self.on_message(slot, m);
                }
            }
        }
    }

    /// Returns true when the handshake completed (or an encrypted one starts) and the buffer
    /// should be looked at again.
    fn on_handshake(&mut self, slot: usize) -> bool {
        let our_id = self.peer_id;
        let (in_policy, out_policy) = (self.settings.in_enc_policy, self.settings.out_enc_policy);
        let (level, prefer_rc4) = (self.settings.allowed_enc_level, self.settings.prefer_rc4);
        let p = self.peers.get_mut(slot).expect("caller checked");
        if !p.outgoing && !p.sent_handshake {
            // Incoming: pick the torrent by its info-hash as soon as it arrives.
            let (reserved, ih) = match peer_wire::decode_handshake_prefix(p.received()) {
                Ok(prefix) => prefix,
                Err(HandshakeError::Incomplete) => return false,
                Err(HandshakeError::BadProtocol) => {
                    // libtorrent `read_protocol_identifier`: not the protocol string, so try it
                    // as an encrypted handshake, unless that cannot be (it is already inside
                    // one) or `in_enc_policy` forbids it.
                    if p.encrypted || p.torrent.is_some() {
                        p.close("not a BitTorrent handshake", true);
                        return false;
                    }
                    if in_policy == EncPolicy::Disabled {
                        p.close("no incoming encrypted connections allowed", false);
                        return false;
                    }
                    p.pe = Some(Box::new(PeHandshake::incoming(level, prefer_rc4)));
                    return true;
                }
            };
            if p.torrent.is_none() {
                if in_policy == EncPolicy::Forced && !p.encrypted {
                    p.close("no incoming plaintext connections allowed", false);
                    return false;
                }
                let Some(&id) = self.by_hash.get(&ih) else {
                    self.peers.get_mut(slot).expect("exists").close("unknown torrent", true);
                    return false;
                };
                if !self.attach_incoming(slot, id) {
                    return false;
                }
            }
            // libtorrent `on_receive_handshake`: the truncated v2 hash makes it a v2 connection,
            // and so does the v2 bit on a v2 torrent (the peer offers the upgrade, and our
            // answer carries the v2 hash); the bit on a torrent known to be v1 is an error.
            let (p, t) = self.peer_and_torrent(slot).expect("exists");
            let Some(t) = t else {
                p.close("torrent removed", false);
                return false;
            };
            if t.info_hashes.truncated_v2() == Some(ih) || !t.info_hashes.has_v1() {
                p.protocol_v2 = true;
            }
            if reserved.supports_v2() {
                if t.has_metadata() && !t.info_hashes.has_v2() {
                    p.close("v2 handshake for a v1 torrent", true);
                    return false;
                }
                p.protocol_v2 |= t.info_hashes.has_v2();
            }
        }
        // An incoming connection answers once its torrent is known (after the encrypted
        // handshake, if any, so the answer goes out under the agreed cipher).
        if !self.peers.get(slot).expect("exists").sent_handshake {
            self.send_handshake(slot);
        }
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let Some(t) = t else {
            p.close("torrent removed", false);
            return false;
        };
        let h = match peer_wire::decode_handshake(p.received()) {
            Ok(h) => h,
            Err(HandshakeError::Incomplete) => return false,
            Err(HandshakeError::BadProtocol) => {
                p.close("not a BitTorrent handshake", true);
                return false;
            }
        };
        // An outgoing v1 connection of a hybrid is upgraded when the peer answers with the v2
        // hash; an incoming one chose its hash above.
        let valid = if p.outgoing {
            let v2 = t.info_hashes.truncated_v2() == Some(h.info_hash);
            p.protocol_v2 |= v2;
            v2 || h.info_hash == t.info_hashes.wire_hash(p.protocol_v2)
        } else {
            t.info_hashes.wire_hashes().any(|w| w == h.info_hash)
        };
        if !valid {
            p.close("info-hash mismatch", true);
            return false;
        }
        if h.peer_id == our_id {
            p.close("connected to ourselves", true);
            t.peer_list.ban(&p.addr);
            return false;
        }
        // libtorrent `bt_peer_connection::on_receive` (read_peer_id): another connection to
        // the same peer, reached at another of its addresses (LSD finds a multi-homed host on
        // each of them). The greater peer-id is the one allowed to initiate, so both ends close
        // the same connection.
        let (pid, outgoing, others) = (h.peer_id, p.outgoing, t.peers.clone());
        let dup = others.into_iter().find(|&s| {
            s != slot
                && self
                    .peers
                    .get(s)
                    .is_some_and(|o| o.state == PeerState::Active && o.closing.is_none() && o.peer_id == pid)
        });
        if let Some(other) = dup {
            if (pid < our_id) == outgoing {
                self.peers.get_mut(other).expect("exists").close("duplicate peer-id", false);
            } else {
                self.peers.get_mut(slot).expect("exists").close("duplicate peer-id", false);
                return false;
            }
        }
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let t = t.expect("checked");
        p.consume(peer_wire::HANDSHAKE_LEN);
        p.peer_id = h.peer_id;
        p.client = peer_wire::identify_client(&h.peer_id);
        p.supports_fast = h.reserved.supports_fast();
        p.supports_extensions = h.reserved.supports_extensions();
        p.state = PeerState::Active;
        // The handshake is what the fast retry was for; a later close waits as usual.
        p.fast_reconnect = false;
        p.peer_has = Bitfield::new(t.num_pieces as usize, false);
        p.last_receive = Instant::now();
        // A plain handshake succeeded under `Enabled`: next time, start plain again
        // (libtorrent `read_peer_id` toggles `pe_support` back).
        if p.outgoing && !p.encrypted && out_policy == EncPolicy::Enabled {
            t.peer_list.set_pe_support(&p.addr, false);
        }
        if !p.outgoing {
            self.has_incoming = true;
        }

        // Our piece set, then the extension handshake.
        self.write_bitfield(slot);
        let listen_port = self.announce_port();
        let reqq = self.settings.max_allowed_in_request_queue.max(1) as u32;
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let t = t.expect("checked");
        let have = t.picker.as_ref().map(|pk| pk.have().clone()).unwrap_or_default();
        let count = have.count();
        let super_seeding = t.super_seeding();
        // BEP 6 allowed-fast set for IPv4 peers, limited to pieces we have; none while super
        // seeding (libtorrent `send_allowed_set`).
        if p.supports_fast && count > 0 && !super_seeding {
            if let std::net::IpAddr::V4(ip) = p.addr.ip() {
                let ih = t.info_hashes.wire_hash(p.protocol_v2);
                for piece in peer_wire::allowed_fast_set(ip, &ih, t.num_pieces, ALLOWED_FAST_SET_SIZE) {
                    if have.get(piece as usize) {
                        peer_wire::encode_u32(p.out(), msg::ALLOWED_FAST, piece);
                        p.fast_set.push(piece);
                    }
                }
            }
        }
        if p.supports_extensions {
            // lt_donthave is built in; ut_metadata and ut_pex are left out for private
            // torrents (libtorrent adds no plugins for them).
            let private = t.ti.as_ref().is_some_and(|ti| ti.is_private());
            let mut messages = vec![(lt_donthave::NAME, lt_donthave::OUR_ID)];
            if !private {
                messages.push((ut_pex::NAME, ut_pex::OUR_ID));
                messages.push((ut_metadata::NAME, ut_metadata::OUR_ID));
            }
            let metadata_size = t.ti.as_ref().filter(|_| !private).map(|ti| ti.info_section().len());
            let ext = peer_wire::encode_ext_handshake(
                &messages,
                &format!("PaTorrent {}", crate::VERSION),
                (listen_port != 0).then_some(listen_port),
                reqq,
                // a super seed is not upload-only: the peer would leave after one piece
                count == have.len() && count > 0 && !super_seeding,
                Some(p.addr.ip()),
                metadata_size,
            );
            peer_wire::encode_extended(p.out(), 0, &ext);
        }
        t.status_dirty = true;
        self.to_flush.push(slot);
        self.maybe_send_hash_request(slot);
        true
    }

    fn on_message(&mut self, slot: usize, m: Owned) {
        // libtorrent asks a v2 peer for hashes once its piece set is known or grows
        let poke = matches!(m, Owned::Have(_) | Owned::HaveAll | Owned::Bitfield(_));
        self.handle_message(slot, m);
        if poke {
            self.maybe_send_hash_request(slot);
        }
    }

    fn handle_message(&mut self, slot: usize, m: Owned) {
        match m {
            Owned::KeepAlive | Owned::Ignored => {}
            Owned::HashRequest(m) => self.on_hash_request(slot, m),
            Owned::Hashes(m, hashes) => self.on_hashes(slot, m, &hashes),
            Owned::HashReject(m) => self.on_hash_reject(slot, m),
            Owned::Choke => {
                let (p, t) = self.peer_and_torrent(slot).expect("exists");
                p.peer_choking = true;
                if !p.supports_fast {
                    // Without the fast extension a choke cancels everything we asked for.
                    if let Some(picker) = t.and_then(|t| t.picker.as_mut()) {
                        for r in p.requests.drain(..) {
                            picker.abort_download(r.piece, r.block);
                        }
                    } else {
                        p.requests.clear();
                    }
                }
            }
            Owned::Unchoke => {
                let p = self.peers.get_mut(slot).expect("exists");
                p.peer_choking = false;
                p.last_piece = Instant::now();
                self.request_blocks(slot);
            }
            Owned::Interested => {
                self.peers.get_mut(slot).expect("exists").peer_interested = true;
                self.maybe_unchoke(slot);
            }
            Owned::NotInterested => {
                let p = self.peers.get_mut(slot).expect("exists");
                if p.peer_interested {
                    p.became_uninterested = Instant::now();
                }
                p.peer_interested = false;
                self.set_choke(slot, true, Instant::now());
            }
            Owned::Have(i) => {
                let close_redundant = self.settings.close_redundant_connections;
                let (p, t) = self.peer_and_torrent(slot).expect("exists");
                let Some(t) = t else { return };
                if !t.has_metadata() {
                    // libtorrent `incoming_have` without metadata: grow the map to fit.
                    if i < MAX_PIECE_COUNT && !p.peer_seed {
                        if i as usize >= p.peer_has.len() {
                            p.peer_has.resize(i as usize + 1, false);
                        }
                        p.peer_has.set(i as usize, true);
                        p.got_bitfield = true;
                    }
                    return;
                }
                if i >= t.num_pieces {
                    p.close(format!("have for invalid piece {i}"), true);
                    return;
                }
                if !p.peer_seed && !p.peer_has.get(i as usize) {
                    p.peer_has.set(i as usize, true);
                    p.got_bitfield = true;
                    let mut interesting = false;
                    if let Some(pk) = t.picker.as_mut() {
                        pk.inc_refcount(i);
                        interesting = pk.wants(i);
                    }
                    if p.peer_has.all_set() && t.is_seed() && close_redundant {
                        p.close("both sides are seeds", false);
                        return;
                    }
                    if interesting && !p.am_interested {
                        self.update_interest(slot);
                    }
                }
                // Also for a redundant HAVE, as libtorrent.
                self.superseed_on_have(slot, i);
            }
            Owned::Bitfield(bytes) => {
                let (p, t) = self.peer_and_torrent(slot).expect("exists");
                let Some(t) = t else { return };
                if !t.has_metadata() {
                    // Kept at its wire length until the piece count is known.
                    p.peer_has = Bitfield::from_bytes(&bytes, bytes.len() * 8).expect("exact length");
                    p.got_bitfield = true;
                    return;
                }
                let Some(bf) = Bitfield::from_bytes(&bytes, t.num_pieces as usize) else {
                    p.close("invalid bitfield size", true);
                    return;
                };
                p.got_bitfield = true;
                self.forget_peer_pieces(slot);
                self.set_peer_pieces(slot, bf, false);
            }
            Owned::HaveAll => {
                let (p, t) = self.peer_and_torrent(slot).expect("exists");
                let Some(t) = t else { return };
                if !p.supports_fast {
                    p.close("unexpected have_all", true);
                    return;
                }
                p.got_bitfield = true;
                let n = t.num_pieces as usize;
                self.forget_peer_pieces(slot);
                self.set_peer_pieces(slot, Bitfield::new(n, true), true);
            }
            Owned::HaveNone => {
                let p = self.peers.get_mut(slot).expect("exists");
                if !p.supports_fast {
                    p.close("unexpected have_none", true);
                    return;
                }
                let had = p.got_bitfield;
                p.got_bitfield = true;
                if had {
                    self.forget_peer_pieces(slot);
                    self.update_interest(slot);
                }
            }
            Owned::Request(piece, start, length) => self.on_request(slot, piece, start, length),
            Owned::Piece(piece, start, data) => self.on_piece(slot, piece, start, data),
            Owned::Cancel(piece, start, length) => {
                let p = self.peers.get_mut(slot).expect("exists");
                let key = (piece, start, length);
                let before = p.upload_queue.len();
                p.upload_queue.retain(|&r| r != key);
                let removed = before != p.upload_queue.len() || p.reads_in_flight.remove(&key);
                if removed && p.supports_fast {
                    peer_wire::encode_block_msg(p.out(), msg::REJECT, piece, start, length);
                    self.to_flush.push(slot);
                }
            }
            Owned::Reject(piece, start) => {
                let (p, t) = self.peer_and_torrent(slot).expect("exists");
                let block = start / BLOCK_SIZE;
                if let Some(pos) = p.requests.iter().position(|r| r.piece == piece && r.block == block) {
                    p.requests.remove(pos);
                    if let Some(pk) = t.and_then(|t| t.picker.as_mut()) {
                        pk.abort_download(piece, block);
                    }
                }
                if !p.peer_choking {
                    self.request_blocks(slot);
                }
            }
            Owned::AllowedFast(i) => {
                let p = self.peers.get_mut(slot).expect("exists");
                if p.allowed_fast.len() < 64 && !p.allowed_fast.contains(&i) {
                    p.allowed_fast.push(i);
                }
            }
            Owned::Extended(id, payload) => {
                if id == 0 {
                    let p = self.peers.get_mut(slot).expect("exists");
                    match peer_wire::decode_ext_handshake(&payload) {
                        Some(h) => {
                            // libtorrent `on_extended_handshake`: `yourip` is the peer's vote
                            let vote = h.your_ip.map(|ip| (ip, p.addr.ip()));
                            p.ext = Some(h);
                            if let Some((ip, from)) = vote {
                                self.set_external_address(ip, crate::net::ip_voter::source::PEER, from);
                            }
                            self.on_metadata_handshake(slot);
                            self.on_pex_handshake(slot);
                        }
                        None => p.close("invalid extension handshake", true),
                    }
                } else if id == ut_metadata::OUR_ID {
                    self.on_metadata_message(slot, &payload);
                } else if id == ut_pex::OUR_ID {
                    self.on_pex_message(slot, &payload);
                } else if id == lt_donthave::OUR_ID {
                    if let Some(piece) = peer_wire::decode_dont_have(&payload) {
                        self.on_dont_have(slot, piece);
                    }
                }
            }
        }
    }

    /// libtorrent `peer_connection::incoming_dont_have`: the peer takes back a piece it
    /// announced.
    fn on_dont_have(&mut self, slot: usize, piece: u32) {
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let Some(t) = t else { return };
        // an invalid piece index, or no metadata yet to size the map: ignored
        if !t.has_metadata() || piece >= t.num_pieces || piece as usize >= p.peer_has.len() {
            return;
        }
        if !p.peer_has.get(piece as usize) {
            return; // redundant DONT_HAVE
        }
        let was_seed = p.peer_seed;
        p.peer_has.set(piece as usize, false);
        p.peer_seed = false;
        if let Some(pk) = t.picker.as_mut() {
            if was_seed {
                // counted as a seed until now: count its remaining pieces one by one instead
                pk.dec_seed();
                pk.inc_refcount_bitfield(&p.peer_has);
            } else {
                pk.dec_refcount(piece);
            }
        }
        if was_seed {
            t.peer_list.set_seed(&p.addr, false);
        }
        self.update_interest(slot);
    }

    /// libtorrent `bt_peer_connection::write_dont_have`: tells a peer that supports
    /// `lt_donthave` that a piece we announced is gone.
    pub(super) fn write_dont_have(&mut self, slot: usize, piece: u32) {
        let Some(p) = self.peers.get_mut(slot) else { return };
        if p.state != PeerState::Active || !p.supports_extensions {
            return;
        }
        let Some(id) = p.ext.as_ref().and_then(|e| e.message_id(lt_donthave::NAME)) else { return };
        peer_wire::encode_dont_have(p.out(), id, piece);
        self.to_flush.push(slot);
    }

    /// A new BITFIELD / HAVE_ALL / HAVE_NONE replaces what the peer announced so far
    /// (libtorrent `torrent::peer_lost` in `incoming_bitfield` and friends). A super seed
    /// that stops sends one after its HAVE_NONE and offered HAVEs.
    fn forget_peer_pieces(&mut self, slot: usize) {
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let Some(t) = t else { return };
        if let Some(pk) = t.picker.as_mut() {
            if p.peer_seed {
                pk.dec_seed();
            } else if p.peer_has.len() == t.num_pieces as usize {
                pk.dec_refcount_bitfield(&p.peer_has);
            }
        }
        if p.peer_seed {
            t.peer_list.set_seed(&p.addr, false);
        }
        p.peer_seed = false;
        p.peer_has = Bitfield::new(t.num_pieces as usize, false);
    }

    /// libtorrent `bt_peer_connection::write_bitfield`: our piece set; while super seeding a
    /// HAVE_NONE (to peers with the fast extension) and two offered pieces instead.
    pub(super) fn write_bitfield(&mut self, slot: usize) {
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        if t.super_seeding() {
            if p.supports_fast {
                peer_wire::encode_simple(p.out(), msg::HAVE_NONE);
            }
            p.superseed = Some(p.superseed.unwrap_or_default());
            self.to_flush.push(slot);
            // bootstrap super seeding with two HAVEs
            for _ in 0..2 {
                if let Some(piece) = self.superseed_pick(slot) {
                    self.superseed_piece(slot, None, Some(piece));
                }
            }
            return;
        }
        p.superseed = None;
        let have = t.picker.as_ref().map(|pk| pk.have().clone()).unwrap_or_default();
        let count = have.count();
        if p.supports_fast && count == have.len() && count > 0 {
            peer_wire::encode_simple(p.out(), msg::HAVE_ALL);
        } else if p.supports_fast && count == 0 {
            peer_wire::encode_simple(p.out(), msg::HAVE_NONE);
        } else if count > 0 {
            peer_wire::encode_bitfield(p.out(), have.as_bytes());
        }
        self.to_flush.push(slot);
    }

    /// The peer told us its full piece set.
    fn set_peer_pieces(&mut self, slot: usize, bf: Bitfield, have_all: bool) {
        let close_redundant = self.settings.close_redundant_connections;
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let Some(t) = t else { return };
        let seed = have_all || (!bf.is_empty() && bf.all_set());
        if let Some(pk) = t.picker.as_mut() {
            if seed {
                pk.inc_seed();
            } else {
                pk.inc_refcount_bitfield(&bf);
            }
        }
        p.peer_seed = seed;
        p.peer_has = bf;
        t.peer_list.set_seed(&p.addr, seed);
        if seed && t.is_seed() && close_redundant {
            p.close("both sides are seeds", false);
            return;
        }
        self.update_interest(slot);
    }

    /// libtorrent `peer_connection::update_interest`.
    pub(super) fn update_interest(&mut self, slot: usize) {
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let Some(t) = t else { return };
        // Upload mode never downloads (libtorrent `peer_connection::update_interest`).
        let interested = match t.picker.as_ref() {
            Some(_) if t.flags.contains(TorrentFlags::UPLOAD_MODE) => false,
            Some(pk) if !t.checking => p.got_bitfield && pk.is_interesting(&p.peer_has, p.peer_seed),
            _ => false,
        };
        if interested == p.am_interested {
            return;
        }
        p.am_interested = interested;
        peer_wire::encode_simple(p.out(), if interested { msg::INTERESTED } else { msg::NOT_INTERESTED });
        self.to_flush.push(slot);
        if interested {
            self.request_blocks(slot);
        }
    }

    /// libtorrent `request_a_block` + `send_block_requests`: tops up the peer's request queue.
    pub(super) fn request_blocks(&mut self, slot: usize) {
        let queue_time = self.settings.request_queue_time.max(1);
        let max_queue = self.settings.max_out_request_queue.max(1) as usize;
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let Some(t) = t else { return };
        if p.state != PeerState::Active
            || p.peer_choking
            || !p.am_interested
            || t.checking
            || t.flags.contains(TorrentFlags::UPLOAD_MODE)
            || p.closing.is_some()
        {
            return;
        }
        if t.picker.is_none() {
            return;
        }
        let desired = p.desired_queue(queue_time, max_queue, BLOCK_SIZE);
        if p.requests.len() >= desired {
            return;
        }
        let want = desired - p.requests.len();
        let on_parole = t.peer_list.get(&p.addr).is_some_and(|k| k.trust.on_parole);
        let Some(picker) = t.picker.as_mut() else { return };
        let requests = &p.requests;
        let picks = picker.pick(&p.peer_has, p.peer_seed, want, p.addr, on_parole, |piece, block| {
            requests.iter().any(|r| r.piece == piece && r.block == block)
        });
        if picks.is_empty() {
            return;
        }
        for (piece, block) in picks {
            let start = block * BLOCK_SIZE;
            let len = (t.piece_size(piece) as u32 - start).min(BLOCK_SIZE);
            peer_wire::encode_block_msg(p.out(), msg::REQUEST, piece, start, len);
            p.requests.push(Request { piece, block });
        }
        self.to_flush.push(slot);
    }

    /// No data from a peer with requests outstanding: hand its blocks to other peers
    /// (libtorrent `peer_connection::snub_peer` / request timeouts).
    pub(super) fn snub(&mut self, slot: usize) {
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        p.snubbed = true;
        let reqs: Vec<Request> = std::mem::take(&mut p.requests);
        for r in &reqs {
            let start = r.block * BLOCK_SIZE;
            let len =
                t.as_ref().map_or(BLOCK_SIZE, |t| (t.piece_size(r.piece) as u32 - start).min(BLOCK_SIZE));
            peer_wire::encode_block_msg(p.out(), msg::CANCEL, r.piece, start, len);
        }
        if let Some(pk) = t.and_then(|t| t.picker.as_mut()) {
            for r in &reqs {
                pk.abort_download(r.piece, r.block);
            }
        }
        // Give the peer one block at a time from now on.
        p.last_piece = Instant::now();
        self.to_flush.push(slot);
    }

    fn on_request(&mut self, slot: usize, piece: u32, start: u32, length: u32) {
        let max_in = self.settings.max_allowed_in_request_queue.max(1) as usize;
        let watermark = self.settings.send_buffer_watermark;
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let Some(t) = t else { return };
        // Super seeding: only the offered pieces (libtorrent `incoming_request`).
        if p.superseed.is_some_and(|o| !o.contains(&Some(piece))) {
            if p.supports_fast && super::room_for_reject(p, watermark) {
                peer_wire::encode_block_msg(p.out(), msg::REJECT, piece, start, length);
                self.to_flush.push(slot);
            }
            return;
        }
        let valid = piece < t.num_pieces
            && length > 0
            && length <= MAX_REQUEST
            && start as u64 + length as u64 <= t.piece_size(piece)
            && t.picker.as_ref().is_some_and(|pk| pk.have_piece(piece));
        let allowed = !p.am_choking || p.fast_set.contains(&piece);
        if !valid || !allowed || p.upload_queue.len() + p.reads_in_flight.len() >= max_in {
            if p.supports_fast {
                // past the watermark the peer is not reading: a reject would only pile up
                if super::room_for_reject(p, watermark) {
                    peer_wire::encode_block_msg(p.out(), msg::REJECT, piece, start, length);
                    self.to_flush.push(slot);
                }
            } else if !valid && allowed {
                p.close("invalid request", true);
            }
            return;
        }
        p.upload_queue.push_back((piece, start, length));
        self.pump_uploads(slot);
    }

    /// Issues disk reads for queued requests while the send buffer is below the watermark.
    pub(super) fn pump_uploads(&mut self, slot: usize) {
        let watermark = self.settings.send_buffer_watermark.max(BLOCK_SIZE) as usize;
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        let Some(storage) = t.storage.clone() else { return };
        let mut reads = Vec::new();
        while p.send_buffer_len() + p.reads_in_flight.len() * BLOCK_SIZE as usize <= watermark
            && p.reads_in_flight.len() < super::MAX_READS_IN_FLIGHT
        {
            let Some(r) = p.upload_queue.pop_front() else { break };
            p.reads_in_flight.insert(r);
            reads.push(r);
        }
        let id = p.id;
        for r in reads {
            self.disk.async_read(&storage, r.0, r.1, r.2, id);
        }
    }

    pub(super) fn on_read(
        &mut self,
        storage: u64,
        cookie: u64,
        piece: u32,
        offset: u32,
        result: Result<Vec<u8>, Error>,
    ) {
        if cookie & super::smart_ban::SMART_BAN_COOKIE != 0 {
            return self.on_smart_ban_read(storage, cookie, piece, offset, result);
        }
        let Some(slot) = self.peer_by_id(cookie) else { return };
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let Some(t) = t else { return };
        if t.storage.as_ref().is_none_or(|s| s.id != storage) {
            return;
        }
        let data = match result {
            Ok(d) => d,
            Err(e) => {
                // libtorrent `on_disk_read_complete`: the peer learns we cannot serve the
                // piece after all, and the request is rejected.
                let rejects: Vec<(u32, u32, u32)> = p
                    .reads_in_flight
                    .iter()
                    .copied()
                    .filter(|&(pc, st, _)| (pc, st) == (piece, offset))
                    .collect();
                for r in &rejects {
                    p.reads_in_flight.remove(r);
                    if p.supports_fast {
                        peer_wire::encode_block_msg(p.out(), msg::REJECT, r.0, r.1, r.2);
                    }
                }
                let id = t.id;
                self.write_dont_have(slot, piece);
                self.to_flush.push(slot);
                self.alert(Alert::Log {
                    id: Some(id),
                    message: format!("read failed for piece {piece}: {e}"),
                });
                return;
            }
        };
        let key = (piece, offset, data.len() as u32);
        if !p.reads_in_flight.remove(&key) || p.closing.is_some() {
            return; // cancelled, or we choked the peer meanwhile
        }
        peer_wire::encode_piece(p.out(), piece, offset, &data);
        p.mark_payload(data.len() as u32);
        self.to_flush.push(slot);
    }

    fn on_piece(&mut self, slot: usize, piece: u32, start: u32, data: Vec<u8>) {
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let Some(t) = t else { return };
        let len = data.len() as u64;
        p.stat.received(len, 0);
        t.stat.get_or_insert_with(Box::default).received(len, 0);
        let block = start / BLOCK_SIZE;
        let pos = (start % BLOCK_SIZE == 0)
            .then(|| p.requests.iter().position(|r| r.piece == piece && r.block == block))
            .flatten();
        let expected = (piece < t.num_pieces)
            .then(|| (t.piece_size(piece) as u32).saturating_sub(start).min(BLOCK_SIZE));
        let Some(pos) = pos.filter(|_| expected == Some(data.len() as u32)) else {
            // Unrequested, cancelled earlier, or malformed: drop it.
            t.redundant_bytes += len;
            self.stat.received(len, 0);
            return;
        };
        p.requests.remove(pos);
        let now = Instant::now();
        p.last_piece = now;
        p.snubbed = false;
        self.stat.received(len, 0);
        let (p, t) = self.peer_and_torrent(slot).expect("exists");
        let t = t.expect("checked");
        let addr = p.addr;
        let Some(picker) = t.picker.as_mut() else { return };
        let mut write = None;
        let mut others = Vec::new();
        if !picker.mark_as_writing(piece, block, addr) {
            t.redundant_bytes += len;
        } else if let Some(storage) = t.storage.clone() {
            t.downloaded_in_session = true;
            t.status_dirty = true;
            others = t.peers.iter().copied().filter(|&s| s != slot).collect();
            write = Some(storage);
        }
        if let Some(storage) = write {
            self.disk.async_write(&storage, piece, start, data);
        }
        // End-game: cancel the same block at the other peers we asked.
        for s in others {
            let Some(o) = self.peers.get_mut(s) else { continue };
            if let Some(i) = o.requests.iter().position(|r| r.piece == piece && r.block == block) {
                o.requests.remove(i);
                peer_wire::encode_block_msg(o.out(), msg::CANCEL, piece, start, len as u32);
                self.to_flush.push(s);
            }
        }
        self.request_blocks(slot);
    }
}
