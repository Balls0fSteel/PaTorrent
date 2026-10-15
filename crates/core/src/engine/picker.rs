// Ported from libtorrent-rasterbar 2.0.11: src/piece_picker.cpp, src/request_blocks.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: libtorrent keeps every piece in a bucketed vector ordered by priority and
// availability and moves pieces between buckets on each HAVE. This version keeps per-piece
// counters plus a pick order (priority, then rarest, then random) that is rebuilt lazily, at most
// once per `REBUILD_INTERVAL` while availability churns. Partial pieces are kept in a map with
// per-block states, as libtorrent's `downloading_piece`. Same picking policy: partial pieces
// first, then rarest-first by priority (or in order when sequential), then end-game.

//! Which blocks to request from which peer.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::util::bitfield::Bitfield;
use crate::util::random::shuffle;

const REBUILD_INTERVAL: Duration = Duration::from_millis(500);

/// libtorrent allows a block to be requested from several peers only in end-game. This caps
/// how many peers may be asked for the same block at once.
const MAX_ENDGAME_REQUESTS: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockState {
    Free,
    /// Requested from this many peers.
    Requested(u8),
    /// Received and handed to the disk.
    Writing,
    /// On disk.
    Finished,
}

#[derive(Debug)]
struct Downloading {
    blocks: Vec<BlockState>,
}

impl Downloading {
    fn count(&self, s: impl Fn(BlockState) -> bool) -> usize {
        self.blocks.iter().filter(|&&b| s(b)).count()
    }
}

pub(crate) struct PiecePicker {
    blocks_per_piece: u32,
    last_piece_blocks: u32,
    have: Bitfield,
    /// Peers (other than seeds) that have each piece.
    availability: Vec<u16>,
    /// Connected seeds, counted once instead of on every piece (libtorrent `m_seeds`).
    seeds: u32,
    priority: Vec<u8>,
    downloading: HashMap<u32, Downloading>,
    /// Pieces we still want, best first. Rebuilt lazily.
    order: Vec<u32>,
    order_dirty: bool,
    order_built: Option<Instant>,
    sequential: bool,
}

impl PiecePicker {
    pub fn new(num_pieces: u32, piece_length: u32, last_piece_size: u32, block_size: u32) -> PiecePicker {
        let n = num_pieces as usize;
        PiecePicker {
            blocks_per_piece: piece_length.div_ceil(block_size),
            last_piece_blocks: last_piece_size.div_ceil(block_size),
            have: Bitfield::new(n, false),
            availability: vec![0; n],
            seeds: 0,
            priority: vec![4; n],
            downloading: HashMap::new(),
            order: Vec::new(),
            order_dirty: true,
            order_built: None,
            sequential: false,
        }
    }

    pub fn num_pieces(&self) -> u32 {
        self.have.len() as u32
    }

    pub fn blocks_in_piece(&self, piece: u32) -> u32 {
        if piece + 1 == self.num_pieces() { self.last_piece_blocks } else { self.blocks_per_piece }
    }

    pub fn have(&self) -> &Bitfield {
        &self.have
    }

    pub fn have_piece(&self, piece: u32) -> bool {
        self.have.get(piece as usize)
    }

    pub fn set_sequential(&mut self, on: bool) {
        if self.sequential != on {
            self.sequential = on;
            self.order_dirty = true;
            self.order_built = None;
        }
    }

    pub fn piece_priority(&self, piece: u32) -> u8 {
        self.priority[piece as usize]
    }

    /// Returns true if the priority changed.
    pub fn set_piece_priority(&mut self, piece: u32, prio: u8) -> bool {
        let p = &mut self.priority[piece as usize];
        if *p == prio {
            return false;
        }
        *p = prio;
        self.order_dirty = true;
        self.order_built = None;
        true
    }

    /// Wanted pieces we have / all wanted pieces.
    #[cfg(test)]
    pub fn wanted_progress(&self) -> (u32, u32) {
        let mut have = 0;
        let mut wanted = 0;
        for (i, &p) in self.priority.iter().enumerate() {
            if p > 0 {
                wanted += 1;
                if self.have.get(i) {
                    have += 1;
                }
            }
        }
        (have, wanted)
    }

