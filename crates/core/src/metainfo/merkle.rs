// Ported from libtorrent-rasterbar 2.0.11: src/merkle.cpp (tree size helpers)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.

//! Merkle tree geometry for BitTorrent v2 (the full tree code arrives in Phase 8).

/// Leaves in a tree over `blocks` items: the next power of two (at least 1).
pub fn merkle_num_leafs(blocks: u32) -> u32 {
    blocks.max(1).next_power_of_two()
}

/// Total nodes in a full binary tree with `leafs` leaves.
pub fn merkle_num_nodes(leafs: u32) -> u32 {
    debug_assert!(leafs.is_power_of_two());
    leafs * 2 - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry() {
        assert_eq!(merkle_num_leafs(0), 1);
        assert_eq!(merkle_num_leafs(1), 1);
        assert_eq!(merkle_num_leafs(80), 128);
        assert_eq!(merkle_num_leafs(128), 128);
        assert_eq!(merkle_num_nodes(128), 255);
        assert_eq!(merkle_num_nodes(1), 1);
    }
}
