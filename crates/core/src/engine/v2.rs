// Ported from libtorrent-rasterbar 2.0.11: src/torrent.cpp (`handle_inconsistent_hashes`,
// `get_hashes`, `initialize_merkle_trees`), src/bt_peer_connection.cpp (`on_hash_request`,
// `on_hashes`, `on_hash_reject`); a fix from 2.1.2 (37d1add171)
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
use crate::metainfo::{FileStorage, MetainfoError, TorrentInfo};
use crate::proto::peer_wire::{self, HashMsg, msg};
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

/// Hash-tree work (merkle node pairs hashed, one SHA-256 each) a peer's budget gains per
/// second: a few milliseconds of one core.
pub(super) const HASH_WORK_PER_SECOND: u64 = 1 << 16;

/// The least a peer's budget holds when full, for torrents of few pieces.
const MIN_HASH_WORK_BURST: u64 = 1 << 16;

/// What answering a valid hash request costs in merkle hashing (pairs hashed, one SHA-256
/// each), as an upper bound. Our trees hold the piece layers only (libtorrent's piece-layer
/// mode), so a node `h` layers above the piece layer is computed from the `2^h` pieces under it
/// with `2^h - 1` hashes, for each of the `count` hashes asked and for each proof hash beyond
/// the subtree the requested hashes span (as `MerkleTree::get_hashes` walks them). Hashes of
/// the piece layer are copied, and requests below it are rejected without hashing.
pub(super) fn hash_request_cost(fs: &FileStorage, m: &HashMsg) -> u64 {
    let piece_base = fs.blocks_per_piece().max(1).trailing_zeros();
    let height = (m.base.max(0) as u32).saturating_sub(piece_base);
    let node = |h: u32| 1u64.checked_shl(h).map_or(u64::MAX, |n| n - 1);
    let count = m.count.max(1) as u64;
    let mut cost = count.saturating_mul(node(height));
    let base_tree_layers = count.next_power_of_two().trailing_zeros();
    // past 64 layers every node costs `u64::MAX` already: no need to walk two billion of them
    for i in base_tree_layers..(m.proof_layers.max(0) as u32).min(64) {
        cost = cost.saturating_add(node(height.saturating_add(i + 1)));
    }
    cost
}

/// A peer's budget of hash-tree work ([`hash_request_cost`]) for its hash requests: a token
/// bucket filled at [`HASH_WORK_PER_SECOND`] up to a burst of `P × log2(P)`, `P` the torrent's
/// piece count padded to a power of two. Beyond it requests get a `hash reject`, uncomputed.
/// New in the parser security review (2026-10-22); libtorrent computes every request (see
/// docs/PORTING.md).
///
/// Why this size: a client fetching a file's piece layer from us (libtorrent's hash picker:
/// 512 pieces per request, two outstanding) asks for proof hashes up to the first node of its
/// tree it knows, so the subtrees beside its path get hashed once per level: the whole layer
/// costs us about `P × (log2(P) - 9)` (measured by
/// `a_client_fetching_piece_layers_stays_within_the_hash_budget`), and over several files no
/// more than for one file of all the pieces. The burst covers that with no time passing, so a
/// well-behaved client never hits the limit, while a peer repeating requests near the root
/// (each about `P` hashes) is held to the refill rate after one burst. A client downloading a
/// torrent with piece layers asks only for block hashes, which our trees do not hold: those
/// are rejected without hashing. Cached interior layers would make every request cheap but
/// cost as much memory again as the piece layers; the budget costs 24 bytes a peer.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HashWorkBudget {
    /// Work left; `None` until the first request (the bucket starts full).
    tokens: Option<u64>,
    refilled: Option<Instant>,
}

impl HashWorkBudget {
    /// The burst for a torrent of `num_pieces` pieces.
    pub(super) fn burst(num_pieces: u32) -> u64 {
        let padded = u64::from(num_pieces.max(1)).next_power_of_two();
        (padded * u64::from(padded.trailing_zeros())).max(MIN_HASH_WORK_BURST)
    }

    /// Takes `cost` from the budget, refilled for the time since the last call and capped at
    /// `burst`; false (and nothing taken) when it does not hold that much.
    pub(super) fn try_spend(&mut self, cost: u64, burst: u64, now: Instant) -> bool {
        let elapsed = self.refilled.map_or(Duration::ZERO, |t| now.saturating_duration_since(t));
        let gained = (elapsed.as_millis() as u64).saturating_mul(HASH_WORK_PER_SECOND) / 1000;
        let tokens = self.tokens.map_or(burst, |t| t.saturating_add(gained).min(burst));
        // the refill counts whole milliseconds: a call that gains nothing leaves the clock
        // alone, so requests less than a millisecond apart still add up to a refill
        if self.refilled.is_none() || gained > 0 || tokens == burst {
            self.refilled = Some(now);
        }
        let ok = cost <= tokens;
        self.tokens = Some(if ok { tokens - cost } else { tokens });
        ok
    }
}