    /// True when every piece with a non-zero priority is done (libtorrent `is_finished`).
    pub fn is_finished(&self) -> bool {
        self.priority.iter().enumerate().all(|(i, &p)| p == 0 || self.have.get(i))
    }

    /// Swarm copies of each piece (connected peers plus seeds).
    pub fn availability(&self, piece: u32) -> u32 {
        self.availability[piece as usize] as u32 + self.seeds
    }

    /// libtorrent `distributed_copies`: the number of complete copies in the connected swarm,
    /// with the fraction of pieces above the minimum as decimals.
    pub fn distributed_copies(&self) -> f32 {
        if self.availability.is_empty() {
            return 0.0;
        }
        let min = *self.availability.iter().min().expect("non-empty");
        let above = self.availability.iter().filter(|&&a| a > min).count();
        (min as u32 + self.seeds) as f32 + above as f32 / self.availability.len() as f32
    }

    // --- Availability -------------------------------------------------------------------------

    pub fn inc_refcount(&mut self, piece: u32) {
        let a = &mut self.availability[piece as usize];
        *a = a.saturating_add(1);
        self.order_dirty = true;
    }

    pub fn inc_refcount_bitfield(&mut self, bits: &Bitfield) {
        for (i, a) in self.availability.iter_mut().enumerate() {
            if bits.get(i) {
                *a = a.saturating_add(1);
            }
        }
        self.order_dirty = true;
    }

    pub fn dec_refcount_bitfield(&mut self, bits: &Bitfield) {
        for (i, a) in self.availability.iter_mut().enumerate() {
            if bits.get(i) {
                *a = a.saturating_sub(1);
            }
        }
        self.order_dirty = true;
    }

    pub fn inc_seed(&mut self) {
        self.seeds += 1;
    }

    pub fn dec_seed(&mut self) {
        self.seeds = self.seeds.saturating_sub(1);
    }

    // --- Interest -----------------------------------------------------------------------------

    /// Whether a peer with `bits` (or a seed) has anything we want.
    pub fn is_interesting(&self, bits: &Bitfield, peer_is_seed: bool) -> bool {
        (0..self.num_pieces()).any(|p| self.wants(p) && (peer_is_seed || bits.get(p as usize)))
    }

    pub fn wants(&self, piece: u32) -> bool {
        !self.have.get(piece as usize) && self.priority[piece as usize] > 0
    }

    // --- Picking ------------------------------------------------------------------------------

    fn rebuild_order(&mut self) {
        let now = Instant::now();
        let fresh = self.order_built.is_some_and(|t| now.duration_since(t) < REBUILD_INTERVAL);
        if !self.order_dirty || (fresh && !self.order.is_empty()) {
            return;
        }
        let mut order = std::mem::take(&mut self.order);
        order.clear();
        order.extend((0..self.num_pieces()).filter(|&p| self.wants(p)));
        self.order = order;
        if self.sequential {
            // Priority still wins; within one priority, in piece order.
            let prio = &self.priority;
            self.order.sort_by_key(|&p| std::cmp::Reverse(prio[p as usize]));
        } else {
            // Random tie-break among equally rare pieces (stable sort after a shuffle).
            shuffle(&mut self.order);
            let (prio, avail) = (&self.priority, &self.availability);
            self.order.sort_by_key(|&p| (std::cmp::Reverse(prio[p as usize]), avail[p as usize]));
        }
        self.order_dirty = false;
        self.order_built = Some(now);
    }

    /// libtorrent `piece_picker::pick_pieces` + `request_a_block`. Picks up to `num_blocks`
    /// blocks from pieces the peer has and marks them requested. `already_requested(piece,
    /// block)` tells whether this peer already has the block in its queue (only consulted in
    /// end-game, where blocks are requested from several peers).
    pub fn pick(
        &mut self,
        peer_has: &Bitfield,
        peer_is_seed: bool,
        num_blocks: usize,
        already_requested: impl Fn(u32, u32) -> bool,
    ) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        if num_blocks == 0 {
            return out;
        }
        let has = |p: u32| peer_is_seed || peer_has.get(p as usize);

