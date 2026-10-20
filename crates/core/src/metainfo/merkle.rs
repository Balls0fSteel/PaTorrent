// Ported from libtorrent-rasterbar 2.0.11: src/merkle.cpp, include/libtorrent/aux_/merkle.hpp,
// test/test_merkle.cpp
// Copyright (c) 2015, 2017, 2019-2021, Arvid Norberg; 2015, Mike Tzou; 2017, Steven Siloti.
// BSD-3-Clause, see THIRD-PARTY-NOTICES.md.

//! Merkle tree helpers for BitTorrent v2 (BEP 52).
//!
//! A tree is a flat array: node 0 is the root, the children of node `i` are `2i + 1` and
//! `2i + 2`, and the leaves are the last `num_leafs` entries. A zero hash means "unknown".
//!
//! ```text
//!             0
//!      1              2
//!   3      4       5       6
//!  7 8    9 10   11 12   13 14
//! ```

use crate::crypto::Sha256;
use crate::types::Sha256Hash;
use crate::util::bitfield::Bitfield;

/// SHA-256 of two concatenated hashes: the parent of `left` and `right`.
pub fn merkle_hash_pair(left: &Sha256Hash, right: &Sha256Hash) -> Sha256Hash {
    let mut h = Sha256::new();
    h.update(&left.0).update(&right.0);
    h.finalize()
}

/// Index of the first node of `layer` (layer 0 is the root).
pub fn merkle_layer_start(layer: usize) -> usize {
    debug_assert!(layer < usize::BITS as usize);
    (1 << layer) - 1
}

/// Flat index of the node `offset` steps from the left of `layer`.
pub fn merkle_to_flat_index(layer: usize, offset: usize) -> usize {
    merkle_layer_start(layer) + offset
}

pub fn merkle_get_parent(tree_node: usize) -> usize {
    // node 0 has no parent
    debug_assert!(tree_node > 0);
    (tree_node - 1) / 2
}

pub fn merkle_get_sibling(tree_node: usize) -> usize {
    // node 0 has no sibling; odd nodes have theirs to the right, even ones to the left
    debug_assert!(tree_node > 0);
    if tree_node & 1 == 1 { tree_node + 1 } else { tree_node - 1 }
}

pub fn merkle_get_first_child(tree_node: usize) -> usize {
    tree_node * 2 + 1
}

/// The first descendant of `tree_node` `depth` layers down.
pub fn merkle_get_first_child_depth(tree_node: usize, depth: usize) -> usize {
    ((tree_node + 1) << depth) - 1
}

/// Total nodes in a full binary tree with `leafs` leaves (a power of two).
pub fn merkle_num_nodes(leafs: usize) -> usize {
    debug_assert!(leafs > 0 && leafs.is_power_of_two());
    ((leafs - 1) << 1) + 1
}

/// Index of the first leaf in a tree with `num_leafs` leaves (a power of two).
pub fn merkle_first_leaf(num_leafs: usize) -> usize {
    debug_assert!(num_leafs > 0 && num_leafs.is_power_of_two());
    num_leafs - 1
}

/// Leaves in a tree over `blocks` items: the next power of two (at least 1).
pub fn merkle_num_leafs(blocks: usize) -> usize {
    blocks.max(1).next_power_of_two()
}

/// Height of a tree with `leaves` leaves (a power of two), not counting the root: a tree with
/// only a root has 0 layers.
pub fn merkle_num_layers(mut leaves: usize) -> usize {
    debug_assert!(leaves & leaves.wrapping_sub(1) == 0);
    let mut layers = 0;
    while leaves > 1 {
        layers += 1;
        leaves >>= 1;
    }
    layers
}

/// Computes every interior node from the leaves, which must all be set.
pub fn merkle_fill_tree(tree: &mut [Sha256Hash], num_leafs: usize) {
    merkle_fill_tree_from(tree, num_leafs, merkle_num_nodes(num_leafs) - num_leafs);
}

/// Computes the subtree above the `num_leafs` nodes starting at `level_start`, up to their
/// common root.
pub fn merkle_fill_tree_from(tree: &mut [Sha256Hash], num_leafs: usize, mut level_start: usize) {
    debug_assert!(num_leafs >= 1);
    let mut level_size = num_leafs;
    while level_size > 1 {
        let mut parent = merkle_get_parent(level_start);
        let mut i = level_start;
        while i < level_start + level_size {
            tree[parent] = merkle_hash_pair(&tree[i], &tree[i + 1]);
            i += 2;
            parent += 1;
        }
        level_start = merkle_get_parent(level_start);
        level_size /= 2;
    }
}

