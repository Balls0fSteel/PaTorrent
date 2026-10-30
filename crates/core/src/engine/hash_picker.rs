// Ported from libtorrent-rasterbar 2.0.11: src/hash_picker.cpp, include/libtorrent/hash_picker.hpp,
// test/test_hash_picker.cpp; a test from 2.0.14 (114f65efc2)
// Copyright (c) 2017, Steven Siloti; 2017-2021, Arvid Norberg. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.

//! Which merkle hashes of a v2 torrent to ask peers for (libtorrent `hash_picker`), and the
//! glue between received hashes / block hashes and the files' merkle trees.
//!
//! ```text
//! merkle tree for a file:
//!
//!              ^            x
//! proof_layer  |    x                x
//!        ^      x       [****************]
//!        |    x   x   x   x    x   x   x   x
//!   base |   x x x x x x x x  x x x x x x x x  <- block hash layer, leaves
//!               ------->
//!               index
//!                       ----------------->
//!                       count
//! ```
//!
//! Piece-layer hashes are asked for 512 at a time. When a piece fails its check, the hashes of
//! its blocks are asked for, to find the bad blocks. The trees are owned by the torrent and
//! passed in, so the picker is plain state.

use std::time::{Duration, Instant};

use crate::metainfo::file_storage::{DEFAULT_BLOCK_SIZE, FileStorage};
use crate::metainfo::merkle::*;
use crate::metainfo::merkle_tree::{MerkleTree, SetBlockResult};
use crate::types::Sha256Hash;
use crate::util::bitfield::Bitfield;

const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(3);

/// Piece-layer hashes per request.
const PIECES_PER_REQUEST: usize = 512;

/// A range of hashes in the merkle tree of `file` (libtorrent `hash_request`), as sent in BEP 52
/// hash request, hashes and hash reject messages. The wire fields are signed 32-bit integers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HashRequest {
    pub file: usize,
    /// The tree layer, 0 for the leaves (block hashes).
    pub base: i32,
    /// The first hash within that layer.
    pub index: i32,
    pub count: i32,
    pub proof_layers: i32,
}

impl HashRequest {
    pub fn new(file: usize, base: i32, index: i32, count: i32, proof_layers: i32) -> HashRequest {
        HashRequest { file, base, index, count, proof_layers }
    }

    /// Uncle hashes that come after the `count` hashes in a reply: the proof layers not
    /// computable from the hashes themselves.
    pub fn num_proof_hashes(&self) -> usize {
        let computable = merkle_num_layers(merkle_num_leafs(self.count.max(1) as usize)) as i64 - 1;
        (self.proof_layers as i64 - computable).max(0) as usize
    }
}

/// Checks a hash request (ours or a peer's, or the header of a hashes message) against the
/// file's tree: the layer, the range within it and the proof layers must exist. `count` is
/// capped at 8192, which caps the piece size the block hashes of a whole piece can be asked for
/// at 128 MiB.
pub fn validate_hash_request(hr: &HashRequest, fs: &FileStorage) -> bool {
    if hr.file >= fs.num_files()
        || hr.base < 0
        || hr.index < 0
        || hr.count <= 0
        || hr.count > 8192
        || hr.proof_layers < 0
    {
        return false;
    }
    let num_leafs = merkle_num_leafs(fs.file_num_blocks(hr.file) as usize);
    let num_layers = merkle_num_layers(num_leafs) as i64;
    let (base, index, count, proof_layers) =
        (hr.base as i64, hr.index as i64, hr.count as i64, hr.proof_layers as i64);
    if base >= num_layers {
        return false;
    }
    // the number of hashes in that layer
    let level_size = (num_leafs >> base) as i64;
    if index >= level_size || index + count > level_size {
        return false;
    }
    proof_layers < num_layers - base
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetBlockHashStatus {
    /// The block hash is verified.
    Success,
    /// It cannot be verified yet.
    Unknown,
    /// The block hash was known and differs.
    BlockHashFailed,
    /// The blocks of a subtree (this one included) do not hash to its root.
    PieceHashFailed,
}

/// What `set_block_hash` found, with the verified (or failed) range of blocks as an offset from
/// the first block of the piece (libtorrent `set_block_hash_result`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetBlockHashResult {
    pub status: SetBlockHashStatus,
    pub first_verified_block: i64,
    pub num_verified: i64,
}

impl SetBlockHashResult {
    fn status(status: SetBlockHashStatus) -> SetBlockHashResult {
        SetBlockHashResult { status, first_verified_block: 0, num_verified: 0 }
    }

    /// The pieces the verified range covers, given the piece the block was in.
    pub fn piece_range(&self, piece: u32, blocks_per_piece: u32) -> std::ops::Range<u32> {
        let bpp = blocks_per_piece as i64;
        let start = piece as i64 + self.first_verified_block / bpp;
        let end = start + self.num_verified / bpp;
        start.max(0) as u32..end.max(0) as u32
    }
}

/// The outcome of a hashes message (libtorrent `add_hashes_result`): `valid` false means the
/// hashes did not validate (the peer sent garbage). Pieces whose blocks were already hashed and
/// can now be checked are listed as passed or failed (with the failed blocks within the piece;
/// none listed when only the piece is known to be bad).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddHashesOutcome {
    pub valid: bool,
    pub hash_failed: Vec<(u32, Vec<usize>)>,
    pub hash_passed: Vec<u32>,
}

