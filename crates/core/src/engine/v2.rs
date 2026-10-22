// Ported from libtorrent-rasterbar 2.0.11: src/torrent.cpp (`handle_inconsistent_hashes`,
// `get_hashes`, `initialize_merkle_trees`), src/bt_peer_connection.cpp (`on_hash_request`,
// `on_hashes`, `on_hash_reject`)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: v2 pieces are verified on the disk thread against the piece layers (see
// `disk::PieceHasher`), so the torrent keeps no block hashes and its trees hold the piece layers
// only; hash requests for block hashes are rejected; see docs/PORTING.md.

//! BitTorrent v2 (BEP 52) handling in the engine.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::Engine;
use super::hash_picker::{HashPicker, HashRequest, validate_hash_request};
use super::peer::PeerState;
use super::tracker::{JobKind, TrackerJob, TrackerOutcome};
use crate::alert::Alert;
use crate::error::Error;
use crate::metainfo::merkle_tree::MerkleTree;
use crate::metainfo::{MetainfoError, TorrentInfo};
use crate::proto::peer_wire::{self, HashMsg, msg};
use crate::proto::tracker::Event;
use crate::status::PeerSource;
use crate::types::{Sha256Hash, TorrentId};
use crate::util::bitfield::Bitfield;

/// The piece layers of a v2 torrent being fetched from v2 peers (libtorrent's hash picker on a
/// torrent without piece layers): the files' trees, filled in as hashes arrive with their
/// proofs, and the picker deciding what to ask for.
pub(crate) struct LayerFetch {
    pub trees: Vec<MerkleTree>,
    pub picker: HashPicker,
}

impl LayerFetch {
    pub fn new(ti: &TorrentInfo) -> LayerFetch {
        let trees = build_trees(ti);
        let picker = HashPicker::new(ti.orig_files(), &trees);
        LayerFetch { trees, picker }
    }

    /// Every file of more than one piece has its whole piece layer.
    pub fn complete(&self, ti: &TorrentInfo) -> bool {
        let fs = ti.orig_files();
        (0..fs.num_files()).all(|f| {
            fs.pad_file_at(f) || fs.file_size(f) <= fs.piece_length() as u64 || {
                let layer = self.trees[f].get_piece_layer();
                layer.len() == fs.file_num_pieces(f) as usize && !layer.iter().any(Sha256Hash::is_zero)
            }
        })
    }

    /// The layers as a bencoded "piece layers" dictionary, as in a `.torrent`.
    pub fn bencoded(&self, ti: &TorrentInfo) -> Vec<u8> {
        let fs = ti.orig_files();
        let mut layers = crate::bencode::Entry::new_dict();
        for f in 0..fs.num_files() {
            if fs.pad_file_at(f) || fs.file_size(f) <= fs.piece_length() as u64 {
                continue;
            }
            let bytes: Vec<u8> = self.trees[f].get_piece_layer().iter().flat_map(|h| h.0).collect();
            *layers.key(fs.root_bytes(f)) = crate::bencode::Entry::Bytes(bytes);
        }
        layers.bencode()
    }
}

/// One merkle tree per file holding what the metadata knows: the root and the piece layer
/// (libtorrent `initialize_merkle_trees`). Pad and empty files get an empty tree.
pub(super) fn build_trees(ti: &TorrentInfo) -> Vec<MerkleTree> {
    let fs = ti.orig_files();
    let blocks_per_piece = fs.blocks_per_piece() as usize;
    (0..fs.num_files())
        .map(|f| {
            if fs.pad_file_at(f) || fs.file_size(f) == 0 {
                return MerkleTree::default();
            }
            let mut tree = MerkleTree::new(fs.file_num_blocks(f) as usize, blocks_per_piece, fs.root(f));
            let layer = ti.piece_layer(f);
            if !layer.is_empty() {
                // the root alone is still known when the layer does not hash to it
                let _ = tree.load_piece_layer(layer);
            }
            tree
        })
        .collect()
}

/// The hashes of a hash request for `ti` (libtorrent `torrent::get_hashes`), empty when the
/// request is invalid or asks for hashes the trees do not hold.
pub(super) fn hashes_for(trees: &[MerkleTree], ti: &TorrentInfo, m: &HashMsg) -> Vec<Sha256Hash> {
    let fs = ti.orig_files();
    let Some(file) = fs.file_index_for_root(&m.root) else { return Vec::new() };
    let req = HashRequest::new(file, m.base, m.index, m.count, m.proof_layers);
    if !validate_hash_request(&req, fs) || !trees.get(file).is_some_and(MerkleTree::is_initialized) {
        return Vec::new();
    }
    trees[file].get_hashes(m.base as usize, m.index as usize, m.count as usize, m.proof_layers as usize)
}