/// Fills in the nodes that can be computed from a tree with arbitrary nodes set, then clears
/// every "orphan" hash, one that does not contribute towards the root and so cannot be
/// validated.
pub fn merkle_fill_partial_tree(tree: &mut [Sha256Hash]) {
    let num_nodes = tree.len();
    // the tree size must be one less than a power of two
    debug_assert!((num_nodes + 1) & num_nodes == 0);

    let num_leafs = num_nodes.div_ceil(2);
    let mut level_size = num_leafs;
    let mut level_start = merkle_first_leaf(num_leafs);
    while level_size > 1 {
        level_start = merkle_get_parent(level_start);
        level_size /= 2;
        for i in level_start..level_start + level_size {
            let child = merkle_get_first_child(i);
            if tree[child].is_zero() || tree[child + 1].is_zero() {
                continue;
            }
            tree[i] = merkle_hash_pair(&tree[child], &tree[child + 1]);
        }
    }

    let mut parent = 0;
    let mut i = 1;
    while i < tree.len() {
        if tree[parent].is_zero() {
            // the validation chain up to the root is broken
            tree[i].clear();
            tree[i + 1].clear();
        } else if tree[i + 1].is_zero() {
            // without its sibling this hash cannot be validated
            tree[i].clear();
        } else if tree[i].is_zero() {
            tree[i + 1].clear();
        }
        i += 2;
        parent += 1;
    }
}

/// Clears the `num_leafs` nodes starting at `level_start` (all in one layer) and their parents
/// within the subtree, but not the subtree's root.
pub fn merkle_clear_tree(tree: &mut [Sha256Hash], num_leafs: usize, mut level_start: usize) {
    debug_assert!(num_leafs >= 1);
    debug_assert!(level_start + num_leafs <= tree.len());
    debug_assert!(merkle_get_layer(level_start) == merkle_get_layer(level_start + num_leafs - 1));
    let mut level_size = num_leafs;
    loop {
        for h in &mut tree[level_start..level_start + level_size] {
            h.clear();
        }
        if level_size == 1 {
            break;
        }
        level_start = merkle_get_parent(level_start);
        level_size /= 2;
    }
}

/// The merkle root over `leaves`, padded on the right with `pad` up to a power of two.
pub fn merkle_root(leaves: &[Sha256Hash], pad: &Sha256Hash) -> Sha256Hash {
    let num_leafs = merkle_num_leafs(leaves.len());
    let mut scratch = Vec::new();
    merkle_root_scratch(leaves, num_leafs, *pad, &mut scratch)
}

/// The root of a tree of `num_leafs` leaves whose first ones are `leaves` and the rest `pad`.
/// `scratch_space` is reused between calls.
pub fn merkle_root_scratch(
    leaves: &[Sha256Hash],
    mut num_leafs: usize,
    mut pad: Sha256Hash,
    scratch_space: &mut Vec<Sha256Hash>,
) -> Sha256Hash {
    debug_assert!(num_leafs > 0 && num_leafs.is_power_of_two());
    if num_leafs == 1 {
        return leaves[0];
    }
    scratch_space.clear();
    scratch_space.extend_from_slice(leaves);
    let mut len = leaves.len();
    while num_leafs > 1 {
        let mut i = 0;
        while i < len / 2 {
            scratch_space[i] = merkle_hash_pair(&scratch_space[i * 2], &scratch_space[i * 2 + 1]);
            i += 1;
        }
        if len & 1 == 1 {
            // an odd number of leaves: the boundary hash spans a payload hash and a pad hash
            scratch_space[i] = merkle_hash_pair(&scratch_space[i * 2], &pad);
            i += 1;
        }
        // pad hashes are implied, only the current layer's pad is kept
        pad = merkle_hash_pair(&pad, &pad);
        len = i;
        num_leafs /= 2;
    }
    scratch_space[0]
}

/// The layer node `idx` is in (0 is the root).
pub fn merkle_get_layer(idx: usize) -> usize {
    let mut layer = 1;
    while idx > (1 << layer) - 2 {
        layer += 1;
    }
    layer - 1
}

/// How many steps from the left of its layer node `idx` is.
pub fn merkle_get_layer_offset(idx: usize) -> usize {
    idx - ((1 << merkle_get_layer(idx)) - 1)
}

/// The pad hash of the layer with `pieces` nodes in a tree whose leaf layer has `blocks`
/// (zero when `pieces >= blocks`).
pub fn merkle_pad(blocks: usize, mut pieces: usize) -> Sha256Hash {
    let mut ret = Sha256Hash::default();
    while pieces < blocks {
        ret = merkle_hash_pair(&ret, &ret);
        pieces *= 2;
    }
    ret
}