impl AddHashesOutcome {
    fn invalid() -> AddHashesOutcome {
        AddHashesOutcome::default()
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PieceHashRequest {
    last_request: Option<Instant>,
    num_requests: i32,
    have: bool,
}

#[derive(Clone, Copy, Debug)]
struct PieceBlockRequest {
    file: usize,
    /// The piece, counted from the file's first piece.
    piece: u32,
    last_request: Option<Instant>,
    num_requests: i32,
}

#[derive(Clone, Debug)]
pub struct HashPicker {
    /// Per file, one entry per span of 512 pieces.
    piece_hash_requested: Vec<Vec<PieceHashRequest>>,
    /// Pieces that failed their check, whose block hashes are wanted; kept sorted by the
    /// number of requests sent.
    piece_block_requests: Vec<PieceBlockRequest>,
    /// Tree levels within a piece: 0 for 16 KiB pieces, 1 for 32 KiB and so on.
    piece_layer: usize,
    /// Tree levels of a 512-piece span, down to the blocks.
    piece_tree_root_layer: usize,
}

impl HashPicker {
    pub fn new(files: &FileStorage, trees: &[MerkleTree]) -> HashPicker {
        let piece_layer = merkle_num_layers((files.piece_length() / DEFAULT_BLOCK_SIZE) as usize);
        let mut piece_hash_requested = vec![Vec::new(); trees.len()];
        for f in 0..files.num_files().min(trees.len()) {
            if files.pad_file_at(f) || files.file_size(f) <= files.piece_length() as u64 {
                continue;
            }
            let tree = &trees[f];
            let v = tree.verified_leafs();
            let num_pieces = files.file_num_pieces(f) as usize;
            let requested = &mut piece_hash_requested[f];
            requested.resize(num_pieces.div_ceil(PIECES_PER_REQUEST), PieceHashRequest::default());

            let piece_layer_idx =
                merkle_num_layers(merkle_num_leafs(files.file_num_blocks(f) as usize)) - piece_layer;
            let piece_layer_start = merkle_layer_start(piece_layer_idx);

            // flag the spans whose hashes are known so they are not asked for again
            for (i, r) in requested.iter_mut().enumerate() {
                let mut j = i * PIECES_PER_REQUEST;
                loop {
                    if j == (i + 1) * PIECES_PER_REQUEST || j >= num_pieces {
                        r.have = true;
                        break;
                    }
                    let known = if files.piece_length() == DEFAULT_BLOCK_SIZE {
                        j < v.len() && v.get(j)
                    } else {
                        tree.has_node(piece_layer_start + j)
                    };
                    if !known {
                        break;
                    }
                    j += 1;
                }
            }
        }
        HashPicker {
            piece_hash_requested,
            piece_block_requests: Vec::new(),
            piece_layer,
            piece_tree_root_layer: piece_layer + merkle_num_layers(PIECES_PER_REQUEST),
        }
    }

    pub fn piece_layer(&self) -> usize {
        self.piece_layer
    }

    /// The next request to send a peer that has `pieces`, if any: block hashes of failed pieces
    /// first, then piece-layer hashes for spans the peer has a piece of.
    pub fn pick_hashes(
        &mut self,
        files: &FileStorage,
        trees: &[MerkleTree],
        pieces: &Bitfield,
        now: Instant,
    ) -> Option<HashRequest> {
        let blocks_per_piece = (files.piece_length() / DEFAULT_BLOCK_SIZE) as usize;

        // forget pieces whose block hashes have arrived since (libtorrent keeps them, but only
        // ever asks once; see docs/PORTING.md)
        self.piece_block_requests.retain(|r| {
            let first_block = r.piece as usize * blocks_per_piece;
            let piece = files.piece_index_at_file(r.file) + r.piece;
            let blocks = files.blocks_in_piece2(piece) as usize;
            let tree = &trees[r.file];
            blocks == 0
                || first_block + blocks > tree.num_blocks()
                || !tree.blocks_verified(first_block, blocks)
        });
        let due = |last: Option<Instant>| {
            last.is_none_or(|t| now.saturating_duration_since(t) > MIN_REQUEST_INTERVAL)
        };
        if let Some(pos) = self.piece_block_requests.iter().position(|e| due(e.last_request)) {
            let req = &mut self.piece_block_requests[pos];
            // blocks from the start of the file
            let first_block = req.piece as usize * blocks_per_piece;
            let node = files.file_first_block_node(req.file) as usize + first_block;
            let proof_layers = Self::layers_to_verify(files, trees, req.file, node)
                + merkle_num_layers(blocks_per_piece) as i64;
            let hash_req = HashRequest::new(
                req.file,
                0,
                first_block as i32,
                blocks_per_piece as i32,
                proof_layers.max(0) as i32,
            );
            req.num_requests += 1;
            req.last_request = Some(now);
            self.piece_block_requests.sort_by_key(|r| r.num_requests);
            return Some(hash_req);
        }

        for fidx in 0..self.piece_hash_requested.len() {
            if files.pad_file_at(fidx) || files.file_size(fidx) == 0 {
                continue;
            }
            let file_first_piece = (files.file_offset(fidx) / files.piece_length() as u64) as usize;
            let num_pieces = files.file_num_pieces(fidx) as usize;
            let num_layers = Self::file_num_layers(files, fidx);
            let piece_tree_root_layer = num_layers.saturating_sub(self.piece_tree_root_layer);
            let piece_tree_root_start = merkle_layer_start(piece_tree_root_layer);

            for i in 0..self.piece_hash_requested[fidx].len() {
                let r = self.piece_hash_requested[fidx][i];
                if r.have
                    || r.last_request.is_some_and(|t| now.saturating_duration_since(t) < MIN_REQUEST_INTERVAL)
                {
                    continue;
                }
                let span = i * PIECES_PER_REQUEST..((i + 1) * PIECES_PER_REQUEST).min(num_pieces);
                if !span
                    .clone()
                    .any(|p| file_first_piece + p < pieces.len() && pieces.get(file_first_piece + p))
                {
                    continue;
                }
                let piece_tree_root = piece_tree_root_start + i;
                let r = &mut self.piece_hash_requested[fidx][i];
                r.num_requests += 1;
                r.last_request = Some(now);

                let piece_tree_num_layers = num_layers - piece_tree_root_layer - self.piece_layer;
                let proof_layers = Self::layers_to_verify(files, trees, fidx, piece_tree_root)
                    + piece_tree_num_layers as i64;
                let count = PIECES_PER_REQUEST.min(merkle_num_leafs(num_pieces - i * PIECES_PER_REQUEST));
                return Some(HashRequest::new(
                    fidx,
                    self.piece_layer as i32,
                    (i * PIECES_PER_REQUEST) as i32,
                    count as i32,
                    proof_layers.max(0) as i32,
                ));
            }
        }
        None
    }

    /// Adds the hashes of a hashes message replying to `req` (validated with
    /// `validate_hash_request`): the `count` hashes followed by the uncle hashes.
    pub fn add_hashes(
        &mut self,
        files: &FileStorage,
        trees: &mut [MerkleTree],
        req: &HashRequest,
        hashes: &[Sha256Hash],
    ) -> AddHashesOutcome {
        if !validate_hash_request(req, files) || req.file >= trees.len() {
            return AddHashesOutcome::invalid();
        }
        let (base, index, count) = (req.base as usize, req.index as usize, req.count as usize);
        let file_num_pieces = files.file_num_pieces(req.file) as usize;
        let unpadded_count = count.min(file_num_pieces.saturating_sub(index));

        if count + req.num_proof_hashes() != hashes.len() {
            return AddHashesOutcome::invalid();
        }
        // piece hashes are only asked for in spans of 512 (or the rest of the file)
        if base == self.piece_layer
            && count != PIECES_PER_REQUEST
            && (count > PIECES_PER_REQUEST || unpadded_count != file_num_pieces.saturating_sub(index))
        {
            return AddHashesOutcome::invalid();
        }
        // only the piece and block layers are taken
        if base != self.piece_layer && base != 0 {
            return AddHashesOutcome::invalid();
        }

        // the hashes asked for, then the uncle hashes anchoring them in the tree
        let (hashes, uncle_hashes) = hashes.split_at(count);
        let num_layers = Self::file_num_layers(files, req.file);
        if num_layers <= base {
            return AddHashesOutcome::invalid();
        }
        let base_layer_idx = num_layers - base;
        let dest_start_idx = merkle_to_flat_index(base_layer_idx, index);
        let file_piece_offset = files.piece_index_at_file(req.file);
        match trees[req.file].add_hashes(dest_start_idx, file_piece_offset, hashes, uncle_hashes) {
            None => AddHashesOutcome::invalid(),
            Some(r) => AddHashesOutcome { valid: true, hash_failed: r.failed, hash_passed: r.passed },
        }
    }

    /// Sets the hash of the block at `offset` in `piece`, computed from its data.
    pub fn set_block_hash(
        &mut self,
        files: &FileStorage,
        trees: &mut [MerkleTree],
        piece: u32,
        offset: u32,
        h: &Sha256Hash,
    ) -> SetBlockHashResult {
        let f = files.file_index_at_piece(piece);
        if files.pad_file_at(f) || f >= trees.len() {
            return SetBlockHashResult {
                status: SetBlockHashStatus::Success,
                first_verified_block: 0,
                num_verified: 0,
            };
        }
        let file_first_piece = files.piece_index_at_file(f);
        let block_offset = piece as u64 * files.piece_length() as u64 + offset as u64 - files.file_offset(f);
        let block_index = (block_offset / DEFAULT_BLOCK_SIZE as u64) as usize;
        let tree = &mut trees[f];
        if h.is_zero() || block_index >= tree.num_blocks() {
            return SetBlockHashResult::status(SetBlockHashStatus::BlockHashFailed);
        }

        let (result, leafs_index, leafs_size) = tree.set_block(block_index, h);
        let status = match result {
            SetBlockResult::Unknown => return SetBlockHashResult::status(SetBlockHashStatus::Unknown),
            SetBlockResult::BlockHashFailed => {
                return SetBlockHashResult::status(SetBlockHashStatus::BlockHashFailed);
            }
            SetBlockResult::HashFailed => SetBlockHashStatus::PieceHashFailed,
            SetBlockResult::Ok => SetBlockHashStatus::Success,
        };
        let blocks_per_piece = (files.piece_length() / DEFAULT_BLOCK_SIZE) as i64;
        let leafs_index = leafs_index as i64;
        SetBlockHashResult {
            status,
            first_verified_block: leafs_index - (piece - file_first_piece) as i64 * blocks_per_piece,
            num_verified: (leafs_size as i64)
                .min(files.file_num_pieces(f) as i64 * blocks_per_piece - leafs_index),
        }
    }

    /// A peer rejected `req` (one of ours): ask someone else.
    pub fn hashes_rejected(&mut self, req: &HashRequest) {
        if req.count <= 0 || req.index < 0 {
            return;
        }
        if req.base as usize == self.piece_layer && req.index as usize % PIECES_PER_REQUEST == 0 {
            if let Some(spans) = self.piece_hash_requested.get_mut(req.file) {
                let mut i = req.index as usize;
                while i < req.index as usize + req.count as usize {
                    if let Some(r) = spans.get_mut(i / PIECES_PER_REQUEST) {
                        r.last_request = None;
                        r.num_requests -= 1;
                    }
                    i += PIECES_PER_REQUEST;
                }
            }
        }
        if req.base == 0 {
            // the block hashes of a failed piece (libtorrent only handles piece-layer rejects)
            let piece = (req.index as usize >> self.piece_layer) as u32;
            for r in &mut self.piece_block_requests {
                if r.file == req.file && r.piece == piece {
                    r.last_request = None;
                }
            }
        }
    }

    /// Piece `index` failed its check: ask for its block hashes to find the bad blocks.
    pub fn verify_block_hashes(&mut self, files: &FileStorage, index: u32) {
        let fidx = files.file_index_at_piece(index);
        let piece = index - files.piece_index_at_file(fidx);
        if self.piece_block_requests.iter().any(|r| r.file == fidx && r.piece == piece) {
            return;
        }
        self.piece_block_requests
            .insert(0, PieceBlockRequest { file: fidx, piece, last_request: None, num_requests: 0 });
    }

    /// True if the piece-layer hash of piece `index` is known.
    pub fn have_hash(&self, files: &FileStorage, trees: &[MerkleTree], index: u32) -> bool {
        let f = files.file_index_at_piece(index);
        if files.file_size(f) <= files.piece_length() as u64 {
            return true;
        }
        let file_first_piece = (files.file_offset(f) / files.piece_length() as u64) as u32;
        trees[f].has_node(files.file_first_piece_node(f) as usize + (index - file_first_piece) as usize)
    }

    /// True if every block hash of `file` is known.
    pub fn have_all_file(trees: &[MerkleTree], file: usize) -> bool {
        trees[file].is_complete()
    }

    pub fn have_all(files: &FileStorage, trees: &[MerkleTree]) -> bool {
        (0..files.num_files()).all(|f| Self::have_all_file(trees, f))
    }

    /// True if the block hashes of `piece` are verified.
    pub fn piece_verified(files: &FileStorage, trees: &[MerkleTree], piece: u32) -> bool {
        let f = files.file_index_at_piece(piece);
        let file_first_piece = (files.file_offset(f) / files.piece_length() as u64) as u32;
        let block_offset =
            (piece - file_first_piece) as usize * (files.piece_length() / DEFAULT_BLOCK_SIZE) as usize;
        let blocks_in_piece = files.blocks_in_piece2(piece) as usize;
        trees[f].blocks_verified(block_offset, blocks_in_piece)
    }

    /// Proof layers needed to verify node `node` of `file`'s tree: up to the first known
    /// ancestor; -1 for the root, which needs none.
    fn layers_to_verify(files: &FileStorage, trees: &[MerkleTree], file: usize, mut node: usize) -> i64 {
        if node == 0 {
            return -1;
        }
        let file_internal_layers =
            merkle_num_layers(merkle_num_leafs(files.file_num_pieces(file) as usize)) as i64 - 1;
        let tree = &trees[file];
        let mut layers = 0;
        loop {
            node = merkle_get_parent(node);
            if tree.has_node(node) {
                break;
            }
            layers += 1;
            if layers == file_internal_layers {
                return layers;
            }
        }
        layers
    }

    fn file_num_layers(files: &FileStorage, file: usize) -> usize {
        merkle_num_layers(merkle_num_leafs(files.file_num_blocks(file) as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metainfo::FileFlags;

    const BLOCK: u32 = DEFAULT_BLOCK_SIZE;

    fn fs_one_file(piece_length: u32, size: u64) -> FileStorage {
        let mut fs = FileStorage::new();
        fs.set_piece_length(piece_length);
        fs.add_file("test/tmp1", size, FileFlags::empty(), 0, "", None).unwrap();
        fs.set_num_pieces(size.div_ceil(piece_length as u64) as u32);
        fs
    }

    fn root1() -> Sha256Hash {
        Sha256Hash::from_hex("0000000000000000000000000000000000000000000000000000000000000001").unwrap()
    }

    fn uncles(tree: &[Sha256Hash], mut i: usize) -> Vec<Sha256Hash> {
        let mut ret = Vec::new();
        while i > 0 {
            ret.push(tree[merkle_get_sibling(i)]);
            i = merkle_get_parent(i);
        }
        ret
    }

    #[test]
    fn reject_piece_request() {
        let fs = fs_one_file(BLOCK, 4 * 512 * BLOCK as u64);
        let trees = vec![MerkleTree::new(4 * 512, 1, root1())];
        let mut picker = HashPicker::new(&fs, &trees);
        let pieces = Bitfield::new(4 * 512, true);
        let now = Instant::now();
        let picked = picker.pick_hashes(&fs, &trees, &pieces, now).unwrap();
        picker.hashes_rejected(&picked);
        let picked2 = picker.pick_hashes(&fs, &trees, &pieces, now).unwrap();
        assert_eq!(picked, picked2);
    }

    #[test]
    fn add_leaf_hashes() {
        let fs = fs_one_file(BLOCK, 4 * 512 * BLOCK as u64);
        let full_tree = build_tree(4 * 512);
        let mut trees = vec![MerkleTree::new(4 * 512, 1, full_tree[0])];
        let mut picker = HashPicker::new(&fs, &trees);
        let leafs = merkle_num_leafs(4 * 512);
        let pieces_start = full_tree.len() - leafs;

        let mut hashes = full_tree[pieces_start..pieces_start + 512].to_vec();
        hashes.extend(uncles(&full_tree, 3));
        assert!(picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 0, 0, 512, 10), &hashes).valid);

        let hashes = &full_tree[pieces_start + 512..pieces_start + 1024];
        assert!(picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 0, 512, 512, 0), hashes).valid);

        let mut hashes = full_tree[pieces_start + 1024..pieces_start + 1536].to_vec();
        hashes.extend(uncles(&full_tree, 5));
        assert!(picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 0, 1024, 512, 10), &hashes).valid);

