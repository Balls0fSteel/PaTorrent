// Ported from libtorrent-rasterbar 2.0.11: src/ut_metadata.cpp (peer plugin: handshake,
// on_extended, tick, maybe_send_request), src/torrent.cpp (set_metadata),
// src/peer_connection.cpp (on_metadata_impl)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: engine methods taking a peer slot instead of plugin objects. Once the metadata is in,
// the torrent goes through the usual start (storage, picker, file check) with its peers kept.

//! BEP 9: fetching a magnet's info dictionary from peers, and serving ours.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::peer::PeerConn;
use super::peer::PeerState;
use super::torrent::Torrent;
use super::{Engine, STORE_FLUSH_DELAY, Slab, TorrentSlot};
use crate::alert::Alert;
use crate::error::Error;
use crate::metainfo::{LoadLimits, TorrentInfo};
use crate::params::TorrentFlags;
use crate::proto::ut_metadata::{self, Message, Received, msg};
use crate::types::TorrentId;

/// After metadata from a single peer (one block) fails the hash check, that peer waits this
/// much longer than usual before we ask it again.
const SINGLE_PEER_PENALTY: Duration = Duration::from_secs(5 * 60);

/// The torrent with `id`, borrowing only the slot table (so peers can be borrowed alongside).
fn slot_torrent(slots: &mut [TorrentSlot], id: TorrentId) -> Option<&mut Torrent> {
    let s = slots.get_mut(id.index() as usize)?;
    if s.generation != id.generation() {
        return None;
    }
    s.torrent.as_mut()
}

/// The peer with connection id `id`, if it is still connected and not closing.
fn peer_alive(peers: &Slab<PeerConn>, id: u64) -> bool {
    peers.get((id & 0xffff_ffff) as usize).is_some_and(|p| p.id == id && p.closing.is_none())
}

/// ut_metadata is off for private torrents.
fn private(t: &Torrent) -> bool {
    t.ti.as_ref().is_some_and(|ti| ti.is_private())
}

impl Engine {
    /// The peer's extension handshake arrived (libtorrent `on_extension_handshake`).
    pub(super) fn on_metadata_handshake(&mut self, slot: usize) {
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        let Some(ext) = p.ext.as_ref() else { return };
        p.meta.message_id = 0;
        if private(t) {
            return;
        }
        let Some(id) = ext.message_id(ut_metadata::NAME) else { return };
        p.meta.message_id = id;
        match ext.metadata_size {
            Some(n) => {
                if let Some(f) = t.fetch.as_mut() {
                    f.size_hint(n.min(i64::MAX as u64) as i64);
                }
            }
            None => p.meta.has_metadata = false,
        }
        self.maybe_request_metadata(slot, Instant::now());
    }

    /// An extended message with our ut_metadata id (libtorrent `on_extended`).
    pub(super) fn on_metadata_message(&mut self, slot: usize, payload: &[u8]) {
        let now = Instant::now();
        let max_size = self.settings.max_metadata_size.max(0) as usize;
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        if p.meta.message_id == 0 || private(t) {
            return;
        }
        let m = match ut_metadata::decode(payload) {
            Ok(m) => m,
            Err(e) => {
                p.close(e.to_string(), true);
                return;
            }
        };
        let id = t.id;
        match m {
            Message::Request { piece } => {
                let size = t.ti.as_ref().filter(|_| t.has_metadata()).map(|ti| ti.info_section().len());
                let valid = size.and_then(|n| ut_metadata::block_range(n, piece)).is_some();
                let ext_id = p.meta.message_id;
                if !valid {
                    ut_metadata::encode(p.out(), ext_id, msg::REJECT, piece, size, &[]);
                } else if p.send_buffer_len() < ut_metadata::SEND_BUFFER_LIMIT {
                    send_block(p, t, piece as u32);
                } else if p.meta.incoming_requests.len() < ut_metadata::MAX_INCOMING_REQUESTS {
                    p.meta.incoming_requests.push_back(piece as u32);
                } else {
                    ut_metadata::encode(p.out(), ext_id, msg::REJECT, piece, size, &[]);
                }
                self.to_flush.push(slot);
            }
            Message::Data { piece, total_size, data } => {
                if !p.meta.take_sent(piece) {
                    return; // unwanted, or timed out
                }
                let Some(fetch) = t.fetch.as_mut() else {
                    t.redundant_bytes += data.len() as u64;
                    return;
                };
                match fetch.received(p.id, piece, total_size, data, max_size, &t.info_hashes) {
                    Received::Rejected | Received::Stored => {}
                    Received::Complete(info) => {
                        self.got_metadata(id, info);
                        return;
                    }
                    Received::HashFailed { sources, single_peer } => {
                        let from = if single_peer { now + SINGLE_PEER_PENALTY } else { now };
                        for s in sources {
                            if let Some(src) = self.peer_by_id(s).and_then(|s| self.peers.get_mut(s)) {
                                src.meta.failed_hash_check(from);
                            }
                        }
                        self.alert(Alert::MetadataFailed { id, error: Error::MismatchingInfoHash });
                    }
                }
                if let Some(t) = self.torrent(id) {
                    t.status_dirty = true;
                }
                self.maybe_request_metadata(slot, now);
            }
            Message::Reject { piece } => p.meta.rejected(piece, now),
            Message::Unknown { .. } => {}
        }
    }