/// Validates `node` at `target_node_idx` with the uncle hashes leading up to a known node of
/// `target_tree`, and inserts the node and the proofs if they validate. Any non-zero hash met
/// on the way is taken as valid and ends the walk, matching or not.
///
/// ```text
///            R
///     2              _
///  _     _        _     1
/// _   _ _   _    N   0 _   _
/// ```
///
/// With the root R known, inserting N takes the uncle hashes 0, 1 and 2.
pub fn merkle_validate_and_insert_proofs(
    target_tree: &mut [Sha256Hash],
    target_node_idx: usize,
    node: &Sha256Hash,
    uncle_hashes: &[Sha256Hash],
) -> bool {
    if target_tree[target_node_idx] == *node {
        return true;
    }
    if !target_tree[target_node_idx].is_zero() || uncle_hashes.is_empty() {
        return false;
    }

    let mut cursor = target_node_idx;
    target_tree[cursor] = *node;
    for proof in uncle_hashes {
        let proof_idx = merkle_get_sibling(cursor);
        debug_assert!(target_tree[proof_idx].is_zero());
        target_tree[proof_idx] = *proof;
        let left = proof_idx.min(cursor);
        let hash = merkle_hash_pair(&target_tree[left], &target_tree[left + 1]);
        cursor = merkle_get_parent(cursor);
        if target_tree[cursor] == hash {
            return true;
        }
        if !target_tree[cursor].is_zero() {
            break;
        }
        target_tree[cursor] = hash;
    }

    // never reached a known hash: undo what was inserted
    let mut clear_cursor = target_node_idx;
    while clear_cursor > cursor {
        let proof_idx = merkle_get_sibling(clear_cursor);
        target_tree[clear_cursor].clear();
        target_tree[proof_idx].clear();
        clear_cursor = merkle_get_parent(clear_cursor);
    }
    false
}

pub fn merkle_validate_node(left: &Sha256Hash, right: &Sha256Hash, parent: &Sha256Hash) -> bool {
    merkle_hash_pair(left, right) == *parent
}

/// Copies the hashes of `src` that validate against `root` into `dst` (same size; cleared or
/// holding valid hashes only) and sets the bits of the leaves found valid in `verified_leafs`.
pub fn merkle_validate_copy(
    src: &[Sha256Hash],
    dst: &mut [Sha256Hash],
    root: &Sha256Hash,
    verified_leafs: &mut Bitfield,
) {
    debug_assert_eq!(src.len(), dst.len());
    if src.is_empty() || src[0] != *root {
        return;
    }
    let num_leafs = dst.len().div_ceil(2);
    dst[0] = src[0];
    let leaf_layer_start = src.len() - num_leafs;
    for i in 0..leaf_layer_start {
        if dst[i].is_zero() {
            continue;
        }
        let left_child = merkle_get_first_child(i);
        let right_child = left_child + 1;
        if merkle_validate_node(&src[left_child], &src[right_child], &dst[i]) {
            dst[left_child] = src[left_child];
            dst[right_child] = src[right_child];
            if left_child >= leaf_layer_start {
                let block_idx = left_child - leaf_layer_start;
                if block_idx < verified_leafs.len() {
                    verified_leafs.set(block_idx, true);
                    // the right child may be the first pad hash, not part of the bitfield
                    if block_idx + 1 < verified_leafs.len() {
                        verified_leafs.set(block_idx + 1, true);
                    }
                }
            }
        }
    }
}

/// True if every pair in the bottom layer of `tree` hashes to its parent.
pub fn merkle_validate_single_layer(tree: &[Sha256Hash]) -> bool {
    if tree.len() == 1 {
        return true;
    }
    let num_leafs = tree.len().div_ceil(2);
    debug_assert!(num_leafs.is_power_of_two());
    let mut idx = merkle_first_leaf(num_leafs);
    while idx < tree.len() {
        if !merkle_validate_node(&tree[idx], &tree[idx + 1], &tree[merkle_get_parent(idx)]) {
            return false;
        }
        idx += 2;
    }
    true
}