        let hashes = &full_tree[pieces_start + 1536..pieces_start + 2048];
        assert!(picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 0, 1536, 512, 0), hashes).valid);

        assert_eq!(trees[0].build_vector(), full_tree);
    }

    #[test]
    fn add_piece_hashes() {
        let fs = fs_one_file(4 * BLOCK, 4 * 1024 * BLOCK as u64);
        let full_tree = build_tree(4 * 1024);
        let mut trees = vec![MerkleTree::new(4 * 1024, 4, full_tree[0])];
        let mut picker = HashPicker::new(&fs, &trees);
        let pieces_start = merkle_num_nodes(1024) - 1024;

        let mut hashes = full_tree[pieces_start..pieces_start + 512].to_vec();
        hashes.push(full_tree[2]);
        assert!(picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 2, 0, 512, 9), &hashes).valid);

        let hashes = &full_tree[pieces_start + 512..pieces_start + 1024];
        assert!(picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 2, 512, 512, 8), hashes).valid);

        let cmp = trees[0].build_vector();
        assert_eq!(&cmp[..merkle_num_nodes(1024)], &full_tree[..merkle_num_nodes(1024)]);
    }

    #[test]
    fn add_piece_hashes_padded_and_unpadded() {
        for (count, n) in [(8, 8), (5, 5)] {
            let fs = fs_one_file(4 * BLOCK, 4 * 1029 * BLOCK as u64);
            let full_tree = build_tree(4 * 1029);
            let mut trees = vec![MerkleTree::new(4 * 1029, 4, full_tree[0])];
            let mut picker = HashPicker::new(&fs, &trees);
            let pieces_start = merkle_num_nodes(merkle_num_leafs(1029)) - merkle_num_leafs(1029);

            // 5 pieces after 1024, rounded up to 8 when padded
            let mut hashes = full_tree[pieces_start + 1024..pieces_start + 1024 + n].to_vec();
            let mut proof = merkle_get_parent(merkle_get_parent(merkle_get_parent(pieces_start + 1024)));
            while proof > 0 {
                hashes.push(full_tree[merkle_get_sibling(proof)]);
                proof = merkle_get_parent(proof);
            }
            let r = picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 2, 1024, count, 10), &hashes);
            assert!(r.valid, "count {count}");
            trees[0].check_invariant();
        }
    }

    // when blocks_per_piece == 1, a block-hash request is always for a lone leaf (count == 1).
    // get_hashes must return the right number of uncle hashes for that shape whenever a real,
    // non-zero proof is needed
    #[test]
    fn add_hashes_lone_leaf_with_proof() {
        let fs = fs_one_file(BLOCK, 10 * BLOCK as u64);
        let full_tree = build_tree(10);

        // a fully populated reference tree, standing in for a peer that can answer any request
        let mut server_tree = MerkleTree::new(10, 1, full_tree[0]);
        server_tree.load_tree(&full_tree, &Bitfield::new(merkle_num_leafs(10), false));

        let mut trees = vec![MerkleTree::new(10, 1, full_tree[0])];
        let mut picker = HashPicker::new(&fs, &trees);

        // piece 5 failed verification; nothing else is known yet, so the resulting block-hash
        // request needs a real, non-trivial proof
        picker.verify_block_hashes(&fs, 5);

        let pieces = Bitfield::new(10, false);
        let picked = picker.pick_hashes(&fs, &trees, &pieces, Instant::now()).unwrap();
        assert_eq!((picked.base, picked.index, picked.count), (0, 5, 1));
        assert!(picked.proof_layers > 0);

        let hashes = server_tree.get_hashes(
            picked.base as usize,
            picked.index as usize,
            picked.count as usize,
            picked.proof_layers as usize,
        );
        // a legitimate, correctly sized response; it must be accepted. libtorrent passes it to
        // the picker's add_hashes, whose check of the block-request shape for one block per
        // piece (piece layer == block layer) came in later fixes not ported here; the tree
        // takes it directly
        assert_eq!(hashes.len(), 1 + picked.num_proof_hashes());
        let (hashes, uncle_hashes) = hashes.split_at(1);
        let dest_start_idx = merkle_first_leaf(merkle_num_leafs(10)) + 5;
        assert!(trees[0].add_hashes(dest_start_idx, 0, hashes, uncle_hashes).is_some());
        assert!(trees[0].blocks_verified(5, 1));
    }

    // a file of 513 pieces: its second span of piece hashes is a single piece (count == 1),
    // whose reply carries that piece's own (padding) sibling as the first proof hash
    #[test]
    fn piece_layer_round_trip_513_pieces() {
        const PIECES: usize = 513;
        let fs = fs_one_file(4 * BLOCK, (4 * PIECES) as u64 * BLOCK as u64);
        let full_tree = build_tree(4 * PIECES);
        let piece_layer_start = merkle_num_nodes(merkle_num_leafs(PIECES)) - merkle_num_leafs(PIECES);
        let piece_layer = &full_tree[piece_layer_start..piece_layer_start + PIECES];

        // a seed: the full tree, or the piece layer from a .torrent
        let mut full_seed = MerkleTree::new(4 * PIECES, 4, full_tree[0]);
        full_seed.load_tree(&full_tree, &Bitfield::new(merkle_num_leafs(4 * PIECES), false));
        let mut layer_seed = MerkleTree::new(4 * PIECES, 4, full_tree[0]);
        assert!(layer_seed.load_piece_layer(&piece_layer.iter().flat_map(|h| h.0).collect::<Vec<u8>>()));

        for seed in [&full_seed, &layer_seed] {
            let mut trees = vec![MerkleTree::new(4 * PIECES, 4, full_tree[0])];
            let mut picker = HashPicker::new(&fs, &trees);
            let pieces = Bitfield::new(PIECES, true);
            let now = Instant::now();
            let mut spans = Vec::new();
            while let Some(req) = picker.pick_hashes(&fs, &trees, &pieces, now) {
                let hashes = seed.get_hashes(
                    req.base as usize,
                    req.index as usize,
                    req.count as usize,
                    req.proof_layers as usize,
                );
                assert_eq!(hashes.len(), req.count as usize + req.num_proof_hashes(), "{req:?}");
                assert!(picker.add_hashes(&fs, &mut trees, &req, &hashes).valid, "{req:?}");
                spans.push((req.index, req.count));
            }
            assert_eq!(spans, [(0, 512), (512, 1)]);
            assert_eq!(trees[0].get_piece_layer(), piece_layer);
        }
    }

    #[test]
    fn add_bad_hashes() {
        let fs = fs_one_file(4 * BLOCK, 4 * 512 * BLOCK as u64);
        let full_tree = build_tree(4 * 512);
        let mut trees = vec![MerkleTree::new(4 * 512, 4, full_tree[0])];
        let mut picker = HashPicker::new(&fs, &trees);

        // bogus hashes
        let hashes = vec![Sha256Hash::default(); 512];
        assert!(!picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 2, 0, 512, 0), &hashes).valid);

        // a bad hash
        let pieces_start = full_tree.len() - 512;
        let mut hashes = full_tree[pieces_start..].to_vec();
        hashes.last_mut().unwrap().0[1] ^= 0xaa;
        assert!(!picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 2, 0, 512, 0), &hashes).valid);

        // the wrong number of hashes, and a layer between the piece and block layers
        assert!(!picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 2, 0, 512, 0), &hashes[1..]).valid);
        assert!(!picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 1, 0, 2, 0), &hashes[..2]).valid);
    }

    #[test]
    fn bad_block_hash() {
        let fs = fs_one_file(BLOCK, 4 * 512 * BLOCK as u64);
        let full_tree = build_tree(4 * 512);
        let mut trees = vec![MerkleTree::new(4 * 512, 1, full_tree[0])];
        trees[0].set_block(1, &root1());
        let mut picker = HashPicker::new(&fs, &trees);

        let leafs_start = full_tree.len() - merkle_num_leafs(4 * 512);
        let mut hashes = full_tree[leafs_start..leafs_start + 512].to_vec();
        hashes.extend(uncles(&full_tree, 3));
        let r = picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 0, 0, 512, 10), &hashes);
        assert!(r.valid);
        assert_eq!(r.hash_failed, vec![(1, vec![0])]);
    }

    #[test]
    fn set_block_hash() {
        let fs = fs_one_file(4 * BLOCK, 4 * 512 * BLOCK as u64);
        let full_tree = build_tree(4 * 512);
        let mut trees = vec![MerkleTree::new(4 * 512, 4, full_tree[0])];
        trees[0].load_tree(&full_tree, &Bitfield::new(merkle_num_leafs(4 * 512), false));
        let first_leaf = full_tree.len() - merkle_num_leafs(4 * 512);
        let mut picker = HashPicker::new(&fs, &trees);

        let r = picker.set_block_hash(&fs, &mut trees, 1, BLOCK, &full_tree[first_leaf + 5]);
        assert_eq!(r.status, SetBlockHashStatus::Success);
        let r = picker.set_block_hash(&fs, &mut trees, 2, BLOCK * 2, &full_tree[first_leaf + 10]);
        assert_eq!(r.status, SetBlockHashStatus::Success);
        let r = picker.set_block_hash(
            &fs,
            &mut trees,
            2,
            BLOCK * 2,
            &Sha256Hash(*b"01234567890123456789012345678901"),
        );
        assert_eq!(r.status, SetBlockHashStatus::BlockHashFailed);
    }

    #[test]
    fn set_block_hash_fail_and_pass() {
        for pass in [false, true] {
            let fs = fs_one_file(4 * BLOCK, 4 * 512 * BLOCK as u64);
            let mut full_tree = build_tree(4 * 512);
            let mut trees = vec![MerkleTree::new(4 * 512, 4, full_tree[0])];
            let first_leaf = full_tree.len() - merkle_num_leafs(4 * 512);

            // clear the inner nodes of piece 3 and one of its leaves
            full_tree[merkle_get_parent(first_leaf + 12)].clear();
            full_tree[merkle_get_parent(first_leaf + 14)].clear();
            let orig_hash = full_tree[first_leaf + 13];
            full_tree[first_leaf + 13].clear();
            trees[0].load_tree(&full_tree, &Bitfield::new(merkle_num_leafs(4 * 512), false));
            let mut picker = HashPicker::new(&fs, &trees);

            for (offset, leaf) in [(0, 12), (2, 14), (3, 15)] {
                let r =
                    picker.set_block_hash(&fs, &mut trees, 3, offset * BLOCK, &full_tree[first_leaf + leaf]);
                assert_eq!(r.status, SetBlockHashStatus::Unknown);
            }
            if pass {
                let r = picker.set_block_hash(&fs, &mut trees, 3, BLOCK, &orig_hash);
                assert_eq!(r.status, SetBlockHashStatus::Success);
                assert_eq!(r.piece_range(3, 4), 3..4);
                assert!(HashPicker::piece_verified(&fs, &trees, 3));
            } else {
                let r = picker.set_block_hash(
                    &fs,
                    &mut trees,
                    3,
                    BLOCK,
                    &Sha256Hash(*b"01234567890123456789012345678901"),
                );
                assert_eq!(r.status, SetBlockHashStatus::PieceHashFailed);
                for leaf in 12..16 {
                    assert!(trees[0].get(merkle_get_parent(first_leaf + leaf)).is_zero());
                }
                assert!(!HashPicker::piece_verified(&fs, &trees, 3));
            }
            trees[0].check_invariant();
        }
    }

    #[test]
    fn pass_piece() {
        let fs = fs_one_file(4 * BLOCK, 4 * 512 * BLOCK as u64);
        let full_tree = build_tree(4 * 512);
        let mut trees = vec![MerkleTree::new(4 * 512, 4, full_tree[0])];
        let mut picker = HashPicker::new(&fs, &trees);
        let first_leaf = full_tree.len() - merkle_num_leafs(4 * 512);

        for i in 0..4 {
            let r = picker.set_block_hash(&fs, &mut trees, 0, BLOCK * i, &full_tree[first_leaf + i as usize]);
            assert_eq!(r.status, SetBlockHashStatus::Unknown);
        }
        let pieces_start = merkle_num_nodes(512) - 512;
        let hashes = &full_tree[pieces_start..pieces_start + 512];
        let r = picker.add_hashes(&fs, &mut trees, &HashRequest::new(0, 2, 0, 512, 8), hashes);
        assert!(r.valid);
        assert_eq!(r.hash_passed, vec![0]);
    }

    #[test]
    fn only_pick_have_pieces() {
        let fs = fs_one_file(BLOCK, 4 * 512 * BLOCK as u64);
        let trees = vec![MerkleTree::new(4 * 512, 1, root1())];
        let mut picker = HashPicker::new(&fs, &trees);
        let mut pieces = Bitfield::new(4 * 512, false);
        pieces.set(512, true);
        pieces.set(1537, true);
        let now = Instant::now();
        let picked: Vec<_> = (0..3).map(|_| picker.pick_hashes(&fs, &trees, &pieces, now)).collect();
        assert_eq!(picked[0], Some(HashRequest::new(0, 0, 512, 512, 10)));
        assert_eq!(picked[1], Some(HashRequest::new(0, 0, 1536, 512, 10)));
        assert_eq!(picked[2], None);
        // after the interval they are due again
        let later = now + MIN_REQUEST_INTERVAL + Duration::from_millis(1);
        assert_eq!(
            picker.pick_hashes(&fs, &trees, &pieces, later),
            Some(HashRequest::new(0, 0, 512, 512, 10))
        );
    }

    #[test]
    fn known_piece_layers_are_not_picked() {
        // the piece layer from the .torrent is loaded: nothing to ask for
        let fs = fs_one_file(4 * BLOCK, 4 * 1024 * BLOCK as u64);
        let full_tree = build_tree(4 * 1024);
        let mut trees = vec![MerkleTree::new(4 * 1024, 4, full_tree[0])];
        let pieces_start = merkle_num_nodes(1024) - 1024;
        let layer: Vec<u8> = full_tree[pieces_start..pieces_start + 1024].iter().flat_map(|h| h.0).collect();
        assert!(trees[0].load_piece_layer(&layer));
        let mut picker = HashPicker::new(&fs, &trees);
        let pieces = Bitfield::new(1024, true);
        assert_eq!(picker.pick_hashes(&fs, &trees, &pieces, Instant::now()), None);
        assert!(picker.have_hash(&fs, &trees, 700));
    }

    #[test]
    fn failed_piece_block_hashes_are_asked_for_until_known() {
        let fs = fs_one_file(4 * BLOCK, 4 * 1024 * BLOCK as u64);
        let full_tree = build_tree(4 * 1024);
        let mut trees = vec![MerkleTree::new(4 * 1024, 4, full_tree[0])];
        let pieces_start = merkle_num_nodes(1024) - 1024;
        let layer: Vec<u8> = full_tree[pieces_start..pieces_start + 1024].iter().flat_map(|h| h.0).collect();
        assert!(trees[0].load_piece_layer(&layer));
        let mut picker = HashPicker::new(&fs, &trees);
        let pieces = Bitfield::new(1024, true);
        let now = Instant::now();

        picker.verify_block_hashes(&fs, 5);
        picker.verify_block_hashes(&fs, 5);
        // the 4 block hashes of piece 5: the layer between them and the known piece hash, plus
        // the 2 layers within the piece (libtorrent's count, which asks for 2 uncle hashes the
        // known piece hash makes unnecessary)
        let req = picker.pick_hashes(&fs, &trees, &pieces, now).unwrap();
        assert_eq!(req, HashRequest::new(0, 0, 20, 4, 3));
        assert!(validate_hash_request(&req, &fs));
        assert_eq!(req.num_proof_hashes(), 2);
        assert_eq!(picker.pick_hashes(&fs, &trees, &pieces, now), None);

        // rejected: asked again at once
        picker.hashes_rejected(&req);
        assert_eq!(picker.pick_hashes(&fs, &trees, &pieces, now), Some(req));

        // the hashes arrive (bad data had been hashed for block 1): the request is done
        let first_leaf = full_tree.len() - merkle_num_leafs(4 * 1024);
        picker.set_block_hash(&fs, &mut trees, 5, BLOCK, &root1());
        let piece_node = merkle_num_nodes(1024) - 1024 + 5;
        let mut reply = full_tree[first_leaf + 20..first_leaf + 24].to_vec();
        reply.push(full_tree[merkle_get_sibling(piece_node)]);
        reply.push(full_tree[merkle_get_sibling(merkle_get_parent(piece_node))]);
        let r = picker.add_hashes(&fs, &mut trees, &req, &reply);
        assert!(r.valid);
        assert_eq!(r.hash_failed, vec![(5, vec![1])]);
        let later = now + MIN_REQUEST_INTERVAL * 2;
        assert_eq!(picker.pick_hashes(&fs, &trees, &pieces, later), None);
    }

    #[test]
    fn validate_hash_request_ranges() {
        let fs = fs_one_file(BLOCK, 2048 * BLOCK as u64);
        let num_leaves = merkle_num_leafs(2048) as i32;
        let num_layers = merkle_num_layers(num_leaves as usize) as i32;
        let (max, min) = (i32::MAX, i32::MIN);
        let ok = |b, i, c, p| validate_hash_request(&HashRequest::new(0, b, i, c, p), &fs);

        assert!(ok(0, 0, 1, 0));
        // file out of range
        assert!(!validate_hash_request(&HashRequest::new(1, 0, 0, 1, 0), &fs));
        assert!(!validate_hash_request(&HashRequest::new(usize::MAX, 0, 0, 1, 0), &fs));
        // base
        for b in [-1, num_layers, max, min] {
            assert!(!ok(b, 0, 1, 0), "base {b}");
        }
        assert!(ok(num_layers - 1, 0, 1, 0));
        // count
        for (b, i, c) in [
            (0, 0, 0),
            (0, 0, num_leaves + 1),
            (0, 100, num_leaves - 100 + 1),
            (0, 0, 8193),
            (0, 0, min),
            (0, 0, max),
            (1, 0, num_leaves / 2 + 1),
            (2, 0, num_leaves / 4 + 1),
            (3, 0, num_leaves / 8 + 1),
            (3, 5, num_leaves / 8 - 5 + 1),
        ] {
            assert!(!ok(b, i, c, 0), "{b} {i} {c}");
        }
        for (b, i, c) in [
            (0, 100, num_leaves - 100),
            (0, 0, 1),
            (0, 0, num_leaves),
            (1, 0, num_leaves / 2),
            (2, 0, num_leaves / 4),
            (3, 0, num_leaves / 8),
            (3, 5, num_leaves / 8 - 5),
        ] {
            assert!(ok(b, i, c, 0), "{b} {i} {c}");
        }
        // proof layers
        for (b, p) in [(0, num_layers), (1, num_layers), (1, min), (1, max), (1, -1)] {
            assert!(!ok(b, 0, 1, p), "{b} {p}");
        }
        assert!(ok(0, 0, 1, num_layers - 1));
        assert!(ok(1, 0, 1, num_layers - 2));
        // index near i32::MAX does not overflow
        assert!(!ok(0, max, max, 0));
    }
}