        // 1. Finish partial pieces first, the ones closest to done first.
        let mut partial: Vec<(u32, usize)> = self
            .downloading
            .iter()
            .filter(|(p, _)| self.priority[**p as usize] > 0 && has(**p))
            .map(|(p, d)| (*p, d.count(|b| b == BlockState::Free)))
            .filter(|&(_, free)| free > 0)
            .collect();
        partial.sort_by_key(|&(p, free)| (free, p));
        for (piece, _) in partial {
            self.take_free_blocks(piece, num_blocks, &mut out);
            if out.len() >= num_blocks {
                return out;
            }
        }

        // 2. New pieces, best first.
        self.rebuild_order();
        let mut i = 0;
        while i < self.order.len() && out.len() < num_blocks {
            let piece = self.order[i];
            i += 1;
            if !self.wants(piece) || self.downloading.contains_key(&piece) || !has(piece) {
                continue;
            }
            let n = self.blocks_in_piece(piece);
            self.downloading.insert(piece, Downloading { blocks: vec![BlockState::Free; n as usize] });
            self.take_free_blocks(piece, num_blocks, &mut out);
        }
        if !out.is_empty() {
            return out;
        }

        // 3. End-game: nothing free is left that this peer has, so ask it for blocks that are
        //    already requested from others (libtorrent requests busy blocks the same way).
        let mut busy: Vec<(u32, u32)> = Vec::new();
        for (&piece, d) in &self.downloading {
            if self.priority[piece as usize] == 0 || !has(piece) {
                continue;
            }
            for (b, &s) in d.blocks.iter().enumerate() {
                if let BlockState::Requested(n) = s {
                    if n < MAX_ENDGAME_REQUESTS && !already_requested(piece, b as u32) {
                        busy.push((piece, b as u32));
                    }
                }
            }
        }
        busy.sort_unstable();
        for (piece, b) in busy.into_iter().take(num_blocks) {
            if let Some(d) = self.downloading.get_mut(&piece) {
                if let BlockState::Requested(n) = d.blocks[b as usize] {
                    d.blocks[b as usize] = BlockState::Requested(n + 1);
                    out.push((piece, b));
                }
            }
        }
        out
    }

    fn take_free_blocks(&mut self, piece: u32, num_blocks: usize, out: &mut Vec<(u32, u32)>) {
        let Some(d) = self.downloading.get_mut(&piece) else { return };
        for (b, s) in d.blocks.iter_mut().enumerate() {
            if out.len() >= num_blocks {
                break;
            }
            if *s == BlockState::Free {
                *s = BlockState::Requested(1);
                out.push((piece, b as u32));
            }
        }
    }

    /// True when the picker has no free block left in any wanted piece (libtorrent end-game).
    pub fn in_endgame(&self) -> bool {
        let all_started = (0..self.num_pieces()).all(|p| !self.wants(p) || self.downloading.contains_key(&p));
        all_started && self.downloading.values().all(|d| d.count(|b| b == BlockState::Free) == 0)
    }

    #[cfg(test)]
    pub fn block_state(&self, piece: u32, block: u32) -> BlockState {
        if self.have_piece(piece) {
            return BlockState::Finished;
        }
        self.downloading
            .get(&piece)
            .and_then(|d| d.blocks.get(block as usize).copied())
            .unwrap_or(BlockState::Free)
    }

    /// A request was cancelled, rejected or timed out (libtorrent `abort_download`).
    pub fn abort_download(&mut self, piece: u32, block: u32) {
        if let Some(s) = self.downloading.get_mut(&piece).and_then(|d| d.blocks.get_mut(block as usize)) {
            if let BlockState::Requested(n) = *s {
                *s = if n > 1 { BlockState::Requested(n - 1) } else { BlockState::Free };
            }
        }
        self.drop_if_untouched(piece);
    }

    fn drop_if_untouched(&mut self, piece: u32) {
        if self.downloading.get(&piece).is_some_and(|d| d.blocks.iter().all(|&b| b == BlockState::Free)) {
            self.downloading.remove(&piece);
        }
    }

    /// The block arrived. Returns false if it is not wanted any more (already writing or done):
    /// the data is redundant and must be dropped.
    pub fn mark_as_writing(&mut self, piece: u32, block: u32) -> bool {
        if self.have_piece(piece) {
            return false;
        }
        let n = self.blocks_in_piece(piece);
        let d = self
            .downloading
            .entry(piece)
            .or_insert_with(|| Downloading { blocks: vec![BlockState::Free; n as usize] });
        match d.blocks.get_mut(block as usize) {
            Some(s @ (BlockState::Free | BlockState::Requested(_))) => {
                *s = BlockState::Writing;
                true
            }
            _ => false,
        }
    }

    pub fn mark_as_finished(&mut self, piece: u32, block: u32) {
        if let Some(s) = self.downloading.get_mut(&piece).and_then(|d| d.blocks.get_mut(block as usize)) {
            *s = BlockState::Finished;
        }
    }

    /// The disk failed to write the block; it must be downloaded again.
    pub fn write_failed(&mut self, piece: u32, block: u32) {
        if let Some(s) = self.downloading.get_mut(&piece).and_then(|d| d.blocks.get_mut(block as usize)) {
            *s = BlockState::Free;
        }
        self.drop_if_untouched(piece);
    }

    /// The piece hashed correctly (libtorrent `piece_passed` + `we_have`).
    pub fn piece_passed(&mut self, piece: u32) {
        self.downloading.remove(&piece);
        if !self.have.get(piece as usize) {
            self.have.set(piece as usize, true);
            self.order_dirty = true;
        }
    }

    /// The piece failed its hash check: every block must be downloaded again
    /// (libtorrent `restore_piece`).
    pub fn piece_failed(&mut self, piece: u32) {
        self.downloading.remove(&piece);
        self.order_dirty = true;
    }

    /// Replaces our piece set wholesale (after checking files) and forgets partial pieces.
    pub fn set_have(&mut self, have: &Bitfield) {
        debug_assert_eq!(have.len(), self.have.len());
        self.have = have.clone();
        self.downloading.clear();
        self.order_dirty = true;
        self.order_built = None;
    }

    /// Forgets all partial pieces (when the torrent stops and its disk state is released).
    /// Returns the pieces that had blocks on disk.
    pub fn clear_downloading(&mut self) -> Vec<u32> {
        let touched = self
            .downloading
            .iter()
            .filter(|(_, d)| {
                d.blocks.iter().any(|&b| matches!(b, BlockState::Writing | BlockState::Finished))
            })
            .map(|(&p, _)| p)
            .collect();
        self.downloading.clear();
        self.order_dirty = true;
        touched
    }

    /// Bytes of finished blocks in partial pieces (for `total_done`).
    pub fn partial_finished_blocks(&self) -> usize {
        self.downloading.values().map(|d| d.count(|b| b == BlockState::Finished)).sum()
    }

    #[cfg(any(debug_assertions, test))]
    #[allow(dead_code)]
    pub fn check_invariant(&self) {
        for (&p, d) in &self.downloading {
            assert!(!self.have.get(p as usize), "piece {p} is both had and downloading");
            assert_eq!(d.blocks.len() as u32, self.blocks_in_piece(p));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: u32 = 16 * 1024;

    fn bits(s: &str) -> Bitfield {
        let mut bf = Bitfield::new(s.len(), false);
        for (i, c) in s.chars().enumerate() {
            bf.set(i, c == '1');
        }
        bf
    }

    fn picker(n: u32) -> PiecePicker {
        // 2 blocks per piece, last piece 1 block.
        PiecePicker::new(n, 2 * B, B, B)
    }

    #[test]
    fn rarest_first() {
        let mut p = picker(4);
        p.inc_refcount_bitfield(&bits("1111"));
        p.inc_refcount_bitfield(&bits("1101"));
        p.inc_refcount_bitfield(&bits("0100"));
        // Availability: 2 3 1 2. Piece 2 is rarest.
        let all = bits("1111");
        let picked = p.pick(&all, false, 2, |_, _| false);
        assert_eq!(picked, [(2, 0), (2, 1)]);
        // Next: pieces 0 and 3 (availability 2), then 1.
        let picked = p.pick(&all, false, 4, |_, _| false);
        let pieces: Vec<u32> = picked.iter().map(|b| b.0).collect();
        assert!(pieces[..3].iter().all(|&x| x == 0 || x == 3), "{picked:?}");
        assert_eq!(picked.len(), 4);
        p.check_invariant();
    }

    #[test]
    fn priorities_partials_and_sequential() {
        let mut p = picker(6);
        let all = bits("111111");
        p.set_piece_priority(0, 0);
        p.set_piece_priority(4, 7);
        assert_eq!(p.pick(&all, false, 1, |_, _| false), [(4, 0)], "highest priority first");
        // The partial piece is finished before anything new starts.
        assert_eq!(p.pick(&all, false, 1, |_, _| false), [(4, 1)]);

        let mut p = picker(6);
        p.set_sequential(true);
        p.set_piece_priority(0, 0);
        let picked = p.pick(&all, true, 5, |_, _| false);
        assert_eq!(picked, [(1, 0), (1, 1), (2, 0), (2, 1), (3, 0)]);
        // Only pieces the peer has.
        let mut p = picker(6);
        assert_eq!(p.pick(&bits("000010"), false, 9, |_, _| false), [(4, 0), (4, 1)]);
        assert!(p.pick(&bits("000000"), false, 9, |_, _| false).is_empty());
    }

    #[test]
    fn block_lifecycle_and_hash_failure() {
        let mut p = picker(2);
        let all = bits("11");
        let picked = p.pick(&all, true, 3, |_, _| false);
        assert_eq!(picked.len(), 3);
        assert!(p.in_endgame());
        // A rejected block becomes free again.
        p.abort_download(1, 0);
        assert_eq!(p.block_state(1, 0), BlockState::Free);
        assert!(!p.in_endgame());
        assert_eq!(p.pick(&all, true, 3, |_, _| false), [(1, 0)]);

        for (piece, block) in [(0, 0), (0, 1), (1, 0)] {
            assert!(p.mark_as_writing(piece, block));
            assert!(!p.mark_as_writing(piece, block), "duplicates are redundant");
            p.mark_as_finished(piece, block);
        }
        p.piece_failed(0);
        assert_eq!(p.block_state(0, 0), BlockState::Free);
        p.piece_passed(1);
        assert!(p.have_piece(1) && !p.is_finished());
        assert_eq!(p.wanted_progress(), (1, 2));
        assert_eq!(p.pick(&all, true, 9, |_, _| false), [(0, 0), (0, 1)]);
        p.piece_passed(0);
        assert!(p.is_finished() && p.have().all_set());
        assert!(!p.is_interesting(&all, true));
    }

    #[test]
    fn endgame_requests_busy_blocks_from_other_peers() {
        let mut p = picker(1);
        let all = bits("1");
        // One block total; peer A has it requested.
        assert_eq!(p.pick(&all, true, 5, |_, _| false), [(0, 0)]);
        // Peer B gets the same block in end-game, peer A does not get it twice.
        assert_eq!(p.pick(&all, true, 5, |_, _| false), [(0, 0)]);
        assert_eq!(p.block_state(0, 0), BlockState::Requested(2));
        assert!(p.pick(&all, true, 5, |_, _| false).is_empty(), "capped at two peers");
        p.abort_download(0, 0);
        assert!(p.pick(&all, true, 5, |piece, block| (piece, block) == (0, 0)).is_empty());
    }

    #[test]
    fn availability_and_copies() {
        let mut p = picker(4);
        p.inc_seed();
        p.inc_refcount_bitfield(&bits("1100"));
        assert_eq!(p.availability(0), 2);
        assert_eq!(p.availability(3), 1);
        assert!((p.distributed_copies() - 1.5).abs() < 1e-6);
        p.dec_refcount_bitfield(&bits("1100"));
        p.dec_seed();
        assert_eq!(p.distributed_copies(), 0.0);
        assert!(p.is_interesting(&bits("0001"), false));
        assert!(!p.is_interesting(&bits("0000"), false));
    }
}
