// Ported from libtorrent-rasterbar 2.0.11: src/merkle_tree.cpp, include/libtorrent/aux_/merkle_tree.hpp,
// test/test_merkle_tree.cpp; fixes from 2.0.14 (114f65efc2)
// Copyright (c) 2020-2021, Arvid Norberg; 2020, Steven Siloti. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.

//! The merkle tree of one file of a v2 torrent (libtorrent `aux::merkle_tree`).
//!
//! Each file has a root hash and a "piece layer", the tree level of whole pieces, usually known
//! from the `.torrent`. Every interior node is either valid or clear: hashes only get in with a
//! proof. Leaves (block hashes) may be unverified, for instance the hashes of data found on
//! disk before the tree is known; `block_verified` tells them apart.
//!
//! The tree is stored as compactly as what is known allows: nothing (just the root), the piece
//! layer, the block layer, or the full padded tree while it is being filled in.

use super::merkle::*;
use crate::types::Sha256Hash;
use crate::util::bitfield::Bitfield;

/// What `add_hashes` learned about previously received blocks: pieces now known to pass, and
/// pieces that failed with the indices (within the piece) of the failed blocks. An empty block
/// list means the piece failed but its block hashes are unknown.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AddHashesResult {
    pub passed: Vec<u32>,
    pub failed: Vec<(u32, Vec<usize>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetBlockResult {
    /// The block hash is verified (with the range of blocks verified with it).
    Ok,
    /// It cannot be verified yet.
    Unknown,
    /// The blocks of a known subtree, this one included, do not hash to its root.
    HashFailed,
    /// The block hash was known and differs.
    BlockHashFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// A default tree: no root, nothing.
    Uninitialized,
    /// Only the root is known; `tree` is empty.
    Empty,
    /// `tree` is the full tree, padding included.
    Full,
    /// `tree` is the piece layer, without padding, all valid.
    PieceLayer,
    /// `tree` is the block layer, without padding, all valid.
    BlockLayer,
}

#[derive(Clone, Debug)]
pub struct MerkleTree {
    root: Sha256Hash,
    tree: Vec<Sha256Hash>,
    /// In full mode, one bit per block: its hash in the tree is verified.
    block_verified: Bitfield,
    num_blocks: usize,
    /// log2 of blocks per piece: the number of tree levels within a piece.
    blocks_per_piece_log: u32,
    mode: Mode,
}

impl Default for MerkleTree {
    fn default() -> MerkleTree {
        MerkleTree {
            root: Sha256Hash::default(),
            tree: Vec::new(),
            block_verified: Bitfield::default(),
            num_blocks: 0,
            blocks_per_piece_log: 0,
            mode: Mode::Uninitialized,
        }
    }
}

impl MerkleTree {
    /// A tree over `num_blocks` blocks of a file with `root`; `blocks_per_piece` is a power of
    /// two.
    pub fn new(num_blocks: usize, blocks_per_piece: usize, root: Sha256Hash) -> MerkleTree {
        debug_assert!(blocks_per_piece.is_power_of_two());
        MerkleTree {
            root,
            tree: Vec::new(),
            block_verified: Bitfield::default(),
            num_blocks,
            blocks_per_piece_log: blocks_per_piece.trailing_zeros(),
            mode: Mode::Empty,
        }
    }

    pub fn root(&self) -> Sha256Hash {
        self.root
    }

    pub fn is_initialized(&self) -> bool {
        self.mode != Mode::Uninitialized
    }

    /// Nodes in the full padded tree.
    pub fn size(&self) -> usize {
        merkle_num_nodes(merkle_num_leafs(self.num_blocks))
    }

    pub fn num_blocks(&self) -> usize {
        self.num_blocks
    }

    fn blocks_per_piece(&self) -> usize {
        1 << self.blocks_per_piece_log
    }

    fn piece_levels(&self) -> usize {
        self.blocks_per_piece_log as usize
    }

    fn num_pieces(&self) -> usize {
        (self.num_blocks + self.blocks_per_piece() - 1) >> self.blocks_per_piece_log
    }

    fn num_leafs(&self) -> usize {
        merkle_num_leafs(self.num_blocks)
    }

    fn block_layer_start(&self) -> usize {
        merkle_first_leaf(self.num_leafs())
    }

    fn piece_layer_start(&self) -> usize {
        merkle_first_leaf(merkle_num_leafs(self.num_pieces()))
    }

    fn piece_pad(&self) -> Sha256Hash {
        merkle_pad(self.blocks_per_piece(), 1)
    }

    /// Takes the hashes of `verified` (one bit per block) that are present in the tree.
    fn load_verified_bits(&mut self, verified: &Bitfield) {
        debug_assert_eq!(self.block_verified.len(), self.num_blocks);
        let first = self.block_layer_start();
        for i in 0..verified.len().min(self.num_blocks) {
            if verified.get(i) && self.has_node(first + i) {
                self.block_verified.set(i, true);
            }
        }
    }

    /// Loads a full tree (resume data), keeping only the hashes that validate against the root.
    pub fn load_tree(&mut self, t: &[Sha256Hash], verified: &Bitfield) {
        if t.is_empty() || self.root != t[0] || self.size() != t.len() {
            return;
        }
        if t.len() == 1 {
            // just the root, which has its own representation
            self.optimize_storage();
            return;
        }
        self.allocate_full();
        let root = self.root;
        merkle_validate_copy(t, &mut self.tree, &root, &mut self.block_verified);
        self.load_verified_bits(verified);
        self.optimize_storage();
        self.optimize_storage_piece_layer();
    }

    fn clear(&mut self) {
        self.tree = Vec::new();
        self.block_verified = Bitfield::default();
        self.mode = Mode::Empty;
    }

    /// Loads the hashes `t` of the nodes set in `mask` (one bit per node of the full tree).
    pub fn load_sparse_tree(&mut self, t: &[Sha256Hash], mask: &Bitfield, verified: &Bitfield) {
        debug_assert_eq!(mask.len(), self.size());
        if mask.len() != self.size() {
            return;
        }
        let set = |range: std::ops::Range<usize>| range.clone().all(|i| mask.get(i));
        let count = |end: usize| (0..end).filter(|&i| mask.get(i)).count();

        let first_block = self.block_layer_start();
        let end_block = first_block + self.num_blocks;
        // the mask covers every block: block-layer mode, validated
        if set(first_block..end_block) {
            let block_index = count(first_block);
            if t.len() < block_index + self.num_blocks {
                return self.clear();
            }
            self.tree = t[block_index..block_index + self.num_blocks].to_vec();
            self.block_verified = Bitfield::default();
            self.mode = Mode::BlockLayer;
            if merkle_root(&self.tree, &Sha256Hash::default()) != self.root {
                self.clear();
            }
            return;
        }

        // the mask covers every piece and nothing below: piece-layer mode, validated
        if self.blocks_per_piece_log > 0 {
            let first_piece = self.piece_layer_start();
            let end_piece = first_piece + self.num_pieces();
            if set(first_piece..end_piece) && (end_piece..mask.len()).all(|i| !mask.get(i)) {
                let piece_index = count(first_piece);
                if t.len() < piece_index + self.num_pieces() {
                    return self.clear();
                }
                self.tree = t[piece_index..piece_index + self.num_pieces()].to_vec();
                self.block_verified = Bitfield::default();
                self.mode = Mode::PieceLayer;
                if merkle_root(&self.tree, &self.piece_pad()) != self.root {
                    self.clear();
                }
                return;
            }
        }

        if t.is_empty() || mask.none_set() {
            return self.clear();
        }

        self.allocate_full();
        let mut cursor = 0;
        for i in 0..mask.len() {
            if !mask.get(i) {
                continue;
            }
            if cursor >= t.len() {
                break;
            }
            self.tree[i] = t[cursor];
            cursor += 1;
        }
        merkle_fill_partial_tree(&mut self.tree);

        // nothing could be validated against the root: the tree is effectively empty
        if self.tree[0] != self.root {
            return self.clear();
        }
        self.load_verified_bits(verified);
        self.optimize_storage();
    }

    /// The piece layer hashes (empty if not known).
    pub fn get_piece_layer(&self) -> Vec<Sha256Hash> {
        match self.mode {
            Mode::Uninitialized | Mode::Empty => Vec::new(),
            Mode::Full => {
                let start = self.piece_layer_start();
                self.tree[start..start + self.num_pieces()].to_vec()
            }
            Mode::PieceLayer => self.tree.clone(),
            Mode::BlockLayer => {
                let bpp = self.blocks_per_piece();
                let mut scratch = Vec::new();
                self.tree
                    .chunks(bpp)
                    .map(|leafs| merkle_root_scratch(leafs, bpp, Sha256Hash::default(), &mut scratch))
                    .collect()
            }
        }
    }

    /// Loads the piece layer from a `.torrent`'s `piece layers`; false if it does not hash to
    /// the root.
    pub fn load_piece_layer(&mut self, piece_layer: &[u8]) -> bool {
        if self.mode == Mode::BlockLayer {
            return true;
        }
        let npieces = self.num_pieces();
        if piece_layer.len() != npieces * Sha256Hash::LEN {
            return false;
        }
        if self.num_blocks == 1 {
            // a tree that is only its root
            if piece_layer[..Sha256Hash::LEN] != self.root.0 {
                return false;
            }
            self.clear();
            return true;
        }
        let pieces: Vec<Sha256Hash> =
            piece_layer.chunks_exact(Sha256Hash::LEN).map(|c| Sha256Hash(c.try_into().unwrap())).collect();
        if merkle_root(&pieces, &self.piece_pad()) != self.root {
            return false;
        }
        // with one block per piece, the piece layer is the block layer
        self.mode = if self.blocks_per_piece_log == 0 { Mode::BlockLayer } else { Mode::PieceLayer };
        self.tree = pieces;
        self.block_verified = Bitfield::default();
        true
    }

    /// Inserts `hashes`, a range of one layer starting at node `dest_start_idx`, proven by
    /// `uncle_hashes` (the sibling hashes up to a known node). Returns `None` if they do not
    /// validate. The pieces of blocks received earlier that can now be checked come back in the
    /// result, with `file_piece_offset` (the file's first piece) added.
    ///
    /// ```text
    ///                         R
    ///            _                          _
    ///     _              1           _              _
    ///  _     _        _     _     T     0        _     _
    /// _   _ _   _    _   _ _   _ T   T _   _    _   _ _   _
    ///                            ^ dest_start_idx
    /// ```
    pub fn add_hashes(
        &mut self,
        dest_start_idx: usize,
        file_piece_offset: u32,
        hashes: &[Sha256Hash],
        uncle_hashes: &[Sha256Hash],
    ) -> Option<AddHashesResult> {
        let mut ret = AddHashesResult::default();

        // the whole tree is known; any pieces were reported when it was completed
        if self.mode == Mode::BlockLayer {
            return Some(ret);
        }
        // the hashes must be an aligned run within one layer (libtorrent asserts what its
        // callers guarantee; a hash request is validated before it gets here)
        let leaf_count = merkle_num_leafs(hashes.len());
        let layer = merkle_get_layer(dest_start_idx);
        if hashes.is_empty()
            || dest_start_idx + leaf_count > self.size()
            || merkle_get_layer(dest_start_idx + leaf_count - 1) != layer
            || merkle_get_layer_offset(dest_start_idx) % leaf_count != 0
        {
            return None;
        }

        let mut tree = vec![Sha256Hash::default(); merkle_num_nodes(leaf_count)];
        let tree_len = tree.len();
        tree[tree_len - leaf_count..tree_len - leaf_count + hashes.len()].copy_from_slice(hashes);

        // the end of a file may need its leaf layer padded
        if leaf_count > hashes.len() {
            let leaf_layer_size = self.num_leafs();
            // the uncle hashes lead to the root, which tells how many layers down this is
            let insert_layer_size = u32::try_from(uncle_hashes.len())
                .ok()
                .and_then(|n| leaf_count.checked_shl(n))
                .unwrap_or(usize::MAX);
            if leaf_layer_size != insert_layer_size {
                let pad_hash = merkle_pad(leaf_layer_size, insert_layer_size);
                for h in &mut tree[tree_len - leaf_count + hashes.len()..] {
                    *h = pad_hash;
                }
            }
        }

        merkle_fill_tree(&mut tree, leaf_count);

        let base_num_layers = merkle_num_layers(leaf_count);
        // the node where the subtree's root goes, which the uncle hashes prove
        let insert_root_idx = dest_start_idx >> base_num_layers;
        // Check the proof against the storage as it is before expanding to the full tree: for a
        // large file that is gigabytes, and any peer can send hashes (SEC-09).
        if !self.proof_reaches_known_node(insert_root_idx, tree[0], uncle_hashes) {
            return None;
        }
        self.allocate_full();
        if !merkle_validate_and_insert_proofs(&mut self.tree, insert_root_idx, &tree[0], uncle_hashes) {
            return None;
        }

        let first_leaf = merkle_first_leaf(self.num_leafs());
        let mut source_cursor = tree_len - leaf_count;
        let mut dest_cursor = dest_start_idx;
        let base = self.piece_levels();
        let mut layer_size = leaf_count;
        while layer_size != 0 {
            for i in 0..layer_size {
                let dst_idx = dest_cursor + i;
                let src_idx = source_cursor + i;
                if self.has_node(dst_idx) {
                    if self.tree[dst_idx] != tree[src_idx] {
                        // only block hashes can differ: inner nodes are set once verified
                        debug_assert!(dst_idx >= first_leaf);
                        let pos = dst_idx - first_leaf;
                        let piece = (pos >> self.blocks_per_piece_log) as u32 + file_piece_offset;
                        let block = pos & (self.blocks_per_piece() - 1);
                        match ret.failed.last_mut() {
                            Some((p, blocks)) if *p == piece => blocks.push(block),
                            _ => ret.failed.push((piece, vec![block])),
                        }
                        // reported once; clear it so it is not reported again
                        self.tree[dst_idx].clear();
                    } else if dst_idx >= first_leaf {
                        // pieces of a single block; the common case is handled below
                        let piece =
                            ((dst_idx - first_leaf) >> self.blocks_per_piece_log) as u32 + file_piece_offset;
                        if ret.passed.last() != Some(&piece) {
                            ret.passed.push(piece);
                        }
                    }
                }
                if dst_idx >= first_leaf && dst_idx - first_leaf < self.num_blocks {
                    self.block_verified.set(dst_idx - first_leaf, true);
                }
                self.tree[dst_idx] = tree[src_idx];
            }
            if layer_size == 1 {
                break;
            }
            dest_cursor = merkle_get_parent(dest_cursor);
            source_cursor = merkle_get_parent(source_cursor);
            layer_size /= 2;
        }

        // newly learned piece hashes may validate block hashes received earlier
        let first_piece_idx = self.piece_layer_start();
        let num_pieces = self.num_pieces();
        if base != 0 && dest_start_idx >= first_piece_idx && dest_start_idx < first_piece_idx + num_pieces {
            let blocks_in_piece = 1 << base;
            for (i, hash) in hashes.iter().enumerate() {
                let piece = dest_start_idx + i;
                if piece - first_piece_idx >= num_pieces {
                    break;
                }
                let block_idx = merkle_get_first_child_depth(piece, base);
                let block_end_idx = (block_idx + blocks_in_piece).min(first_leaf + self.num_blocks);
                if self.tree[block_idx..block_end_idx].iter().any(Sha256Hash::is_zero) {
                    continue;
                }
                merkle_fill_tree_from(&mut self.tree, blocks_in_piece, block_idx);
                let piece_index = (piece - first_piece_idx) as u32 + file_piece_offset;
                if self.tree[piece] != *hash {
                    merkle_clear_tree(&mut self.tree, blocks_in_piece, block_idx);
                    // put the right hash back
                    self.tree[piece] = *hash;
                    // no block list: which block failed is unknown, so the block hashes get
                    // requested
                    ret.failed.push((piece_index, Vec::new()));
                } else {
                    ret.passed.push(piece_index);
                    let leafs_start = block_idx - self.block_layer_start();
                    let leafs_end = self.num_blocks.min(leafs_start + blocks_in_piece);
                    for k in leafs_start..leafs_end {
                        self.block_verified.set(k, true);
                    }
                }
            }
        }

        self.optimize_storage();
        Some(ret)
    }

    /// Sets the hash of block `block_index` (from the data) and verifies what it can. Returns
    /// the outcome with the range of blocks it covers.
    pub fn set_block(&mut self, block_index: usize, h: &Sha256Hash) -> (SetBlockResult, usize, usize) {
        debug_assert!(block_index < self.num_blocks);
        let first_leaf = merkle_first_leaf(self.num_leafs());
        let block_tree_index = first_leaf + block_index;

        if self.blocks_verified(block_index, 1) {
            // known already: compare
            let result = if self.compare_node(block_tree_index, h) {
                SetBlockResult::Ok
            } else {
                SetBlockResult::BlockHashFailed
            };
            return (result, block_index, 1);
        }

        self.allocate_full();
        self.tree[block_tree_index] = *h;

        // rather than hashing nodes that cannot be verified, find the largest subtree of known
        // leaves and check that its root is known
        let (leafs_start, leafs_size, root_index) =
            merkle_find_known_subtree(&self.tree, block_index, self.num_blocks);
        if self.tree[root_index].is_zero() {
            return (SetBlockResult::Unknown, leafs_start, leafs_size);
        }

        // merkle_fill_tree_from overwrites the root
        let root = self.tree[root_index];
        merkle_fill_tree_from(&mut self.tree, leafs_size, first_leaf + leafs_start);
        if root != self.tree[root_index] {
            // the subtree failed: clear its block hashes and inner nodes
            merkle_clear_tree(&mut self.tree, leafs_size, first_leaf + leafs_start);
            self.tree[root_index] = root;
            return (SetBlockResult::HashFailed, leafs_start, leafs_size);
        }

        let leafs_end = self.num_blocks.min(leafs_start + leafs_size);
        for i in leafs_start..leafs_end {
            self.block_verified.set(i, true);
        }

        // optimizing storage is costly; only try when it may help
        if block_index == self.num_blocks - 1 || !self.tree[block_tree_index + 1].is_zero() {
            self.optimize_storage();
        }
        (SetBlockResult::Ok, leafs_start, leafs_size)
    }

    /// Whether `node`, placed at `idx` and hashed up with `uncle_hashes`, meets the first known
    /// node on its way to the root with the right hash: what [`merkle_validate_and_insert_proofs`]
    /// decides, without writing (or allocating) anything. Only the first known node is computed.
    fn proof_reaches_known_node(
        &self,
        mut idx: usize,
        node: Sha256Hash,
        uncle_hashes: &[Sha256Hash],
    ) -> bool {
        if self.has_node(idx) {
            return self.compare_node(idx, &node);
        }
        let mut hash = node;
        for proof in uncle_hashes {
            let sibling = merkle_get_sibling(idx);
            hash =
                if sibling < idx { merkle_hash_pair(proof, &hash) } else { merkle_hash_pair(&hash, proof) };
            idx = merkle_get_parent(idx);
            if self.has_node(idx) {
                return self.compare_node(idx, &hash);
            }
        }
        false
    }

    pub fn has_node(&self, idx: usize) -> bool {
        debug_assert!(idx < self.size());
        match self.mode {
            Mode::Uninitialized => false,
            Mode::Empty => idx == 0,
            Mode::Full => !self.tree[idx].is_zero(),
            Mode::PieceLayer => idx < merkle_get_first_child(self.piece_layer_start()),
            Mode::BlockLayer => idx < self.block_layer_start() + self.num_blocks,
        }
    }

    pub fn compare_node(&self, idx: usize, h: &Sha256Hash) -> bool {
        match self.mode {
            Mode::Uninitialized => h.is_zero(),
            Mode::Empty => {
                if idx == 0 {
                    self.root == *h
                } else {
                    h.is_zero()
                }
            }
            Mode::Full => self.tree[idx] == *h,
            Mode::PieceLayer => {
                let first = self.piece_layer_start();
                let pieces_end = first + self.num_pieces();
                let end = first + merkle_num_leafs(self.num_pieces());
                if idx >= end {
                    h.is_zero()
                } else if idx >= pieces_end {
                    *h == self.piece_pad()
                } else if idx >= first {
                    self.tree[idx - first] == *h
                } else {
                    self.get(idx) == *h
                }
            }
            Mode::BlockLayer => {
                let first = self.block_layer_start();
                let end = first + self.num_blocks;
                if idx >= end {
                    h.is_zero()
                } else if idx >= first {
                    self.tree[idx - first] == *h
                } else {
                    self.get(idx) == *h
                }
            }
        }
    }

    /// The hash of node `idx` (zero if unknown), computed when the storage does not hold it.
    pub fn get(&self, idx: usize) -> Sha256Hash {
        let mut scratch = Vec::new();
        self.get_impl(idx, &mut scratch)
    }

    fn get_impl(&self, mut idx: usize, scratch_space: &mut Vec<Sha256Hash>) -> Sha256Hash {
        match self.mode {
            Mode::Uninitialized => Sha256Hash::default(),
            Mode::Empty => {
                if idx == 0 {
                    self.root
                } else {
                    Sha256Hash::default()
                }
            }
            Mode::Full => self.tree[idx],
            Mode::PieceLayer | Mode::BlockLayer => {
                let piece_mode = self.mode == Mode::PieceLayer;
                let start = if piece_mode { self.piece_layer_start() } else { self.block_layer_start() };
                if piece_mode && idx >= merkle_get_first_child(start) {
                    return Sha256Hash::default();
                }
                let mut layer_size = 1;
                while idx < start {
                    idx = merkle_get_first_child(idx);
                    layer_size *= 2;
                }
                idx -= start;
                if idx >= self.tree.len() {
                    let leafs = if piece_mode { layer_size << self.blocks_per_piece_log } else { layer_size };
                    return merkle_pad(leafs, 1);
                }
                let pad_hash = if piece_mode { self.piece_pad() } else { Sha256Hash::default() };
                let layer = &self.tree[idx..idx + (self.tree.len() - idx).min(layer_size)];
                merkle_root_scratch(layer, layer_size, pad_hash, scratch_space)
            }
        }
    }

    /// The full padded tree (unknown nodes zero).
    pub fn build_vector(&self) -> Vec<Sha256Hash> {
        if self.mode == Mode::Uninitialized {
            return Vec::new();
        }
        let mut ret = vec![Sha256Hash::default(); self.size()];
        match self.mode {
            Mode::Uninitialized | Mode::Empty => {}
            Mode::Full => ret.copy_from_slice(&self.tree),
            Mode::PieceLayer => {
                let piece_layer_size = merkle_num_leafs(self.num_pieces());
                let pad_hash = self.piece_pad();
                let start = merkle_first_leaf(piece_layer_size);
                ret[start..start + self.tree.len()].copy_from_slice(&self.tree);
                for h in &mut ret[start + self.tree.len()..start + piece_layer_size] {
                    *h = pad_hash;
                }
                merkle_fill_tree(&mut ret[..merkle_num_nodes(piece_layer_size)], piece_layer_size);
            }
            Mode::BlockLayer => {
                let num_leafs = self.num_leafs();
                let start = merkle_first_leaf(num_leafs);
                ret[start..start + self.tree.len()].copy_from_slice(&self.tree);
                merkle_fill_tree(&mut ret, num_leafs);
            }
        }
        ret[0] = self.root;
        ret
    }

    /// The known nodes and a mask of which nodes of the full tree they are (resume data).
    pub fn build_sparse_vector(&self) -> (Vec<Sha256Hash>, Bitfield) {
        if self.mode == Mode::Uninitialized {
            return (Vec::new(), Bitfield::default());
        }
        let mut mask = Bitfield::new(self.size(), false);
        let mut ret = Vec::new();
        match self.mode {
            Mode::Uninitialized | Mode::Empty => {}
            Mode::Full => {
                for (i, h) in self.tree.iter().enumerate() {
                    if !h.is_zero() {
                        ret.push(*h);
                        mask.set(i, true);
                    }
                }
            }
            Mode::PieceLayer | Mode::BlockLayer => {
                let start = if self.mode == Mode::PieceLayer {
                    merkle_first_leaf(merkle_num_leafs(self.num_pieces()))
                } else {
                    merkle_first_leaf(self.num_leafs())
                };
                for i in start..start + self.tree.len() {
                    mask.set(i, true);
                }
                ret = self.tree.clone();
            }
        }
        (ret, mask)
    }

    /// One bit per block: its hash is verified. With one block per piece the piece layer (and a
    /// one-block tree's root) covers the blocks too.
    pub fn verified_leafs(&self) -> Bitfield {
        match self.mode {
            Mode::Uninitialized | Mode::Empty => Bitfield::new(self.num_blocks, self.num_blocks == 1),
            Mode::PieceLayer => Bitfield::new(self.num_blocks, self.piece_levels() == 0),
            Mode::BlockLayer => Bitfield::new(self.num_blocks, true),
            Mode::Full => self.block_verified.clone(),
        }
    }

    /// True if the whole tree is known and verified.
    pub fn is_complete(&self) -> bool {
        match self.mode {
            Mode::Uninitialized => false,
            Mode::Empty => self.num_blocks == 1,
            Mode::PieceLayer => self.piece_levels() == 0,
            Mode::BlockLayer => true,
            Mode::Full => !self.block_verified.is_empty() && self.block_verified.all_set(),
        }
    }

    /// True if the hashes of blocks `[block_idx, block_idx + num_blocks)` are verified.
    pub fn blocks_verified(&self, block_idx: usize, num_blocks: usize) -> bool {
        debug_assert!(num_blocks > 0 && block_idx + num_blocks <= self.num_blocks);
        match self.mode {
            Mode::Uninitialized => false,
            Mode::Empty => self.num_blocks == 1,
            Mode::PieceLayer => self.piece_levels() == 0,
            Mode::BlockLayer => true,
            Mode::Full => (block_idx..block_idx + num_blocks).all(|i| self.block_verified.get(i)),
        }
    }

    fn allocate_full(&mut self) {
        if self.mode == Mode::Full {
            return;
        }
        // a complete tree is never expanded again
        debug_assert!(self.mode != Mode::BlockLayer);
        self.tree = self.build_vector();
        self.mode = Mode::Full;
        self.block_verified.resize(self.num_blocks, false);
    }

    fn optimize_storage(&mut self) {
        if self.mode != Mode::Full {
            return;
        }
        if self.num_blocks == 1 {
            // only a root: no storage needed
            self.clear();
            return;
        }
        if self.block_verified.all_set() {
            let start = self.block_layer_start();
            self.tree = self.tree[start..start + self.num_blocks].to_vec();
            self.mode = Mode::BlockLayer;
            self.block_verified = Bitfield::default();
        }
    }

    fn optimize_storage_piece_layer(&mut self) {
        if self.mode != Mode::Full {
            return;
        }
        // any block hash keeps the tree full, or it would be lost
        let piece_layer_size = merkle_num_leafs(self.num_pieces());
        if self.blocks_per_piece_log > 0
            && merkle_validate_single_layer(&self.tree[..merkle_num_nodes(piece_layer_size)])
            && self.tree[self.block_layer_start()..].iter().all(Sha256Hash::is_zero)
        {
            let start = self.piece_layer_start();
            self.tree = self.tree[start..start + self.num_pieces()].to_vec();
            self.mode = Mode::PieceLayer;
            self.block_verified = Bitfield::default();
        }
    }

    /// The hashes of a hash request: `count` nodes from `index` in layer `base` (0 = blocks),
    /// then the uncle hashes for `proof_layers` layers above them (those not computable from
    /// the hashes themselves). Empty if any of them is unknown. The request must be valid
    /// (`hash_picker::validate_hash_request`).
    pub fn get_hashes(
        &self,
        base: usize,
        index: usize,
        count: usize,
        proof_layers: usize,
    ) -> Vec<Sha256Hash> {
        let num_layers = merkle_num_layers(self.num_leafs());
        if base > num_layers {
            return Vec::new();
        }
        let base_layer_idx = num_layers - base;
        let layer_start_idx = merkle_to_flat_index(base_layer_idx, index);

        let mut ret = Vec::with_capacity(count);
        let mut scratch_space = Vec::new();
        if base == 0 && self.mode == Mode::BlockLayer {
            let blocks_end = (index + count).min(self.num_blocks);
            ret.extend_from_slice(&self.tree[index.min(blocks_end)..blocks_end]);
            // the rest is padding
            ret.resize(count, Sha256Hash::default());
        } else {
            for i in layer_start_idx..layer_start_idx + count {
                if i >= self.size() {
                    return Vec::new();
                }
                // pad hashes are zero and do not fail the request
                if (base != 0 || i < self.num_blocks + layer_start_idx - index) && !self.has_node(i) {
                    return Vec::new();
                }
                ret.push(self.get_impl(i, &mut scratch_space));
            }
        }

        // proof_idx climbs one layer per iteration, starting at layer_start_idx (i == 0). Its
        // sibling at a layer is implied by the `count` hashes gathered above while still inside
        // the subtree they form on their own (`implied_layers` tall); from there up to
        // `proof_layers` (inclusive) each sibling is sent. `count == 1` forms no subtree, so
        // even the leaf's own sibling (i == 0) is sent
        let implied_layers = merkle_num_layers(merkle_num_leafs(count));
        let mut proof_idx = layer_start_idx;
        for _ in 0..implied_layers {
            // the requester set proof_layers too high (libtorrent asserts)
            if proof_idx == 0 {
                return Vec::new();
            }
            proof_idx = merkle_get_parent(proof_idx);
        }
        for i in implied_layers..=proof_layers {
            if proof_idx == 0 {
                return Vec::new();
            }
            let sibling = merkle_get_sibling(proof_idx);
            // a sibling past the last real block is a padding leaf, always the all-zero leaf
            // pad and never a stored node; padding only exists at the leaf layer
            let sibling_is_real = i > 0 || base != 0 || sibling < self.num_blocks + layer_start_idx - index;
            if !self.has_node(proof_idx) || (sibling_is_real && !self.has_node(sibling)) {
                return Vec::new();
            }
            ret.push(if sibling_is_real {
                self.get_impl(sibling, &mut scratch_space)
            } else {
                Sha256Hash::default()
            });
            proof_idx = merkle_get_parent(proof_idx);
        }
        ret
    }

    /// libtorrent's `check_invariant`, run by the tests after each change.
    #[cfg(test)]
    pub(crate) fn check_invariant(&self) {
        if self.num_blocks == 1 && self.mode != Mode::Uninitialized {
            // a one-block tree is verified by its root
            assert!(self.blocks_verified(0, 1));
        }
        match self.mode {
            Mode::Uninitialized | Mode::Empty => {
                assert!(self.tree.is_empty());
                assert!(self.block_verified.is_empty());
            }
            Mode::Full => {
                assert_eq!(self.tree[0], self.root);
                assert_eq!(self.block_verified.len(), self.num_blocks);
                let num_leafs = self.num_leafs();
                if self.tree.len() == 1 {
                    return;
                }
                // above the blocks, every set hash has a set sibling and validates
                let mut i = 1;
                while i < self.tree.len() - num_leafs {
                    if self.tree[i].is_zero() {
                        assert!(self.tree[i + 1].is_zero());
                    } else {
                        assert!(merkle_validate_node(
                            &self.tree[i],
                            &self.tree[i + 1],
                            &self.tree[merkle_get_parent(i)]
                        ));
                    }
                    i += 2;
                }
                // verified blocks are correct
                let first_block = merkle_first_leaf(num_leafs);
                let mut i = first_block;
                let mut b = 0;
                while i < first_block + self.num_blocks {
                    if i + 1 == first_block + self.num_blocks {
                        if self.block_verified.get(b) {
                            assert!(self.has_node(i));
                            let parent = merkle_get_parent(i);
                            assert!(merkle_validate_node(
                                &self.tree[i],
                                &Sha256Hash::default(),
                                &self.tree[parent]
                            ));
                        }
                    } else {
                        assert_eq!(self.block_verified.get(b), self.block_verified.get(b + 1));
                        if self.block_verified.get(b) {
                            assert!(self.has_node(i) && self.has_node(i + 1));
                            let parent = merkle_get_parent(i);
                            assert!(merkle_validate_node(
                                &self.tree[i],
                                &self.tree[i + 1],
                                &self.tree[parent]
                            ));
                        }
                    }
                    i += 2;
                    b += 2;
                }
                // padding is zero
                assert!(self.tree[first_block + self.num_blocks..].iter().all(Sha256Hash::is_zero));
            }
            Mode::PieceLayer => {
                assert_eq!(merkle_root(&self.tree, &self.piece_pad()), self.root);
                assert!(self.block_verified.is_empty());
            }
            Mode::BlockLayer => {
                assert_eq!(merkle_root(&self.tree, &Sha256Hash::default()), self.root);
                assert!(self.block_verified.is_empty());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::random::crypto_random_bytes;
    use std::sync::LazyLock;

    const NUM_BLOCKS: usize = 259;
    static F: LazyLock<Vec<Sha256Hash>> = LazyLock::new(|| build_tree(NUM_BLOCKS));

    fn f() -> &'static [Sha256Hash] {
        &F
    }

    fn num_leafs() -> usize {
        merkle_num_leafs(NUM_BLOCKS)
    }

    fn num_nodes() -> usize {
        merkle_num_nodes(num_leafs())
    }

    fn num_pad_leafs() -> usize {
        num_leafs() - NUM_BLOCKS
    }

    fn empty_verified() -> Bitfield {
        Bitfield::new(NUM_BLOCKS, false)
    }

    fn rand_sha256() -> Sha256Hash {
        let mut h = Sha256Hash::default();
        crypto_random_bytes(&mut h.0);
        h
    }

    fn build_proof(tree: &[Sha256Hash], mut target: usize, end: usize) -> Vec<Sha256Hash> {
        let mut ret = Vec::new();
        while target > end {
            ret.push(tree[merkle_get_sibling(target)]);
            target = merkle_get_parent(target);
        }
        ret
    }

    fn corrupt(hashes: &[Sha256Hash]) -> Vec<Sha256Hash> {
        let mut ret = hashes.to_vec();
        let n = ret.len();
        ret[146542934 % n].0[2] ^= 0x26;
        ret
    }

    fn all_set(n: usize) -> Bitfield {
        Bitfield::new(n, true)
    }

    fn none_set(n: usize) -> Bitfield {
        Bitfield::new(n, false)
    }

    fn set_range(mut bits: Bitfield, start: usize, count: usize) -> Bitfield {
        for i in start..start + count {
            bits.set(i, true);
        }
        bits
    }

    fn range(first: usize, count: usize) -> &'static [Sha256Hash] {
        &f()[first..first + count]
    }

    fn mask_of(len: usize, set: impl IntoIterator<Item = usize>) -> Bitfield {
        let mut m = Bitfield::new(len, false);
        for i in set {
            m.set(i, true);
        }
        m
    }

    #[test]
    fn load_tree() {
        // full tree, valid root
        let mut t = MerkleTree::new(NUM_BLOCKS, 1, f()[0]);
        t.load_tree(f(), &empty_verified());
        t.check_invariant();
        for i in 0..num_nodes() - num_pad_leafs() {
            assert!(t.has_node(i));
            assert!(t.compare_node(i, &f()[i]));
        }
        for i in num_nodes() - num_pad_leafs()..num_nodes() {
            assert!(!t.has_node(i));
            assert!(t.compare_node(i, &f()[i]));
        }

        // mismatching root
        let mut t = MerkleTree::new(NUM_BLOCKS, 1, Sha256Hash(*b"01234567890123456789012345678901"));
        t.load_tree(f(), &empty_verified());
        assert!(t.has_node(0));
        assert!((1..num_nodes()).all(|i| !t.has_node(i)));

        // mismatching size
        let mut t = MerkleTree::new(NUM_BLOCKS, 1, f()[0]);
        t.load_tree(&f()[..f().len() - 1], &empty_verified());
        assert!(t.has_node(0));
        assert!((1..num_nodes()).all(|i| !t.has_node(i)));
    }

    #[test]
    fn load_sparse_tree() {
        // full tree, valid root
        let mut t = MerkleTree::new(NUM_BLOCKS, 1, f()[0]);
        t.load_sparse_tree(f(), &all_set(f().len()), &empty_verified());
        t.check_invariant();
        for i in 0..num_nodes() - num_pad_leafs() {
            assert!(t.has_node(i));
            assert!(t.compare_node(i, &f()[i]));
        }
        for i in num_nodes() - num_pad_leafs()..num_nodes() {
            assert!(!t.has_node(i));
            assert!(t.compare_node(i, &f()[i]));
        }

        // mismatching root
        let mut t = MerkleTree::new(NUM_BLOCKS, 1, Sha256Hash(*b"01234567890123456789012345678901"));
        t.load_sparse_tree(&f()[1..3], &mask_of(f().len(), [1, 2]), &empty_verified());
        assert!(t.has_node(0));
        assert!((1..num_nodes()).all(|i| !t.has_node(i)));

        // block layer
        let mut t = MerkleTree::new(NUM_BLOCKS, 1, f()[0]);
        let first_block = merkle_first_leaf(num_leafs());
        let mask = mask_of(f().len(), first_block..first_block + NUM_BLOCKS);
        t.load_sparse_tree(range(first_block, NUM_BLOCKS), &mask, &empty_verified());
        t.check_invariant();
        for i in 0..num_nodes() - num_pad_leafs() {
            assert!(t.has_node(i));
            assert!(t.compare_node(i, &f()[i]));
        }
        for i in num_nodes() - num_pad_leafs()..num_nodes() {
            assert!(!t.has_node(i));
            assert!(t.compare_node(i, &f()[i]));
        }

        // piece layer
        let num_pieces = NUM_BLOCKS.div_ceil(2);
        let first_piece = merkle_first_leaf(merkle_num_leafs(num_pieces));
        let mut t = MerkleTree::new(NUM_BLOCKS, 2, f()[0]);
        let mask = mask_of(f().len(), first_piece..first_piece + num_pieces);
        t.load_sparse_tree(range(first_piece, num_pieces), &mask, &empty_verified());
        t.check_invariant();
        let end_piece_layer = first_piece + merkle_num_leafs(num_pieces);
        for i in 0..end_piece_layer {
            assert!(t.has_node(i));
            assert!(t.compare_node(i, &f()[i]));
        }
        assert!((end_piece_layer..num_nodes()).all(|i| !t.has_node(i)));
    }

    fn roundtrip(t: &MerkleTree, block_count: usize, blocks_per_piece: usize) {
        let (tree, mask) = t.build_sparse_vector();
        let mut t2 = MerkleTree::new(block_count, blocks_per_piece, f()[0]);
        t2.load_sparse_tree(&tree, &mask, &empty_verified());
        t2.check_invariant();
        assert_eq!(t.build_vector(), t2.build_vector());
        for i in 0..t.size() {
            assert_eq!(t.get(i), t2.get(i));
            assert_eq!(t.has_node(i), t2.has_node(i));
            if !t.has_node(i) {
                assert!(t.get(i).is_zero());
            }
            if !t2.has_node(i) {
                assert!(t2.get(i).is_zero());
            }
            assert!(t.compare_node(i, &t2.get(i)));
            assert!(t2.compare_node(i, &t.get(i)));
        }
    }

    #[test]
    fn roundtrips() {
        // empty tree
        roundtrip(&MerkleTree::new(NUM_BLOCKS, 1, f()[0]), NUM_BLOCKS, 1);

        // full tree
        let mut t = MerkleTree::new(NUM_BLOCKS, 1, f()[0]);
        t.load_tree(f(), &empty_verified());
        roundtrip(&t, NUM_BLOCKS, 1);

        // piece layer tree
        let mut t = MerkleTree::new(NUM_BLOCKS, 2, f()[0]);
        let mut sparse = f().to_vec();
        for h in &mut sparse[f().len() / 2..] {
            h.clear();
        }
        t.load_tree(&sparse, &empty_verified());
        t.check_invariant();
        roundtrip(&t, NUM_BLOCKS, 2);

        // partial trees
        for modulus in [3, 4] {
            let mut t = MerkleTree::new(NUM_BLOCKS, 2, f()[0]);
            let mut sparse = f().to_vec();
            for (i, h) in sparse.iter_mut().enumerate().skip(f().len() / 4) {
                if i % modulus == 0 {
                    h.clear();
                }
            }
            t.load_tree(&sparse, &empty_verified());
            t.check_invariant();
            roundtrip(&t, NUM_BLOCKS, 2);
        }

        // one block
        let mut t = MerkleTree::new(1, 256, f()[0]);
        t.load_tree(&f()[..1], &empty_verified());
        roundtrip(&t, 1, 256);

        // two blocks
        let mut t = MerkleTree::new(2, 256, f()[0]);
        t.load_tree(&f()[..3], &none_set(2));
        roundtrip(&t, 2, 256);

        // two blocks, partial
        let mut pf = f()[..3].to_vec();
        pf[2].clear();
        let mut t = MerkleTree::new(2, 256, f()[0]);
        t.load_tree(&pf, &none_set(2));
        roundtrip(&t, 2, 256);
    }

    #[test]
    fn small_tree() {
        // a single block with a large piece size
        let t = MerkleTree::new(1, 256, f()[0]);
        assert_eq!(t.build_vector(), vec![f()[0]]);
    }

    #[test]
    fn sparse_merkle_tree_block_layer() {
        let mut t = MerkleTree::new(NUM_BLOCKS, 2, f()[0]);
        t.load_tree(&f()[..t.size()], &empty_verified());
        for i in 0..t.size() {
            assert_eq!(t.get(i), f()[i]);
        }
    }

    #[test]
    fn get_piece_layer() {
        // 8 blocks per piece
        let mut t = MerkleTree::new(NUM_BLOCKS, 8, f()[0]);
        assert_eq!(t.verified_leafs(), none_set(NUM_BLOCKS));
        t.load_tree(&f()[..t.size()], &empty_verified());
        let num_pieces = NUM_BLOCKS.div_ceil(8);
        let piece_layer_start = merkle_first_leaf(merkle_num_leafs(num_pieces));
        let piece_layer = t.get_piece_layer();
        assert_eq!(t.verified_leafs(), all_set(NUM_BLOCKS));
        assert_eq!(piece_layer.len(), num_pieces);
        for (i, h) in piece_layer.iter().enumerate() {
            assert_eq!(t.get(piece_layer_start + i), *h);
        }
    }

    #[test]
    fn get_piece_layer_piece_layer_mode() {
        let mut t = MerkleTree::new(NUM_BLOCKS, 4, f()[0]);
        let num_pieces = NUM_BLOCKS.div_ceil(4);
        let bytes: Vec<u8> = range(127, num_pieces).iter().flat_map(|h| h.0).collect();
        assert!(t.load_piece_layer(&bytes));
        t.check_invariant();
        let piece_layer_start = merkle_first_leaf(merkle_num_leafs(num_pieces));
        let piece_layer = t.get_piece_layer();
        assert_eq!(t.verified_leafs(), none_set(NUM_BLOCKS));
        assert_eq!(piece_layer.len(), num_pieces);
        for (i, h) in piece_layer.iter().enumerate() {
            assert_eq!(t.get(piece_layer_start + i), *h);
        }
        // a wrong layer is refused
        let mut t = MerkleTree::new(NUM_BLOCKS, 4, f()[0]);
        assert!(!t.load_piece_layer(
            &corrupt(range(127, num_pieces)).iter().flat_map(|h| h.0).collect::<Vec<u8>>()
        ));
        assert!(!t.load_piece_layer(&bytes[..bytes.len() - 32]));
    }

    #[test]
    fn get_hashes() {
        let mut t = MerkleTree::new(NUM_BLOCKS, 2, f()[0]);
        t.load_tree(&f()[..t.size()], &empty_verified());

        // the leaf layer, all of it, all but the first, all but the last
        assert_eq!(t.get_hashes(0, 0, NUM_BLOCKS, 0), range(511, NUM_BLOCKS));
        assert_eq!(t.get_hashes(0, 1, NUM_BLOCKS - 1, 0), range(512, NUM_BLOCKS - 1));
        assert_eq!(t.get_hashes(0, 0, NUM_BLOCKS - 1, 0), range(511, NUM_BLOCKS - 1));

        // one layer up
        assert_eq!(t.get_hashes(1, 0, 256, 0), range(255, 256));

        // one layer up plus a proof layer: the sibling of the hashes' root (255 / 2 / 2 = 63)
        let h = t.get_hashes(1, 0, 4, 2);
        assert_eq!(&h[..4], range(255, 4));
        assert_eq!(&h[4..], &[f()[merkle_get_sibling(63)]]);

        // one layer up, hashes 2 to 10, 5 proof layers
        let h = t.get_hashes(1, 2, 8, 5);
        assert_eq!(&h[..8], range(255 + 2, 8));
        let start_proofs = merkle_get_parent(merkle_get_parent(merkle_get_parent(257)));
        let proofs = [
            f()[merkle_get_sibling(start_proofs)],
            f()[merkle_get_sibling(merkle_get_parent(start_proofs))],
            f()[merkle_get_sibling(merkle_get_parent(merkle_get_parent(start_proofs)))],
        ];
        assert_eq!(&h[8..], &proofs);

        // the full tree: no proofs
        assert_eq!(t.get_hashes(0, 0, 512, 8), range(511, 512));

        // the second half: one proof
        let h = t.get_hashes(0, 256, 256, 8);
        assert_eq!(&h[..256], range(511 + 256, 256));
        assert_eq!(&h[256..], &[f()[1]]);

        // the 3rd quarter: two proofs
        let h = t.get_hashes(0, 256, 128, 8);
        assert_eq!(&h[..128], range(511 + 256, 128));
        assert_eq!(&h[128..], &[f()[6], f()[1]]);

        // the 3rd quarter one layer up
        let h = t.get_hashes(1, 128, 64, 7);
        assert_eq!(&h[..64], range(255 + 128, 64));
        assert_eq!(&h[64..], &[f()[6], f()[1]]);

        // no proof hashes asked for
        assert_eq!(t.get_hashes(1, 128, 64, 0), range(255 + 128, 64));

        // a single leaf (count == 1) has no internal subtree of its own, so its immediate
        // sibling must be included as the first proof hash
        let h = t.get_hashes(0, 0, 1, 8);
        assert_eq!(&h[..1], range(511, 1));
        assert_eq!(&h[1..], build_proof(f(), 511, 0));
    }

    // the layers of the 259-block tree:
    //   127 ... 254 <- piece layer (4 blocks per piece)
    //   511 ... 771 ... padding ... 1022 <- block layer

    #[test]
    fn add_hashes_full_tree() {
        for blocks_per_piece in [1, 2, 4] {
            let mut t = MerkleTree::new(NUM_BLOCKS, blocks_per_piece, f()[0]);
            let res = t.add_hashes(511, 1, range(511, 512), &[]).expect("valid");
            t.check_invariant();
            assert!(res.passed.is_empty() && res.failed.is_empty());
            for i in 127..255 {
                assert_eq!(t.get(i), f()[i]);
            }
            for i in 511..1023 {
                assert_eq!(t.get(i), f()[i]);
            }
            assert_eq!(t.verified_leafs(), all_set(NUM_BLOCKS));
        }
    }

    #[test]
    fn add_hashes_one_piece() {
        let blocks_per_piece = 4;
        for piece_index in [0, 64, 5, 30] {
            let mut t = MerkleTree::new(NUM_BLOCKS, blocks_per_piece, f()[0]);
            let insert_idx = 127 + piece_index;
            let first = 511 + piece_index * blocks_per_piece;
            let res = t
                .add_hashes(first, 1, range(first, blocks_per_piece), &build_proof(f(), insert_idx, 0))
                .expect("valid");
            t.check_invariant();
            assert!(res.passed.is_empty() && res.failed.is_empty());

            // the trail of proof hashes
            let mut i = insert_idx;
            while i > 0 {
                assert_eq!(t.get(i), f()[i]);
                assert_eq!(t.get(merkle_get_sibling(i)), f()[merkle_get_sibling(i)]);
                i = merkle_get_parent(i);
            }
            // the piece layer: the root of the added hashes and its proof sibling only
            for i in 127..255 {
                if i == insert_idx || merkle_get_sibling(i) == insert_idx {
                    assert_eq!(t.get(i), f()[i]);
                } else {
                    assert!(t.get(i).is_zero());
                }
            }
            // the block layer
            for i in 511..1023 {
                if i >= first && i < first + 4 {
                    assert_eq!(t.get(i), f()[i]);
                } else {
                    assert!(t.get(i).is_zero());
                }
            }
            let start_block = piece_index * blocks_per_piece;
            let count = blocks_per_piece.min(NUM_BLOCKS - start_block);
            assert_eq!(t.verified_leafs(), set_range(none_set(NUM_BLOCKS), start_block, count));
        }
    }

    #[test]
    fn add_hashes_one_piece_invalid() {
        let blocks_per_piece = 4;
        for piece_index in [0, 64, 5, 30] {
            let insert_idx = 127 + piece_index;
            let first = 511 + piece_index * blocks_per_piece;

            // a corrupt proof
            let mut t = MerkleTree::new(NUM_BLOCKS, blocks_per_piece, f()[0]);
            let res = t.add_hashes(
                first,
                1,
                range(first, blocks_per_piece),
                &corrupt(&build_proof(f(), insert_idx, 0)),
            );
            assert!(res.is_none());
            assert_eq!(t.verified_leafs(), none_set(NUM_BLOCKS));
            t.check_invariant();

            // a corrupt hash
            let mut t = MerkleTree::new(NUM_BLOCKS, blocks_per_piece, f()[0]);
            let res = t.add_hashes(
                first,
                1,
                &corrupt(range(first, blocks_per_piece)),
                &build_proof(f(), insert_idx, 0),
            );
            assert!(res.is_none());
            assert_eq!(t.verified_leafs(), none_set(NUM_BLOCKS));
            t.check_invariant();
        }
    }

    /// SEC-09: junk hashes from a peer are refused before the tree grows to its full size.
    /// Here 2^20 blocks (a 16 GiB file) would mean a 64 MiB full tree.
    #[test]
    fn invalid_hashes_do_not_allocate_the_full_tree() {
        let num_blocks = 1 << 20;
        let blocks_per_piece = 4;
        let first_leaf = merkle_first_leaf(merkle_num_leafs(num_blocks));
        for (index, hashes, proof) in [
            // one block hash with no proof, as in the review's 97-byte message
            (first_leaf, vec![rand_sha256()], vec![]),
            // a piece's blocks with a made-up proof up to the root
            (first_leaf, vec![rand_sha256(); 4], vec![rand_sha256(); 18]),
        ] {
            let mut t = MerkleTree::new(num_blocks, blocks_per_piece, rand_sha256());
            assert!(t.add_hashes(index, 0, &hashes, &proof).is_none());
            assert_eq!(t.mode, Mode::Empty);
            assert!(t.tree.is_empty());
        }
    }

    #[test]
    fn add_hashes_full_tree_existing_blocks() {
        for piece_index in [0, 63] {
            for blocks_per_piece in [1, 2, 4] {
                for valid in [true, false] {
                    let mut t = MerkleTree::new(NUM_BLOCKS, blocks_per_piece, f()[0]);
                    let start = 511 + piece_index * blocks_per_piece;
                    let end = 511 + (piece_index * blocks_per_piece + 8).min(NUM_BLOCKS);
                    for i in start..end {
                        let h = if valid { f()[i] } else { rand_sha256() };
                        assert_eq!(t.set_block(i - 511, &h).0, SetBlockResult::Unknown);
                    }
                    t.check_invariant();

                    let res = t.add_hashes(511, 10, range(511, 512), &[]).expect("valid");
                    t.check_invariant();
                    let pieces: Vec<u32> =
                        (0..8 / blocks_per_piece as u32).map(|p| piece_index as u32 + p + 10).collect();
                    if valid {
                        assert_eq!(res.passed, pieces);
                        assert!(res.failed.is_empty());
                    } else {
                        assert!(res.passed.is_empty());
                        assert_eq!(res.failed.iter().map(|f| f.0).collect::<Vec<_>>(), pieces);
                        for (n, (_, blocks)) in res.failed.iter().enumerate() {
                            let idx = piece_index + n;
                            let len = blocks_per_piece.min(NUM_BLOCKS - idx * blocks_per_piece);
                            assert_eq!(*blocks, (0..len).collect::<Vec<_>>());
                        }
                    }
                    assert_eq!(t.verified_leafs(), all_set(NUM_BLOCKS));
                }
            }
        }
    }

    #[test]
    fn set_block_full_block_layer() {
        for valid in [true, false] {
            let mut t = MerkleTree::new(NUM_BLOCKS, 4, f()[0]);
            assert!(t.add_hashes(511, 1, range(511, 512), &[]).is_some());
            assert_eq!(t.verified_leafs(), all_set(NUM_BLOCKS));
            // the tree is complete: this only compares
            for block in 0..NUM_BLOCKS {
                let h = if valid { f()[511 + block] } else { rand_sha256() };
                let want = if valid { SetBlockResult::Ok } else { SetBlockResult::BlockHashFailed };
                assert_eq!(t.set_block(block, &h), (want, block, 1));
            }
            t.check_invariant();
        }
    }

    #[test]
    fn set_block_full_piece_layer() {
        let blocks_per_piece = 4;
        let mut t = MerkleTree::new(NUM_BLOCKS, blocks_per_piece, f()[0]);
        assert!(t.add_hashes(127, 1, range(127, 128), &[]).is_some());
        for block in 0..NUM_BLOCKS {
            let result = t.set_block(block, &f()[511 + block]);
            t.check_invariant();
            if block % blocks_per_piece == blocks_per_piece - 1 || block == NUM_BLOCKS - 1 {
                assert_eq!(result, (SetBlockResult::Ok, block - block % blocks_per_piece, blocks_per_piece));
                assert_eq!(t.verified_leafs(), set_range(none_set(NUM_BLOCKS), 0, block + 1));
            } else {
                assert_eq!(result.0, SetBlockResult::Unknown);
                assert_eq!(
                    t.verified_leafs(),
                    set_range(none_set(NUM_BLOCKS), 0, block - block % blocks_per_piece)
                );
            }
        }
    }

    #[test]
    fn set_block_invalid_full_piece_layer() {
        let blocks_per_piece = 4;
        let mut t = MerkleTree::new(NUM_BLOCKS, blocks_per_piece, f()[0]);
        assert!(t.add_hashes(127, 1, range(127, 128), &[]).is_some());
        for block in 0..NUM_BLOCKS {
            let result = t.set_block(block, &rand_sha256());
            t.check_invariant();
            if block % blocks_per_piece == blocks_per_piece - 1 || block == NUM_BLOCKS - 1 {
                assert_eq!(
                    result,
                    (SetBlockResult::HashFailed, block - block % blocks_per_piece, blocks_per_piece)
                );
            } else {
                assert_eq!(result.0, SetBlockResult::Unknown);
            }
            assert_eq!(t.verified_leafs(), none_set(NUM_BLOCKS));
        }
    }

    #[test]
    fn set_block_empty_tree() {
        let mut t = MerkleTree::new(NUM_BLOCKS, 4, f()[0]);
        for block in 0..NUM_BLOCKS - 1 {
            assert_eq!(t.set_block(block, &f()[511 + block]).0, SetBlockResult::Unknown);
            assert_eq!(t.verified_leafs(), none_set(NUM_BLOCKS));
        }
        let block = NUM_BLOCKS - 1;
        assert_eq!(t.set_block(block, &f()[511 + block]), (SetBlockResult::Ok, 0, num_leafs()));
        assert_eq!(t.verified_leafs(), all_set(NUM_BLOCKS));
        t.check_invariant();
    }

    #[test]
    fn set_block_invalid_empty_tree() {
        let mut t = MerkleTree::new(NUM_BLOCKS, 4, f()[0]);
        for block in 0..NUM_BLOCKS {
            let result = t.set_block(block, &rand_sha256());
            let want =
                if block == NUM_BLOCKS - 1 { SetBlockResult::HashFailed } else { SetBlockResult::Unknown };
            assert_eq!(result.0, want);
            assert_eq!(t.verified_leafs(), none_set(NUM_BLOCKS));
        }
        t.check_invariant();
    }

    #[test]
    fn add_hashes_block_layer_no_padding() {
        let mut t = MerkleTree::new(NUM_BLOCKS, 4, f()[0]);
        let res = t.add_hashes(511, 1, range(511, NUM_BLOCKS), &[]).expect("valid");
        assert!(res.passed.is_empty() && res.failed.is_empty());
        for i in 0..1023 {
            assert_eq!(t.get(i), f()[i]);
        }
        assert_eq!(t.verified_leafs(), all_set(NUM_BLOCKS));
    }

    #[test]
    fn add_hashes_piece_layer_no_padding() {
        let mut t = MerkleTree::new(NUM_BLOCKS, 4, f()[0]);
        let num_pieces = NUM_BLOCKS.div_ceil(4);
        let res = t.add_hashes(127, 1, range(127, num_pieces), &[]).expect("valid");
        t.check_invariant();
        assert!(res.passed.is_empty() && res.failed.is_empty());
        for i in 0..255 {
            assert_eq!(t.get(i), f()[i]);
        }
        for i in 255..1023 {
            assert!(t.get(i).is_zero());
        }
        assert_eq!(t.verified_leafs(), none_set(NUM_BLOCKS));
    }

    #[test]
    fn add_hashes_partial_proofs() {
        let mut t = MerkleTree::new(NUM_BLOCKS, 4, f()[0]);
        // the first two layers
        assert!(t.add_hashes(3, 1, range(3, 4), &[]).is_some());
        for i in 0..7 {
            assert_eq!(t.get(i), f()[i]);
        }
        // a proof that only reaches node 3, which is known
        let res = t.add_hashes(127, 1, range(127, 4), &build_proof(f(), 31, 3)).expect("valid");
        t.check_invariant();
        assert!(res.passed.is_empty() && res.failed.is_empty());
        for i in 127..131 {
            assert_eq!(t.get(i), f()[i]);
        }
        assert_eq!(t.verified_leafs(), none_set(NUM_BLOCKS));
    }

    #[test]
    fn hostile_add_hashes_and_get_hashes_do_not_panic() {
        let mut t = MerkleTree::new(NUM_BLOCKS, 4, f()[0]);
        assert!(t.add_hashes(1022, 1, range(511, 4), &[]).is_none());
        assert!(t.add_hashes(511, 1, &[], &[]).is_none());
        assert!(t.add_hashes(511, 1, range(511, 3), &[f()[1]; 70]).is_none());
        t.check_invariant();
        assert!(t.get_hashes(20, 0, 1, 0).is_empty());
        assert!(t.get_hashes(0, 0, 1, 40).is_empty());
    }
}