    /// Asks the peer for a metadata block if the torrent needs one (libtorrent
    /// `maybe_send_request`).
    pub(super) fn maybe_request_metadata(&mut self, slot: usize, now: Instant) {
        let Some(p) = self.peers.get(slot) else { return };
        if p.closing.is_some() || p.state != PeerState::Active || !p.meta.may_request(now) {
            return;
        }
        let (Some(tid), pid, has_metadata) = (p.torrent, p.id, p.meta.has_metadata) else { return };
        let peers = &self.peers;
        let Some(fetch) = slot_torrent(&mut self.slots, tid).and_then(|t| t.fetch.as_mut()) else { return };
        let Some(piece) = fetch.request(pid, has_metadata, now, |id| peer_alive(peers, id)) else { return };
        let p = self.peers.get_mut(slot).expect("checked");
        p.meta.sent_requests.push(piece as i64);
        let ext_id = p.meta.message_id;
        ut_metadata::encode(p.out(), ext_id, msg::REQUEST, piece as i64, None, &[]);
        self.to_flush.push(slot);
    }

    /// Per-tick work for one peer (libtorrent peer plugin `tick`): new requests, and requests
    /// that waited for room in the send buffer.
    pub(super) fn metadata_tick(&mut self, slot: usize, now: Instant) {
        self.maybe_request_metadata(slot, now);
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        if p.meta.incoming_requests.is_empty() || p.closing.is_some() {
            return;
        }
        while p.send_buffer_len() < ut_metadata::SEND_BUFFER_LIMIT {
            let Some(piece) = p.meta.incoming_requests.pop_front() else { break };
            send_block(p, t, piece);
        }
        self.to_flush.push(slot);
    }

    /// The info dictionary arrived and matches the info-hash (libtorrent `torrent::set_metadata`
    /// and `peer_connection::on_metadata_impl`).
    fn got_metadata(&mut self, id: TorrentId, info: Vec<u8>) {
        let parsed = TorrentInfo::from_info_section(&info, LoadLimits::default())
            .map_err(Error::from)
            .and_then(|ti| {
                if ti.v2() && !ti.v1() {
                    Err(Error::Unsupported("v2-only torrents (PLAN.md Phase 8)"))
                } else {
                    Ok(ti)
                }
            });
        let ti = match parsed {
            Ok(ti) => ti,
            Err(error) => {
                // The data matches the hash, so the swarm's metadata itself is bad: stop with
                // an error, as libtorrent does.
                if let Some(t) = self.torrent(id) {
                    t.fetch = None;
                    t.error = Some(format!("invalid metadata from peers: {error}"));
                    t.status_dirty = true;
                }
                self.alert(Alert::MetadataFailed { id, error: error.clone() });
                self.alert(Alert::TorrentError { id, error });
                self.update_running(id);
                self.needs_auto_manage = true;
                return;
            }
        };
        let Some(t) = self.torrent(id) else { return };
        t.fetch = None;
        if ti.info_hashes().v1 == t.info_hashes.v1 {
            // A hybrid torrent's v2 hash is known now.
            t.info_hashes = ti.info_hashes();
        }
        if !t.user_named {
            t.name = ti.name().to_owned();
        }
        let given = t.file_priority.len();
        t.set_metadata(Arc::new(ti));
        if t.flags.contains(TorrentFlags::DEFAULT_DONT_DOWNLOAD) {
            for p in t.file_priority.iter_mut().skip(given) {
                *p = 0;
            }
            t.flags.remove(TorrentFlags::DEFAULT_DONT_DOWNLOAD);
        }
        t.resume_dirty = true;
        t.status_dirty = true;
        let n = t.num_pieces;
        let slots = t.peers.clone();
        let key = t.key.clone();
        let rec = t.resume_data();
        for slot in slots {
            let Some(p) = self.peers.get_mut(slot) else { continue };
            if p.state != PeerState::Active {
                continue;
            }
            // Piece maps collected without a piece count get their real size.
            let seed = p.peer_seed;
            p.peer_has.resize(n as usize, seed);
            p.peer_seed = seed || p.peer_has.all_set();
            p.allowed_fast.retain(|&i| i < n);
            let (addr, seed) = (p.addr, p.peer_seed);
            if let Some(t) = self.torrent(id) {
                t.peer_list.set_seed(&addr, seed);
            }
        }
        if let Some(store) = self.store.as_mut() {
            if let Err(e) = store.put(&key, &rec, Some(&info)) {
                self.out.push(Alert::Log { id: Some(id), message: format!("cannot save the metadata: {e}") });
            }
            self.store_flush_due.get_or_insert(Instant::now() + STORE_FLUSH_DELAY);
        }
        self.alert(Alert::MetadataReceived { id });
        self.start_torrent(id);
    }
}

/// Queues metadata block `piece` for the peer.
fn send_block(p: &mut PeerConn, t: &Torrent, piece: u32) {
    let Some(ti) = t.ti.as_ref() else { return };
    let section = ti.info_section();
    let Some(range) = ut_metadata::block_range(section.len(), piece as i64) else { return };
    let ext_id = p.meta.message_id;
    ut_metadata::encode(p.out(), ext_id, msg::DATA, piece as i64, Some(section.len()), &section[range]);
}