/// Removes `req` from a peer's outstanding hash requests; false if it was not among them, so
/// the hashes or reject answer nothing we asked that peer.
fn take_hash_request(outstanding: &mut Vec<HashRequest>, req: &HashRequest) -> bool {
    let before = outstanding.len();
    outstanding.retain(|r| r != req);
    outstanding.len() != before
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
                    let fs = ti.orig_files();
                    let valid = fs.file_index_for_root(&m.root).is_some_and(|file| {
                        validate_hash_request(
                            &HashRequest::new(file, m.base, m.index, m.count, m.proof_layers),
                            fs,
                        )
                    });
                    // an invalid request is rejected by `hashes_for` without hashing and is not
                    // charged; a valid one beyond the peer's budget is rejected uncomputed
                    let affordable = !valid || {
                        let burst = HashWorkBudget::burst(fs.num_pieces());
                        p.hash_budget.try_spend(hash_request_cost(fs, &m), burst, Instant::now())
                    };
                    if affordable { hashes_for(trees, ti, &m) } else { Vec::new() }
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        write_hash_answer(p.out(), queued, &m, &hashes);
        self.to_flush.push(slot);
    }

    /// libtorrent `on_hashes`: the reply to one of our requests goes into the trees while the
    /// piece layers are being fetched, and is dropped otherwise, as are hashes we did not ask
    /// this peer for (2.1.2 37d1add171). Hashes that do not prove out, or a malformed reply, end
    /// the connection.
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
        if !take_hash_request(&mut p.hash_requests, &req) {
            return;
        }
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
        if !take_hash_request(&mut p.hash_requests, &req) {
            return;
        }
        let Some(lf) = t.layer_fetch.as_mut() else { return };
        lf.picker.hashes_rejected(ti.orig_files(), &req);
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
        t.layer_fetch = Some(Box::new(LayerFetch::new(&ti)));
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
        self.prioritize_lsd(id);
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
        let key = self.torrent_ref(id).map(|t| t.key()).unwrap_or_default();
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
        let Some(tr) = t.trackers_v2.iter_mut().find(|tr| *tr.url == *job.url) else { return };
        match outcome {
            TrackerOutcome::Announce(r, _) => {
                let event = match &job.kind {
                    JobKind::Announce(req) => Some(req.event),
                    JobKind::Scrape(_) => None,
                };
                tr.announced(event, r.tracker_id.as_deref(), r.interval, r.min_interval, min_interval, now);
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
        t.set_have(None);
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

    /// libtorrent 2.1.2 (37d1add171): hashes are only taken as the answer to a request still
    /// outstanding with that peer; unsolicited ones, or a second answer, are dropped.
    #[test]
    fn only_requested_hashes_are_taken() {
        let req = HashRequest::new(0, 2, 0, 512, 9);
        let other = HashRequest::new(0, 2, 512, 512, 9);
        let mut outstanding = vec![req, other];
        assert!(take_hash_request(&mut outstanding, &req));
        assert_eq!(outstanding, vec![other]);
        assert!(!take_hash_request(&mut outstanding, &req), "answered already");
        assert!(!take_hash_request(&mut outstanding, &HashRequest::new(0, 2, 1024, 512, 9)));
        assert_eq!(outstanding, vec![other]);
    }

    const BLOCK: u32 = crate::metainfo::file_storage::DEFAULT_BLOCK_SIZE;

    /// One file of `pieces` pieces of four blocks, and our tree of it in piece-layer mode.
    fn one_file(pieces: u32) -> (FileStorage, MerkleTree) {
        use crate::metainfo::merkle::{merkle_pad, merkle_root};
        let mut fs = FileStorage::new();
        fs.set_piece_length(4 * BLOCK);
        let size = u64::from(pieces) * u64::from(4 * BLOCK);
        fs.add_file("t/f", size, crate::metainfo::FileFlags::empty(), 0, "", None).unwrap();
        fs.set_num_pieces(pieces);
        let layer: Vec<Sha256Hash> = (0..pieces)
            .map(|i| {
                let mut h = [0u8; 32];
                h[..4].copy_from_slice(&i.to_le_bytes());
                h[31] = 1;
                Sha256Hash(h)
            })
            .collect();
        let root = merkle_root(&layer, &merkle_pad(4, 1));
        let mut tree = MerkleTree::new(pieces as usize * 4, 4, root);
        let bytes: Vec<u8> = layer.iter().flat_map(|h| h.0).collect();
        assert!(tree.load_piece_layer(&bytes));
        (fs, tree)
    }

    fn msg_for(r: &HashRequest) -> HashMsg {
        HashMsg {
            root: Sha256Hash::default(),
            base: r.base,
            index: r.index,
            count: r.count,
            proof_layers: r.proof_layers,
        }
    }

    /// Security review 2026-10-22: a client fetching the piece layer from us the way
    /// libtorrent's hash picker does (512 pieces per request, two outstanding, replies in
    /// order) never runs out of hash-work budget, with no time passing at all, and gets the
    /// whole layer.
    #[test]
    fn a_client_fetching_piece_layers_stays_within_the_hash_budget() {
        for pieces in [3, 600, 4099, 1 << 14] {
            let (fs, ours) = one_file(pieces);
            let mut theirs = vec![MerkleTree::new(pieces as usize * 4, 4, ours.root())];
            let mut picker = HashPicker::new(&fs, &theirs);
            let all = Bitfield::new(pieces as usize, true);
            let now = Instant::now();
            let burst = HashWorkBudget::burst(fs.num_pieces());
            let mut budget = HashWorkBudget::default();
            let (mut spent, mut requests) = (0, 0);
            let mut outstanding = std::collections::VecDeque::new();
            loop {
                while outstanding.len() < 2 {
                    let Some(req) = picker.pick_hashes(&fs, &theirs, &all, now) else { break };
                    let m = msg_for(&req);
                    let cost = hash_request_cost(&fs, &m);
                    assert!(
                        budget.try_spend(cost, burst, now),
                        "{pieces} pieces: request {requests} rejected"
                    );
                    spent += cost;
                    requests += 1;
                    let hashes = ours.get_hashes(
                        m.base as usize,
                        m.index as usize,
                        m.count as usize,
                        m.proof_layers as usize,
                    );
                    assert!(!hashes.is_empty());
                    outstanding.push_back((req, hashes));
                }
                let Some((req, hashes)) = outstanding.pop_front() else { break };
                assert!(picker.add_hashes(&fs, &mut theirs, &req, &hashes).valid);
            }
            assert_eq!(theirs[0].get_piece_layer(), ours.get_piece_layer(), "{pieces} pieces");
            // about `P × (log2(P) - 9)`: 65,506 hashes for 16,384 pieces, 393,090 for 65,536
            let padded = u64::from(pieces).next_power_of_two();
            let levels = u64::from(padded.trailing_zeros().saturating_sub(9));
            assert!(spent <= padded * levels, "{pieces} pieces: {spent} in {requests} requests");
        }
    }

    /// Security review 2026-10-22: a peer repeating a request that costs a SHA-256 per piece of
    /// the file (one hash near the root, with its proof) is answered until its burst is spent,
    /// then rejected, then answered again at the refill rate.
    #[test]
    fn repeated_root_level_hash_requests_are_rejected_beyond_the_budget() {
        let pieces = 1 << 14;
        let (fs, _) = one_file(pieces);
        // the piece layer is 2 layers above the blocks; ask for one node 12 layers above it,
        // with its proof up to the root (one layer below the root's children)
        let m = HashMsg { root: Sha256Hash::default(), base: 2 + 12, index: 0, count: 1, proof_layers: 1 };
        let req = HashRequest::new(0, m.base, m.index, m.count, m.proof_layers);
        assert!(validate_hash_request(&req, &fs));
        let cost = hash_request_cost(&fs, &m);
        assert_eq!(cost, (1 << 12) - 1 + (1 << 13) - 1);
        let burst = HashWorkBudget::burst(fs.num_pieces());
        assert_eq!(burst, 14 << 14);

        let t0 = Instant::now();
        let mut budget = HashWorkBudget::default();
        let answered = (0..100).take_while(|_| budget.try_spend(cost, burst, t0)).count();
        assert_eq!(answered as u64, burst / cost);
        assert!(!budget.try_spend(cost, burst, t0 + Duration::from_millis(10)));
        // one second refills `HASH_WORK_PER_SECOND`: a few more, never the whole burst at once
        let t1 = t0 + Duration::from_secs(1);
        let again = (0..100).take_while(|_| budget.try_spend(cost, burst, t1)).count() as u64;
        assert!((1..=HASH_WORK_PER_SECOND / cost + 1).contains(&again), "{again}");
        // a cheap request (the piece layer, no proof) still goes through
        let cheap = HashMsg { base: 2, count: 512, proof_layers: 0, ..m };
        assert_eq!(hash_request_cost(&fs, &cheap), 0);
        assert!(budget.try_spend(hash_request_cost(&fs, &cheap), burst, t1 + Duration::from_millis(10)));
        // costs saturate instead of overflowing
        let silly = HashMsg { base: i32::MAX, count: i32::MAX, proof_layers: i32::MAX, ..m };
        assert_eq!(hash_request_cost(&fs, &silly), u64::MAX);
    }
}