/// Bytes queued for a peer at which its hash requests are rejected instead of answered.
/// A 49-byte request can ask for 8192 hashes (256 KiB), and libtorrent answers every one, so a
/// peer that sends requests and never reads the answers would grow our send buffer without
/// bound. ut_metadata requests have the same kind of guard (`ut_metadata::SEND_BUFFER_LIMIT`).
/// New in the security review of 2026-10-22; libtorrent has no such limit.
pub(super) const HASH_REPLY_SEND_BUFFER_LIMIT: usize = 1024 * 1024;

/// Appends the answer to a hash request to `out`: the hashes, or a reject when there are none
/// or when `queued` (the bytes already waiting for the peer) has reached
/// [`HASH_REPLY_SEND_BUFFER_LIMIT`]. A reject is no larger than the request.
fn write_hash_answer(out: &mut Vec<u8>, queued: usize, m: &HashMsg, hashes: &[Sha256Hash]) {
    if hashes.is_empty() || queued >= HASH_REPLY_SEND_BUFFER_LIMIT {
        peer_wire::encode_hash_msg(out, msg::HASH_REJECT, m);
    } else {
        peer_wire::encode_hashes(out, m, hashes);
    }
}

impl Engine {
    /// libtorrent `on_hash_request`: a v2 peer asks for part of a file's merkle tree; it gets
    /// the hashes or a reject. Hash messages on a v1 connection end it.
    pub(super) fn on_hash_request(&mut self, slot: usize, m: HashMsg) {
        let Some((p, t)) = self.peer_and_torrent(slot) else { return };
        if !p.protocol_v2 {
            p.close("hash request on a v1 connection", true);
            return;
        }
        let queued = p.send_buffer_len();
        let hashes = match t {
            // not even looked up while the peer leaves its earlier answers unread
            _ if queued >= HASH_REPLY_SEND_BUFFER_LIMIT => Vec::new(),
            Some(t) => match &t.ti {
                Some(ti) => {
                    let trees = t.merkle_trees.get_or_insert_with(|| build_trees(ti));
                    hashes_for(trees, ti, &m)
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        write_hash_answer(p.out(), queued, &m, &hashes);
        self.to_flush.push(slot);
    }

    /// libtorrent `on_hashes`: the reply to one of our requests (or unsolicited hashes, which
    /// libtorrent takes too) goes into the trees while the piece layers are being fetched, and
    /// is dropped otherwise. Hashes that do not prove out, or a malformed reply, end the
    /// connection.
    pub(super) fn on_hashes(&mut self, slot: usize, m: HashMsg, hashes: &[u8]) {
        let Some((p, t)) = self.peer_and_torrent(slot) else { return };
        if !p.protocol_v2 {
            p.close("hashes on a v1 connection", true);
            return;
        }
        let Some(t) = t else { return };
        let Some(ti) = t.ti.as_ref() else { return };
        let fs = ti.orig_files();
        let file = fs.file_index_for_root(&m.root).unwrap_or(usize::MAX);
        let req = HashRequest::new(file, m.base, m.index, m.count, m.proof_layers);
        if !validate_hash_request(&req, fs)
            || hashes.len() != (m.count as usize + req.num_proof_hashes()) * 32
        {
            p.close("invalid hashes", true);
            return;
        }
        p.hash_requests.retain(|r| *r != req);
        let Some(lf) = t.layer_fetch.as_mut() else { return };
        let hashes: Vec<Sha256Hash> =
            hashes.chunks_exact(32).map(|h| Sha256Hash(h.try_into().expect("32 bytes"))).collect();
        if !lf.picker.add_hashes(fs, &mut lf.trees, &req, &hashes).valid {
            p.close("hashes that do not prove out", true);
            return;
        }
        let id = t.id;
        if lf.complete(ti) {
            self.finish_layer_fetch(id);
        } else {
            self.maybe_send_hash_request(slot);
        }
    }

    /// libtorrent `on_hash_reject` and `torrent::hashes_rejected`: the request is asked of
    /// someone else, so every v2 peer gets a chance to send one.
    pub(super) fn on_hash_reject(&mut self, slot: usize, m: HashMsg) {
        let Some((p, t)) = self.peer_and_torrent(slot) else { return };
        if !p.protocol_v2 {
            p.close("hash reject on a v1 connection", true);
            return;
        }
        let Some(t) = t else { return };
        let Some(ti) = t.ti.as_ref() else { return };
        let file = ti.orig_files().file_index_for_root(&m.root).unwrap_or(usize::MAX);
        let req = HashRequest::new(file, m.base, m.index, m.count, m.proof_layers);
        let before = p.hash_requests.len();
        p.hash_requests.retain(|r| *r != req);
        if p.hash_requests.len() == before {
            return;
        }
        let Some(lf) = t.layer_fetch.as_mut() else { return };
        lf.picker.hashes_rejected(&req);
        for peer in t.peers.clone() {
            self.maybe_send_hash_request(peer);
        }
    }

    /// libtorrent `maybe_send_hash_request`: a v2 peer with fewer than two requests out gets the
    /// next one the hash picker chooses for the pieces it has.
    pub(super) fn maybe_send_hash_request(&mut self, slot: usize) {
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        if p.state != PeerState::Active || p.closing.is_some() || !p.protocol_v2 || p.hash_requests.len() > 1
        {
            return;
        }
        let (Some(ti), Some(lf)) = (t.ti.as_ref(), t.layer_fetch.as_mut()) else { return };
        let fs = ti.orig_files();
        let pieces =
            if p.peer_seed { Bitfield::new(fs.num_pieces() as usize, true) } else { p.peer_has.clone() };
        if pieces.len() != fs.num_pieces() as usize {
            return;
        }
        let Some(req) = lf.picker.pick_hashes(fs, &lf.trees, &pieces, Instant::now()) else { return };
        let m = HashMsg {
            root: fs.root(req.file),
            base: req.base,
            index: req.index,
            count: req.count,
            proof_layers: req.proof_layers,
        };
        peer_wire::encode_hash_msg(p.out(), msg::HASH_REQUEST, &m);
        p.hash_requests.push(req);
        self.to_flush.push(slot);
    }

    /// A v2 torrent whose metadata lacks piece layers (a v2 magnet, or a `.torrent` without
    /// them) asks its v2 peers for them before it checks its files, reported as downloading
    /// metadata meanwhile.
    pub(super) fn start_layer_fetch(&mut self, id: TorrentId) {
        let Some(t) = self.torrent(id) else { return };
        let Some(ti) = t.ti.clone() else { return };
        t.layer_fetch = Some(LayerFetch::new(&ti));
        t.started_at = Some(Instant::now());
        t.status_dirty = true;
        let peers = t.peers.clone();
        self.mark_live(id);
        self.alert(Alert::Log { id: Some(id), message: "fetching the v2 piece layers from peers".into() });
        self.set_state(id, crate::status::TorrentState::DownloadingMetadata);
        for slot in peers {
            self.maybe_send_hash_request(slot);
        }
        self.connect_peers_for(id, 10);
        self.prioritize_dht(id);
        self.lsd_announce(id, Instant::now());
    }

    /// Every piece layer arrived and proved out against the file roots: the metadata gets
    /// them, the store keeps them, and the torrent starts as any v2 torrent does.
    fn finish_layer_fetch(&mut self, id: TorrentId) {
        let Some(t) = self.torrent(id) else { return };
        let (Some(ti), Some(lf)) = (t.ti.clone(), t.layer_fetch.take()) else { return };
        let layers = lf.bencoded(&ti);
        let mut full = (*ti).clone();
        if let Err(e) = full.set_piece_layers(&layers) {
            // the trees validated every hash, so this would be a bug
            self.alert(Alert::Log { id: Some(id), message: format!("piece layers: {e}") });
            return;
        }
        t.set_metadata(Arc::new(full));
        t.resume_dirty = true;
        for slot in t.peers.clone() {
            if let Some(p) = self.peers.get_mut(slot) {
                p.hash_requests.clear();
            }
        }
        let key = self.torrent_ref(id).map(|t| t.key.clone()).unwrap_or_default();
        if let Some(store) = self.store.as_mut() {
            if let Err(e) = store.put_piece_layers(&key, &layers) {
                self.out
                    .push(Alert::Log { id: Some(id), message: format!("cannot save the piece layers: {e}") });
            }
        }
        self.alert(Alert::Log { id: Some(id), message: "v2 piece layers complete".into() });
        self.start_torrent(id);
    }

    /// The reply to a hybrid's announce under its v2 hash: the v2 tracker state follows it as
    /// the visible one follows the v1 announce, and the peers it names are reached with the v2
    /// hash. No alerts: frontends show the v1 announce.
    pub(super) fn apply_v2_announce(&mut self, job: TrackerJob, outcome: TrackerOutcome, now: Instant) {
        let min_interval = self.settings.min_announce_interval;
        let id = job.torrent;
        let Some(t) = self.torrent(id) else { return };
        let Some(tr) = t.trackers_v2.iter_mut().find(|tr| tr.url == job.url) else { return };
        match outcome {
            TrackerOutcome::Announce(r, _) => {
                tr.updating = false;
                tr.fails = 0;
                tr.working = true;
                if let JobKind::Announce(req) = &job.kind {
                    match req.event {
                        Event::Started => tr.start_sent = true,
                        Event::Completed => tr.complete_sent = true,
                        _ => {}
                    }
                }
                if let Some(tid) = &r.tracker_id {
                    tr.tracker_id = tid.clone();
                }
                tr.interval = Some(r.interval);
                let interval = r.interval.max(min_interval.min(r.interval.max(60)));
                tr.next_announce = Some(now + Duration::from_secs(interval as u64));
                tr.min_announce = r.min_interval.map(|m| now + Duration::from_secs(m as u64));
                self.add_peers(id, &r.peers, PeerSource::TRACKER);
                if let Some(t) = self.torrent(id) {
                    for addr in &r.peers {
                        t.peer_list.set_protocol_v2(addr);
                    }
                }
                self.connect_peers_for(id, 10);
            }
            TrackerOutcome::Scrape(_) => {}
            TrackerOutcome::Failed(message) => tr.failed(message, now),
        }
    }

    /// libtorrent `handle_inconsistent_hashes`: a hybrid piece passed only one of its v1 and v2
    /// hashes. The torrent stops with the error naming the piece's file and forgets every piece,
    /// so the next start checks the files again.
    pub(super) fn inconsistent_hashes(&mut self, id: TorrentId, piece: u32) {
        let Some(t) = self.torrent(id) else { return };
        let error = Error::Metainfo(MetainfoError::TorrentInconsistentHashes);
        let file = t.ti.as_ref().and_then(|ti| {
            ti.map_block(piece, 0, 1).first().map(|s| ti.files().file_path(s.file_index, &t.save_path))
        });
        if let Some(p) = t.picker.as_mut() {
            p.set_have(&Bitfield::new(t.num_pieces as usize, false));
        }
        t.have = None;
        t.unfinished.clear();
        t.checked = false;
        t.error = Some(match &file {
            Some(f) => format!("{f}: {error}"),
            None => error.to_string(),
        });
        t.status_dirty = true;
        t.resume_dirty = true;
        self.alert(Alert::Log { id: Some(id), message: format!("piece {piece}: {error}") });
        if let Some(file) = file {
            self.alert(Alert::FileError { id, file, error: error.clone() });
        }
        self.alert(Alert::TorrentError { id, error });
        self.update_running(id);
        self.needs_auto_manage = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security review 2026-10-22: hash requests are answered while the peer's send buffer is
    /// below the limit, and rejected (an answer no larger than the request) from then on, so a
    /// peer that never reads cannot make one 49-byte request queue 256 KiB of hashes after
    /// another.
    #[test]
    fn hash_requests_are_rejected_while_the_send_buffer_is_full() {
        let m = HashMsg { root: Sha256Hash([7; 32]), base: 1, index: 0, count: 8192, proof_layers: 0 };
        let hashes = vec![Sha256Hash([1; 32]); 8192];
        let mut out = Vec::new();
        write_hash_answer(&mut out, 0, &m, &hashes);
        assert_eq!(out[4], msg::HASHES);
        assert_eq!(out.len(), 4 + 1 + 48 + 8192 * 32);

        for queued in [HASH_REPLY_SEND_BUFFER_LIMIT, HASH_REPLY_SEND_BUFFER_LIMIT * 4] {
            let mut out = Vec::new();
            write_hash_answer(&mut out, queued, &m, &hashes);
            assert_eq!(out[4], msg::HASH_REJECT);
            assert_eq!(out.len(), 4 + 1 + 48, "the size of the request");
        }
        let mut out = Vec::new();
        write_hash_answer(&mut out, 0, &m, &[]);
        assert_eq!(out[4], msg::HASH_REJECT);
    }
}