/// For leaf `block_index`, the largest subtree whose leaves are all known: returns
/// `(leafs_start, leafs_size, root_index)`, the range of the leaf layer that can be verified
/// and the node that must be known to do so (the caller checks it). Leaves from
/// `num_valid_leafs` on are padding and expected to be zero.
pub fn merkle_find_known_subtree(
    tree: &[Sha256Hash],
    block_index: usize,
    num_valid_leafs: usize,
) -> (usize, usize, usize) {
    let mut leafs_start = block_index;
    let mut leafs_size = 1usize;
    let first_leaf = tree.len() / 2;
    let mut root_index = merkle_get_sibling(first_leaf + block_index);
    let mut i = block_index;
    loop {
        let first_check_index = if i & 1 == 1 {
            leafs_start as isize - leafs_size as isize
        } else {
            (leafs_start + leafs_size) as isize
        };
        let n = (leafs_size as isize).min(num_valid_leafs as isize - first_check_index);
        for j in 0..n.max(0) {
            if tree[(first_leaf as isize + first_check_index + j) as usize].is_zero() {
                return (leafs_start, leafs_size, root_index);
            }
        }
        if i & 1 == 1 {
            leafs_start -= leafs_size;
        }
        leafs_size *= 2;
        root_index = merkle_get_parent(root_index);
        // a known inner node implies a known parent, so if the root is known the next sibling
        // subtree is computed already when all of its leaves are valid
        if !tree[root_index].is_zero() {
            break;
        }
        debug_assert!(root_index != 0);
        i >>= 1;
    }
    debug_assert!(leafs_start + leafs_size > block_index);
    (leafs_start, leafs_size, root_index)
}

