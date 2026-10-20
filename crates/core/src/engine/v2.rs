// Ported from libtorrent-rasterbar 2.0.11: src/torrent.cpp (`handle_inconsistent_hashes`,
// `get_hashes`, `initialize_merkle_trees`), src/bt_peer_connection.cpp (`on_hash_request`,
// `on_hashes`, `on_hash_reject`)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: v2 pieces are verified on the disk thread against the piece layers (see
// `disk::PieceHasher`), so the torrent keeps no block hashes and its trees hold the piece layers
// only; hash requests for block hashes are rejected; see docs/PORTING.md.

//! BitTorrent v2 (BEP 52) handling in the engine.

use super::Engine;
use super::hash_picker::{HashRequest, validate_hash_request};
use crate::alert::Alert;
use crate::error::Error;
use crate::metainfo::merkle_tree::MerkleTree;
use crate::metainfo::{MetainfoError, TorrentInfo};
use crate::proto::peer_wire::{self, HashMsg, msg};
use crate::types::{Sha256Hash, TorrentId};
use crate::util::bitfield::Bitfield;

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

impl Engine {
    /// libtorrent `on_hash_request`: a v2 peer asks for part of a file's merkle tree; it gets
    /// the hashes or a reject. Hash messages on a v1 connection end it.
    pub(super) fn on_hash_request(&mut self, slot: usize, m: HashMsg) {
        let Some((p, t)) = self.peer_and_torrent(slot) else { return };
        if !p.protocol_v2 {
            p.close("hash request on a v1 connection", true);
            return;
        }
        let hashes = match t {
            Some(t) => match &t.ti {
                Some(ti) => {
                    let trees = t.merkle_trees.get_or_insert_with(|| build_trees(ti));
                    hashes_for(trees, ti, &m)
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        if hashes.is_empty() {
            peer_wire::encode_hash_msg(p.out(), msg::HASH_REJECT, &m);
        } else {
            peer_wire::encode_hashes(p.out(), &m, &hashes);
        }
        self.to_flush.push(slot);
    }

    /// libtorrent `on_hashes`: hashes we did not ask for (we only run v2 torrents whose piece
    /// layers are known, and ask for nothing yet) are checked against the request they claim to
    /// answer and dropped; a malformed reply ends the connection.
    pub(super) fn on_hashes(&mut self, slot: usize, m: HashMsg, hashes: &[u8]) {
        let Some((p, t)) = self.peer_and_torrent(slot) else { return };
        if !p.protocol_v2 {
            p.close("hashes on a v1 connection", true);
            return;
        }
        let Some(ti) = t.and_then(|t| t.ti.as_ref()) else { return };
        let fs = ti.orig_files();
        let file = fs.file_index_for_root(&m.root).unwrap_or(usize::MAX);
        let req = HashRequest::new(file, m.base, m.index, m.count, m.proof_layers);
        if !validate_hash_request(&req, fs)
            || hashes.len() != (m.count as usize + req.num_proof_hashes()) * 32
        {
            p.close("invalid hashes", true);
        }
    }

    /// libtorrent `on_hash_reject`: nothing was asked for, so there is nothing to retry.
    pub(super) fn on_hash_reject(&mut self, slot: usize, _m: HashMsg) {
        let Some(p) = self.peers.get_mut(slot) else { return };
        if !p.protocol_v2 {
            p.close("hash reject on a v1 connection", true);
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