/// A full tree over `size` synthetic leaves (leaf `i` is the 32-bit value `i + 1` repeated in
/// native byte order), as libtorrent's test helper `build_tree` makes it.
#[cfg(test)]
pub(crate) fn build_tree(size: usize) -> Vec<Sha256Hash> {
    let num_leafs = merkle_num_leafs(size);
    let mut full_tree = vec![Sha256Hash::default(); merkle_num_nodes(num_leafs)];
    let first = full_tree.len() - num_leafs;
    for i in 0..size {
        let mut h = [0u8; 32];
        for c in h.chunks_exact_mut(4) {
            c.copy_from_slice(&(i as u32 + 1).to_ne_bytes());
        }
        full_tree[first + i] = Sha256Hash(h);
    }
    merkle_fill_tree(&mut full_tree, num_leafs);
    full_tree
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &[u8; 32]) -> Sha256Hash {
        Sha256Hash(*text)
    }

    fn hp(l: Sha256Hash, r: Sha256Hash) -> Sha256Hash {
        merkle_hash_pair(&l, &r)
    }

    fn bits(b: &Bitfield, expected: &str) {
        for (i, c) in expected.chars().enumerate() {
            assert_eq!(b.get(i), c == '1', "bit {i} of {expected}");
        }
    }

    struct Fixture {
        a: Sha256Hash,
        b: Sha256Hash,
        c: Sha256Hash,
        d: Sha256Hash,
        e: Sha256Hash,
        f: Sha256Hash,
        g: Sha256Hash,
        h: Sha256Hash,
        o: Sha256Hash,
        l: Sha256Hash,
        ab: Sha256Hash,
        cd: Sha256Hash,
        ef: Sha256Hash,
        gh: Sha256Hash,
        ad: Sha256Hash,
        eh: Sha256Hash,
        ah: Sha256Hash,
    }

    fn fx() -> Fixture {
        let a = s(b"11111111111111111111111111111111");
        let b = s(b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let c = s(b"cccccccccccccccccccccccccccccccc");
        let d = s(b"dddddddddddddddddddddddddddddddd");
        let e = s(b"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let f = s(b"ffffffffffffffffffffffffffffffff");
        let g = s(b"gggggggggggggggggggggggggggggggg");
        let h = s(b"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii");
        let (ab, cd, ef, gh) = (hp(a, b), hp(c, d), hp(e, f), hp(g, h));
        let (ad, eh) = (hp(ab, cd), hp(ef, gh));
        Fixture {
            a,
            b,
            c,
            d,
            e,
            f,
            g,
            h,
            o: Sha256Hash::default(),
            l: s(b"11111111111111111111111111111111"),
            ab,
            cd,
            ef,
            gh,
            ad,
            eh,
            ah: hp(ad, eh),
        }
    }

    #[test]
    fn num_leafs() {
        let expected = [1, 2, 4, 4, 8, 8, 8, 8, 16, 16, 16, 16, 16, 16, 16, 16, 32, 32];
        for (i, &e) in expected.iter().enumerate() {
            assert_eq!(merkle_num_leafs(i + 1), e, "{}", i + 1);
        }
    }

    #[test]
    fn get_parent_and_sibling() {
        let parents = [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6];
        let siblings = [2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 12, 11, 14, 13];
        for i in 1..15 {
            assert_eq!(merkle_get_parent(i), parents[i - 1]);
            assert_eq!(merkle_get_sibling(i), siblings[i - 1]);
        }
    }

    #[test]
    fn num_nodes_and_first_leaf() {
        for (leafs, nodes, first) in [(1, 1, 0), (2, 3, 1), (4, 7, 3), (8, 15, 7), (16, 31, 15)] {
            assert_eq!(merkle_num_nodes(leafs), nodes);
            assert_eq!(merkle_first_leaf(leafs), first);
        }
    }

    #[test]
    fn get_layer_and_offset() {
        let layers = [0, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4];
        let offsets = [0, 0, 1, 0, 1, 2, 3, 0, 1, 2, 3, 4, 5, 6, 7, 0];
        for i in 0..16 {
            assert_eq!(merkle_get_layer(i), layers[i], "{i}");
            assert_eq!(merkle_get_layer_offset(i), offsets[i], "{i}");
        }
    }

    #[test]
    fn num_layers() {
        for (leaves, layers) in [(0, 0), (1, 0), (2, 1), (4, 2), (8, 3), (16, 4)] {
            assert_eq!(merkle_num_layers(leaves), layers);
        }
    }

    #[test]
    fn get_first_child() {
        for i in 0..17 {
            assert_eq!(merkle_get_first_child(i), 2 * i + 1);
        }
        for (depth, want) in [0, 1, 3, 7, 15, 31].into_iter().enumerate() {
            assert_eq!(merkle_get_first_child_depth(0, depth), want);
        }
        for (depth, want) in [2, 5, 11, 23, 47, 95].into_iter().enumerate() {
            assert_eq!(merkle_get_first_child_depth(2, depth), want);
        }
    }

    #[test]
    fn layer_start_and_flat_index() {
        for (layer, want) in [0, 1, 3, 7, 15, 31, 63, 127, 255, 511].into_iter().enumerate() {
            assert_eq!(merkle_layer_start(layer), want);
        }
        let cases = [
            (0, 0, 0),
            (1, 0, 1),
            (1, 1, 2),
            (2, 0, 3),
            (2, 1, 4),
            (2, 2, 5),
            (2, 3, 6),
            (3, 0, 7),
            (3, 1, 8),
            (3, 2, 9),
            (3, 3, 10),
            (3, 4, 11),
            (3, 5, 12),
            (3, 6, 13),
            (3, 7, 14),
        ];
        for (layer, offset, want) in cases {
            assert_eq!(merkle_to_flat_index(layer, offset), want);
        }
    }

    #[test]
    fn fill_tree() {
        let Fixture { a, b, c, d, e, f, g, h, o, ab, cd, ef, gh, ad, eh, ah, .. } = fx();

        let mut tree = vec![o, o, o, o, o, o, o, a, b, c, d, e, f, g, h];
        merkle_fill_tree_from(&mut tree, 8, 7);
        assert_eq!(tree, vec![ah, ad, eh, ab, cd, ef, gh, a, b, c, d, e, f, g, h]);

        // left side
        let mut tree = vec![o, o, o, ab, cd, o, o, a, b, c, d, o, o, o, o];
        merkle_fill_tree_from(&mut tree, 4, 7);
        assert_eq!(tree, vec![o, ad, o, ab, cd, o, o, a, b, c, d, o, o, o, o]);

        // right side
        let mut tree = vec![o, o, o, o, o, o, o, o, o, o, o, a, b, c, d];
        merkle_fill_tree_from(&mut tree, 4, 11);
        assert_eq!(tree, vec![o, o, ad, o, o, ab, cd, o, o, o, o, a, b, c, d]);

        // shallow left
        let mut tree = vec![o, o, o, a, b, o, o, o, o, o, o, o, o, o, o];
        merkle_fill_tree_from(&mut tree, 2, 3);
        assert_eq!(tree, vec![o, ab, o, a, b, o, o, o, o, o, o, o, o, o, o]);

        // shallow right
        let mut tree = vec![o, o, o, o, o, a, b, o, o, o, o, o, o, o, o];
        merkle_fill_tree_from(&mut tree, 2, 5);
        assert_eq!(tree, vec![o, o, ab, o, o, a, b, o, o, o, o, o, o, o, o]);
    }

    #[test]
    fn fill_partial_tree() {
        let Fixture { a, b, c, d, e, f, g, h, o, ab, cd, ef, gh, ad, eh, ah, .. } = fx();
        let cases: [(Vec<Sha256Hash>, Vec<Sha256Hash>); 11] = [
            // whole tree
            (
                vec![o, o, o, o, o, o, o, a, b, c, d, e, f, g, h],
                vec![ah, ad, eh, ab, cd, ef, gh, a, b, c, d, e, f, g, h],
            ),
            // left side
            (
                vec![o, o, eh, ab, cd, o, o, a, b, c, d, o, o, o, o],
                vec![ah, ad, eh, ab, cd, o, o, a, b, c, d, o, o, o, o],
            ),
            // right side
            (
                vec![o, ad, o, o, o, o, o, o, o, o, o, e, f, g, h],
                vec![ah, ad, eh, o, o, ef, gh, o, o, o, o, e, f, g, h],
            ),
            // shallow left
            (
                vec![o, o, eh, ab, cd, o, o, o, o, o, o, o, o, o, o],
                vec![ah, ad, eh, ab, cd, o, o, o, o, o, o, o, o, o, o],
            ),
            // shallow right
            (
                vec![o, ad, o, o, o, ef, gh, o, o, o, o, o, o, o, o],
                vec![ah, ad, eh, o, o, ef, gh, o, o, o, o, o, o, o, o],
            ),
            // uneven
            (
                vec![o, ad, o, o, o, ef, gh, o, o, o, o, o, o, o, o],
                vec![ah, ad, eh, o, o, ef, gh, o, o, o, o, o, o, o, o],
            ),
            // clear orphans
            (
                vec![o, ad, ah, o, o, ef, gh, a, o, c, o, o, o, o, o],
                vec![ah, ad, eh, o, o, ef, gh, o, o, o, o, o, o, o, o],
            ),
            // clear orphan subtree
            (vec![o, o, o, o, o, o, o, a, b, c, d, o, o, o, o], vec![o; 15]),
            // fill subtree
            (
                vec![o, o, eh, o, o, o, o, a, b, c, d, o, o, o, o],
                vec![ah, ad, eh, ab, cd, o, o, a, b, c, d, o, o, o, o],
            ),
            // clear no-siblings left
            (
                vec![o, ad, ah, o, o, ef, gh, o, o, o, o, o, o, o, h],
                vec![ah, ad, eh, o, o, ef, gh, o, o, o, o, o, o, o, o],
            ),
            // fill gaps
            (
                vec![o, ad, ah, o, o, ef, gh, a, b, c, d, o, o, o, o],
                vec![ah, ad, eh, ab, cd, ef, gh, a, b, c, d, o, o, o, o],
            ),
        ];
        for (n, (mut tree, want)) in cases.into_iter().enumerate() {
            merkle_fill_partial_tree(&mut tree);
            assert_eq!(tree, want, "case {n}");
        }
        // clear no-siblings right
        let mut tree = vec![o, ad, ah, o, o, ef, gh, o, o, o, o, o, o, g, o];
        merkle_fill_partial_tree(&mut tree);
        assert_eq!(tree, vec![ah, ad, eh, o, o, ef, gh, o, o, o, o, o, o, o, o]);
    }

    #[test]
    fn root() {
        let Fixture { a, b, c, d, e, f, g, h, o, ab, ef, ad, ah, .. } = fx();
        assert_eq!(merkle_root(&[a, b, c, d, e, f, g, h], &o), ah);
        assert_eq!(merkle_root(&[a, b, c, d, e, f], &o), hp(ad, hp(ef, hp(o, o))));
        assert_eq!(merkle_root(&[a, b], &o), ab);
        assert_eq!(merkle_root(&[a], &o), a);
    }

    #[test]
    fn root_scratch() {
        let Fixture { a, b, c, d, e, f, g, h, o, ab, ef, ad, ah, .. } = fx();
        let mut buf = Vec::new();
        assert_eq!(merkle_root_scratch(&[a, b, c, d, e, f, g, h], 8, o, &mut buf), ah);
        assert_eq!(merkle_root_scratch(&[a, b, c, d, e, f], 8, o, &mut buf), hp(ad, hp(ef, hp(o, o))));
        assert_eq!(merkle_root_scratch(&[a, b], 2, o, &mut buf), ab);
        // unaligned leaf layer
        assert_eq!(
            merkle_root_scratch(&[a, b, c], 8, o, &mut buf),
            hp(hp(ab, hp(c, o)), hp(hp(o, o), hp(o, o)))
        );
    }

    #[test]
    fn clear_tree() {
        let Fixture { l, o, .. } = fx();
        let full = || vec![l; 15];
        let cases = [
            (8, 7, vec![o; 15]),
            (4, 7, vec![l, o, l, o, o, l, l, o, o, o, o, l, l, l, l]),
            (4, 11, vec![l, l, o, l, l, o, o, l, l, l, l, o, o, o, o]),
            (2, 3, vec![l, o, l, o, o, l, l, l, l, l, l, l, l, l, l]),
            (2, 5, vec![l, l, o, l, l, o, o, l, l, l, l, l, l, l, l]),
        ];
        for (num, start, want) in cases {
            let mut tree = full();
            merkle_clear_tree(&mut tree, num, start);
            assert_eq!(tree, want, "clear {num} at {start}");
        }
    }

    #[test]
    fn pad() {
        let z = Sha256Hash::default();
        for n in [1, 2, 4, 8, 16] {
            assert_eq!(merkle_pad(n, n), z);
        }
        let pad1 = hp(z, z);
        for (b, p) in [(2, 1), (4, 2), (8, 4), (16, 8)] {
            assert_eq!(merkle_pad(b, p), pad1);
        }
        let pad2 = hp(pad1, pad1);
        for (b, p) in [(4, 1), (8, 2), (16, 4), (32, 8)] {
            assert_eq!(merkle_pad(b, p), pad2);
        }
    }

    #[test]
    fn validate_node() {
        let Fixture { a, b, c, d, e, f, g, h, ab, cd, ef, gh, ad, eh, ah, .. } = fx();
        assert!(merkle_validate_node(&a, &b, &ab));
        assert!(merkle_validate_node(&c, &d, &cd));
        assert!(merkle_validate_node(&e, &f, &ef));
        assert!(merkle_validate_node(&g, &h, &gh));
        assert!(merkle_validate_node(&ab, &cd, &ad));
        assert!(merkle_validate_node(&ef, &gh, &eh));
        assert!(merkle_validate_node(&ad, &eh, &ah));
        assert!(!merkle_validate_node(&b, &a, &ab));
        assert!(!merkle_validate_node(&d, &c, &cd));
        assert!(!merkle_validate_node(&f, &e, &ef));
        assert!(!merkle_validate_node(&h, &g, &gh));
    }

    #[test]
    fn validate_copy() {
        let Fixture { a, b, c, d, e, f, g, h, o, ab, cd, ef, gh, ad, eh, ah, .. } = fx();
        let full = vec![ah, ad, eh, ab, cd, ef, gh, a, b, c, d, e, f, g, h];

        // full
        let mut dst = vec![o; 15];
        let mut verified = Bitfield::new(8, false);
        merkle_validate_copy(&full, &mut dst, &ah, &mut verified);
        bits(&verified, "11111111");
        assert_eq!(dst, full);

        // odd nodes: h pretends to be padding
        let mut dst = vec![o; 15];
        let mut verified = Bitfield::new(7, false);
        merkle_validate_copy(&full, &mut dst, &ah, &mut verified);
        bits(&verified, "1111111");
        assert_eq!(dst, full);

        // invalid leaf: leaf 5 is wrong, so its sibling cannot be validated either
        let src = vec![ah, ad, eh, ab, cd, ef, gh, a, b, c, d, e, ef, g, h];
        let mut dst = vec![o; 15];
        let mut verified = Bitfield::new(8, false);
        merkle_validate_copy(&src, &mut dst, &ah, &mut verified);
        bits(&verified, "11110011");
        assert_eq!(dst, vec![ah, ad, eh, ab, cd, ef, gh, a, b, c, d, o, o, g, h]);

        // many invalid leaves
        let src = vec![ah, ad, eh, ab, cd, ef, gh, a, b, ef, d, eh, ef, g, ah];
        let mut dst = vec![o; 15];
        let mut verified = Bitfield::new(8, false);
        merkle_validate_copy(&src, &mut dst, &ah, &mut verified);
        bits(&verified, "11000000");
        assert_eq!(dst, vec![ah, ad, eh, ab, cd, ef, gh, a, b, o, o, o, o, o, o]);

        // partial
        let src = vec![ah, ad, eh, ab, cd, ef, o, a, b, c, o, o, o, o, o];
        let mut dst = vec![o; 15];
        let mut verified = Bitfield::new(8, false);
        merkle_validate_copy(&src, &mut dst, &ah, &mut verified);
        bits(&verified, "11000000");
        assert_eq!(dst, vec![ah, ad, eh, ab, cd, o, o, a, b, o, o, o, o, o, o]);

        // invalid root
        let mut dst = vec![o; 15];
        let mut verified = Bitfield::new(8, false);
        merkle_validate_copy(&src, &mut dst, &a, &mut verified);
        bits(&verified, "00000000");
        assert_eq!(dst, vec![o; 15]);

        // root only
        let mut src = vec![o; 15];
        src[0] = ah;
        let mut dst = vec![o; 15];
        let mut verified = Bitfield::new(8, false);
        merkle_validate_copy(&src, &mut dst, &ah, &mut verified);
        bits(&verified, "00000000");
        assert_eq!(dst, src);
    }

    #[test]
    fn validate_single_layer() {
        let Fixture { a, b, c, d, e, f, g, h, o, ab, cd, ef, gh, .. } = fx();
        assert!(!merkle_validate_single_layer(&[o, o, o, o, o, o, o, a, b, c, d, e, f, g, h]));
        assert!(!merkle_validate_single_layer(&[o, o, o, ab, cd, o, gh, a, b, c, d, e, f, g, h]));
        assert!(!merkle_validate_single_layer(&[o, o, o, ab, cd, ef, gh, a, b, c, o, e, f, g, h]));
        assert!(merkle_validate_single_layer(&[o, o, o, ab, cd, ef, gh, a, b, c, d, e, f, g, h]));
    }

    #[test]
    fn find_known_subtree() {
        let Fixture { a, b, c, d, e, f, g, h, o, ab, cd, ef, gh, ad, eh, ah, .. } = fx();
        let full = [ah, ad, eh, ab, cd, ef, gh, a, b, c, d, e, f, g, h];
        assert_eq!(merkle_find_known_subtree(&full, 1, 8), (0, 2, 3));
        let two_levels = [ah, ad, eh, o, o, ef, gh, a, b, c, d, e, f, g, h];
        assert_eq!(merkle_find_known_subtree(&two_levels, 1, 8), (0, 4, 1));
        let unknown = [ah, ad, eh, o, o, ef, gh, a, b, o, d, e, f, g, h];
        assert_eq!(merkle_find_known_subtree(&unknown, 1, 8), (0, 2, 3));
        // the last leaf is padding: taken as correct although zero
        let padding = [ah, ad, eh, o, o, ef, gh, a, b, o, d, e, f, g, o];
        assert_eq!(merkle_find_known_subtree(&padding, 6, 7), (6, 2, 6));
        let padding2 = [ah, ad, eh, o, o, o, o, a, b, o, d, e, f, g, o];
        assert_eq!(merkle_find_known_subtree(&padding2, 6, 7), (4, 4, 2));
        let more_padding = [ah, ad, eh, o, o, o, o, a, b, o, d, e, f, o, o];
        assert_eq!(merkle_find_known_subtree(&more_padding, 5, 6), (4, 4, 2));
    }

    #[test]
    fn validate_and_insert_proofs() {
        let Fixture { a, b, e, f, g, h, o, cd, ef, gh, ad, eh, ah, ab, .. } = fx();
        let with_root = |r: Sha256Hash| {
            let mut t = vec![o; 15];
            t[0] = r;
            t
        };

        // mixed
        let mut tree = with_root(ah);
        assert!(merkle_validate_and_insert_proofs(&mut tree, 11, &e, &[f, gh, ad]));
        assert_eq!(tree, vec![ah, ad, eh, o, o, ef, gh, o, o, o, o, e, f, o, o]);

        // mixed failure: wrong root, everything filled in is cleared again
        let mut tree = with_root(eh);
        assert!(!merkle_validate_and_insert_proofs(&mut tree, 11, &e, &[f, gh, ad]));
        assert_eq!(tree, with_root(eh));

        // left
        let mut tree = with_root(ah);
        assert!(merkle_validate_and_insert_proofs(&mut tree, 7, &a, &[b, cd, eh]));
        assert_eq!(tree, vec![ah, ad, eh, ab, cd, o, o, a, b, o, o, o, o, o, o]);

        // right
        let mut tree = with_root(ah);
        assert!(merkle_validate_and_insert_proofs(&mut tree, 14, &h, &[g, ef, ad]));
        assert_eq!(tree, vec![ah, ad, eh, o, o, ef, gh, o, o, o, o, o, o, g, h]);

        // early success
        let mut tree = with_root(ah);
        tree[1] = ad;
        tree[2] = eh;
        assert!(merkle_validate_and_insert_proofs(&mut tree, 11, &e, &[f, gh, ad]));
        assert_eq!(tree, vec![ah, ad, eh, o, o, ef, gh, o, o, o, o, e, f, o, o]);

        // early failure: node 2 holds a wrong hash
        let mut tree = with_root(ah);
        tree[1] = ad;
        tree[2] = ah;
        assert!(!merkle_validate_and_insert_proofs(&mut tree, 11, &e, &[f, gh, ad]));
        let mut want = with_root(ah);
        want[1] = ad;
        want[2] = ah;
        assert_eq!(tree, want);

        // no uncles
        let mut tree = with_root(ah);
        assert!(!merkle_validate_and_insert_proofs(&mut tree, 1, &ad, &[]));
        assert_eq!(tree, with_root(ah));

        // the root itself
        let mut tree = with_root(ah);
        assert!(merkle_validate_and_insert_proofs(&mut tree, 0, &ah, &[]));
        assert_eq!(tree, with_root(ah));
        assert!(!merkle_validate_and_insert_proofs(&mut tree, 0, &a, &[]));
        assert_eq!(tree, with_root(ah));

        // too many uncles: the extra ones are never looked at
        let mut tree = with_root(ah);
        assert!(merkle_validate_and_insert_proofs(&mut tree, 11, &e, &[f, gh, ad, a, b, o, o]));
        assert_eq!(tree, vec![ah, ad, eh, o, o, ef, gh, o, o, o, o, e, f, o, o]);
    }
}
