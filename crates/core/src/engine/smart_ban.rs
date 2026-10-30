// Ported from libtorrent-rasterbar 2.0.11: src/smart_ban.cpp, src/torrent.cpp (piece_failed,
// penalize_peers, the trust points of piece_passed, ban_peer)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the plugin is a struct on the torrent, driven by engine methods, and its disk reads
// come back through the engine's read completions (cookies with the top bit set) instead of
// bound callbacks, so the block's downloader and the entry under test wait beside the map until
// the read returns. Peers are named by address (libtorrent: `torrent_peer*`).

//! Banning peers that send bad data: the trust points of `torrent::penalize_peers`, and the
//! block-level blame of the smart_ban plugin, which hashes the blocks of a failed piece per
//! peer and, once the piece passes, bans the peers whose blocks differed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;

use super::Engine;
use super::peer::PeerState;
use super::peer_list::KnownPeer;
use crate::alert::Alert;
use crate::crypto::Sha1;
use crate::disk::BLOCK_SIZE;
use crate::types::{Sha1Hash, TorrentId};

/// Disk read cookies of the plugin's reads. Peer reads use connection ids, whose top bit is
/// never set (the generation counter stays below 2^31).
pub(super) const SMART_BAN_COOKIE: u64 = 1 << 63;
/// Set on reads of a passed piece (`on_read_ok_block`); clear on reads of a failed one.
const OK_BLOCK: u64 = 1 << 62;

/// libtorrent `penalize_peers`: the trust points lost per failed piece, their floor, and the
/// cap on the fail counter.
const TRUST_LOSS: i8 = 2;
const MIN_TRUST: i8 = -7;
const MAX_TRUST: i8 = 8;

/// A peer's hash failures and trust, counted on the peer-list entry (libtorrent
/// `torrent_peer::trust_points`, `hashfails`, `on_parole`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Trust {
    /// Starts at 0, loses 2 per failed piece it took part in (floor -7) and gains 1 per passed
    /// one (cap 8). At -7 the peer is banned.
    pub trust_points: i8,
    pub hashfails: u8,
    /// The peer sent part of a piece that failed: until it delivers a whole piece that passes,
    /// it only downloads pieces alone, so the next failure is clearly its own.
    pub on_parole: bool,
}

impl Trust {
    /// libtorrent `penalize_peers`, for one peer. Returns true if the peer is to be banned:
    /// it lost all trust, or it is known to have sent bad data (`known_bad_peer`: the only
    /// sender of the failed piece).
    pub fn failed(&mut self, known_bad_peer: bool, use_parole_mode: bool) -> bool {
        if use_parole_mode {
            self.on_parole = true;
        }
        // we decrease more than we increase, to keep the allowed failed/passed ratio low
        self.trust_points = (self.trust_points - TRUST_LOSS).max(MIN_TRUST);
        // capped at 255, the width of libtorrent's counter
        self.hashfails = self.hashfails.saturating_add(1);
        self.trust_points <= MIN_TRUST || known_bad_peer
    }

    /// libtorrent `piece_passed`: a piece the peer took part in checked out.
    pub fn passed(&mut self) {
        self.on_parole = false;
        self.trust_points = (self.trust_points + 1).min(MAX_TRUST);
    }
}

/// libtorrent `block_entry`: ties a block's SHA-1 to the peer that sent it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BlockEntry {
    peer: SocketAddr,
    digest: Sha1Hash,
}

/// The smart_ban plugin's state for one torrent.
#[derive(Default)]
pub(crate) struct SmartBan {
    /// libtorrent `m_block_hashes`: (piece, block) → who sent it and what it hashed to, for
    /// pieces that failed.
    block_hashes: BTreeMap<(u32, u32), BlockEntry>,
    /// Blocks of a failed piece being read back, with their downloader.
    failed_reads: HashMap<(u32, u32), SocketAddr>,
    /// Entries of a piece that passed, being read back for comparison.
    ok_reads: HashMap<(u32, u32), BlockEntry>,
}

/// What to do about a block read back from disk.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Nothing,
    Ban(SocketAddr),
}

impl SmartBan {
    /// libtorrent `on_piece_failed`: the blocks to read back and hash, one per block with a
    /// known downloader.
    pub fn on_piece_failed(&mut self, piece: u32, downloaders: &[Option<SocketAddr>]) -> Vec<u32> {
        let mut reads = Vec::new();
        for (block, peer) in downloaders.iter().enumerate() {
            if let Some(peer) = peer {
                self.failed_reads.insert((piece, block as u32), *peer);
                reads.push(block as u32);
            }
        }
        reads
    }

    /// libtorrent `on_piece_pass`: the blocks of the passed piece recorded at an earlier
    /// failure, to read back and compare. Their entries leave the map.
    pub fn on_piece_pass(&mut self, piece: u32) -> Vec<u32> {
        let keys: Vec<(u32, u32)> =
            self.block_hashes.range((piece, 0)..(piece + 1, 0)).map(|(k, _)| *k).collect();
        let mut reads = Vec::with_capacity(keys.len());
        for key in keys {
            let e = self.block_hashes.remove(&key).expect("listed");
            self.ok_reads.insert(key, e);
            reads.push(key.1);
        }
        reads
    }

    /// libtorrent `on_read_failed_block`. `peer_state` tells whether the downloader is still
    /// in the peer list, and whether it is banned.
    pub fn on_read_failed_block(
        &mut self,
        piece: u32,
        block: u32,
        data: &[u8],
        peer_state: impl Fn(SocketAddr) -> Option<bool>,
    ) -> Verdict {
        let Some(peer) = self.failed_reads.remove(&(piece, block)) else { return Verdict::Nothing };
        // there is no peer with this address anymore
        let Some(banned) = peer_state(peer) else { return Verdict::Nothing };
        let e = BlockEntry { peer, digest: Sha1::digest(data) };
        match self.block_hashes.get(&(piece, block)) {
            Some(prev) if prev.peer == peer => {
                // this peer has sent us this block before. If the peer is already banned, it
                // doesn't matter if it sent good or bad data. Nothing's going to change it
                if !banned && prev.digest != e.digest {
                    // this time the digest of the block is different from the first time it
                    // sent it: at least one of them must be bad
                    return Verdict::Ban(peer);
                }
                Verdict::Nothing
            }
            // libtorrent inserts with a hint into a std::map: an entry from another peer
            // stays, and a new block is recorded
            Some(_) => Verdict::Nothing,
            None => {
                self.block_hashes.insert((piece, block), e);
                Verdict::Nothing
            }
        }
    }

    /// libtorrent `on_read_ok_block`: the block of the good piece against what the peer sent.
    pub fn on_read_ok_block(&mut self, piece: u32, block: u32, data: &[u8]) -> Verdict {
        let Some(e) = self.ok_reads.remove(&(piece, block)) else { return Verdict::Nothing };
        if e.digest == Sha1::digest(data) { Verdict::Nothing } else { Verdict::Ban(e.peer) }
    }

    /// A read came back with an error: ignored, as libtorrent does.
    pub fn read_failed(&mut self, piece: u32, block: u32) {
        self.failed_reads.remove(&(piece, block));
        self.ok_reads.remove(&(piece, block));
    }

    /// Once we are a seed there is nothing left to blame (libtorrent swaps the map away).
    /// Reads in flight keep what they need: libtorrent binds it into their callbacks.
    pub fn clear(&mut self) {
        self.block_hashes.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.block_hashes.len()
    }
}

impl Engine {
    /// libtorrent `torrent::piece_failed` after the picker restored the piece: count the
    /// failed bytes, let smart_ban read the blocks back, and penalize the peers that sent them.
    /// `downloaders` is the picker's `get_downloaders` from before the restore.
    pub(super) fn on_piece_failed(
        &mut self,
        id: TorrentId,
        piece: u32,
        downloaders: Vec<Option<SocketAddr>>,
    ) {
        let use_parole_mode = self.settings.use_parole_mode;
        let Some(t) = self.torrent(id) else { return };
        let peers: HashSet<SocketAddr> = downloaders.iter().flatten().copied().collect();
        // if this piece wasn't downloaded from peers, we just found it on disk. In that case,
        // we should just consider it as "not-have"
        if peers.is_empty() {
            return;
        }
        t.failed_bytes += t.piece_size(piece);
        if let Some(storage) = t.storage.clone() {
            let size = t.piece_size_for_req(piece);
            for block in t.smart_ban.get_or_insert_with(Box::default).on_piece_failed(piece, &downloaders) {
                let start = block * BLOCK_SIZE;
                let len = (size - start as u64).min(BLOCK_SIZE as u64) as u32;
                self.disk.async_read(&storage, piece, start, len, SMART_BAN_COOKIE);
            }
        }
        // did we receive this piece from a single peer? Then we know it sent bad data.
        let known_bad_peer = peers.len() == 1;
        let mut banned = Vec::new();
        for addr in peers {
            let Some(t) = self.torrent(id) else { return };
            match t.peer_list.get_mut(&addr) {
                Some(p) => {
                    if p.trust.failed(known_bad_peer, use_parole_mode) {
                        banned.push(addr);
                    }
                }
                None => {
                    self.web_seed_hash_failed(id, piece, addr, known_bad_peer, use_parole_mode);
                }
            }
        }
        for addr in banned {
            self.ban_peer(id, addr, "too many corrupt pieces");
        }
    }

    /// libtorrent `torrent::piece_passed`: the peers that sent parts of the piece regain
    /// trust and leave parole; smart_ban compares the blocks recorded at an earlier failure.
    pub(super) fn on_piece_pass(&mut self, id: TorrentId, piece: u32, downloaders: Vec<Option<SocketAddr>>) {
        let Some(t) = self.torrent(id) else { return };
        let peers: HashSet<SocketAddr> = downloaders.iter().flatten().copied().collect();
        for addr in &peers {
            if let Some(p) = t.peer_list.get_mut(addr) {
                p.trust.passed();
            }
        }
        self.web_seeds_passed(id, &peers);
        let Some(t) = self.torrent(id) else { return };
        let reads = t.smart_ban.as_mut().map_or_else(Vec::new, |s| s.on_piece_pass(piece));
        if let Some(storage) = t.storage.clone().filter(|_| !reads.is_empty()) {
            let size = t.piece_size_for_req(piece);
            for block in reads {
                let start = block * BLOCK_SIZE;
                let len = (size - start as u64).min(BLOCK_SIZE as u64) as u32;
                self.disk.async_read(&storage, piece, start, len, SMART_BAN_COOKIE | OK_BLOCK);
            }
        }
        if let Some(t) = self.torrent(id).filter(|t| t.is_seed()) {
            if let Some(s) = t.smart_ban.as_mut() {
                s.clear();
            }
        }
    }

    /// A block read back for smart_ban (libtorrent `on_read_failed_block` / `on_read_ok_block`).
    pub(super) fn on_smart_ban_read(
        &mut self,
        storage: u64,
        cookie: u64,
        piece: u32,
        offset: u32,
        result: Result<Vec<u8>, crate::error::Error>,
    ) {
        let Some(&id) = self.storages.get(&storage) else { return };
        let Some(t) = self.torrent(id) else { return };
        if t.storage.as_ref().is_none_or(|s| s.id != storage) {
            return;
        }
        let block = offset / BLOCK_SIZE;
        let data = match result {
            Ok(d) => d,
            Err(_) => {
                t.smart_ban.get_or_insert_with(Box::default).read_failed(piece, block);
                return;
            }
        };
        let verdict = if cookie & OK_BLOCK != 0 {
            t.smart_ban.get_or_insert_with(Box::default).on_read_ok_block(piece, block, &data)
        } else {
            let list = &t.peer_list;
            let peer_state = |addr: SocketAddr| list.get(&addr).map(|p| p.banned);
            t.smart_ban.get_or_insert_with(Box::default).on_read_failed_block(piece, block, &data, peer_state)
        };
        if let Verdict::Ban(addr) = verdict {
            self.ban_peer(id, addr, &format!("sent a bad block of piece {piece} (block {block})"));
        }
    }

    /// libtorrent `torrent::ban_peer` plus the disconnect that follows it: the peer is never
    /// connected again, and any connection to it is closed. A peer that is unknown or already
    /// banned is left alone.
    pub(super) fn ban_peer(&mut self, id: TorrentId, addr: SocketAddr, reason: &str) {
        let Some(t) = self.torrent(id) else { return };
        if t.peer_list.get(&addr).is_none_or(|p| p.banned) {
            return;
        }
        t.peer_list.ban(&addr);
        let mut peer_id = [0u8; 20];
        for slot in t.peers.clone() {
            let Some(p) = self.peers.get_mut(slot) else { continue };
            if p.addr == addr && p.closing.is_none() {
                if p.state == PeerState::Active {
                    peer_id = p.peer_id;
                }
                p.close(format!("banned: {reason}"), true);
            }
        }
        self.alert(Alert::Log { id: Some(id), message: format!("banning peer {addr}: {reason}") });
        self.alert(Alert::PeerBanned { id, addr, peer_id });
    }

    /// The trust bookkeeping of a known peer, for `peer_info`.
    pub(super) fn trust_of(t: Option<&super::torrent::Torrent>, addr: &SocketAddr) -> Trust {
        t.and_then(|t| t.peer_list.get(addr)).map_or(Trust::default(), |p: &KnownPeer| p.trust)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1)), 6881);
    const C: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 2)), 6881);

    #[test]
    fn trust_points() {
        let mut t = Trust::default();
        assert!(!t.failed(false, true));
        assert_eq!((t.trust_points, t.hashfails, t.on_parole), (-2, 1, true));
        t.passed();
        assert_eq!((t.trust_points, t.on_parole), (-1, false));
        // Four failures in a row (-2 each, floor -7) exhaust the trust.
        assert!(!t.failed(false, false) && !t.failed(false, false));
        assert!(!t.on_parole, "parole mode off");
        assert!(t.failed(false, false), "trust gone");
        assert_eq!(t.trust_points, -7);
        // The only sender of a bad piece is banned at once.
        let mut t = Trust::default();
        assert!(t.failed(true, true));
        // Trust caps at 8.
        let mut t = Trust::default();
        for _ in 0..20 {
            t.passed();
        }
        assert_eq!(t.trust_points, 8);
        for _ in 0..255 {
            t.failed(false, true);
        }
        assert_eq!(t.hashfails, 255);
    }

    // The smart_ban plugin's rule: a peer whose block differs from the good piece is banned,
    // one whose block matches is cleared.
    #[test]
    fn bans_the_peer_whose_block_differed() {
        let mut sb = SmartBan::default();
        let (good0, good1, bad1) = (vec![1u8; 100], vec![2u8; 100], vec![9u8; 100]);
        // Piece 3 failed: block 0 from A (good), block 1 from C (bad); block 2 from nobody.
        assert_eq!(sb.on_piece_failed(3, &[Some(A), Some(C), None]), [0, 1]);
        let alive = |_: SocketAddr| Some(false);
        assert_eq!(sb.on_read_failed_block(3, 0, &good0, alive), Verdict::Nothing);
        assert_eq!(sb.on_read_failed_block(3, 1, &bad1, alive), Verdict::Nothing);
        assert_eq!(sb.len(), 2);
        // Another piece's entries are untouched by piece 3 passing.
        assert_eq!(sb.on_piece_failed(4, &[Some(C)]), [0]);
        assert_eq!(sb.on_read_failed_block(4, 0, &bad1, alive), Verdict::Nothing);
        // Piece 3 passes: both recorded blocks are compared.
        assert_eq!(sb.on_piece_pass(3), [0, 1]);
        assert_eq!(sb.len(), 1);
        assert_eq!(sb.on_read_ok_block(3, 0, &good0), Verdict::Nothing);
        assert_eq!(sb.on_read_ok_block(3, 1, &good1), Verdict::Ban(C));
        assert!(sb.on_piece_pass(3).is_empty(), "entries leave the map");
        assert_eq!(sb.on_read_ok_block(3, 1, &good1), Verdict::Nothing, "no entry waiting");
        assert!(sb.on_piece_pass(5).is_empty());
    }

    #[test]
    fn a_peer_that_sends_two_versions_of_a_block_is_banned() {
        let mut sb = SmartBan::default();
        let alive = |_: SocketAddr| Some(false);
        sb.on_piece_failed(0, &[Some(A)]);
        assert_eq!(sb.on_read_failed_block(0, 0, b"first", alive), Verdict::Nothing);
        // The same block again, different data: one of the two was bad.
        sb.on_piece_failed(0, &[Some(A)]);
        assert_eq!(sb.on_read_failed_block(0, 0, b"second", alive), Verdict::Ban(A));
        // Identical data again: nothing new. A banned peer is not banned twice.
        sb.on_piece_failed(0, &[Some(A)]);
        assert_eq!(sb.on_read_failed_block(0, 0, b"first", alive), Verdict::Nothing);
        sb.on_piece_failed(0, &[Some(A)]);
        assert_eq!(sb.on_read_failed_block(0, 0, b"third", |_| Some(true)), Verdict::Nothing);
        // Another peer's version of a recorded block is not stored over it (libtorrent's
        // hinted map insert keeps the first entry); a vanished peer is ignored.
        sb.on_piece_failed(0, &[Some(C)]);
        assert_eq!(sb.on_read_failed_block(0, 0, b"other", alive), Verdict::Nothing);
        assert_eq!(sb.len(), 1);
        sb.on_piece_failed(1, &[Some(C)]);
        assert_eq!(sb.on_read_failed_block(1, 0, b"x", |_| None), Verdict::Nothing);
        assert_eq!(sb.len(), 1);
        // Read errors drop the pending block; `clear` forgets the recorded hashes but not the
        // reads in flight.
        sb.on_piece_failed(2, &[Some(C)]);
        sb.read_failed(2, 0);
        assert_eq!(sb.on_read_failed_block(2, 0, b"x", alive), Verdict::Nothing);
        assert_eq!(sb.on_piece_pass(0), [0]);
        sb.clear();
        assert_eq!(sb.len(), 0);
        assert_eq!(sb.on_read_ok_block(0, 0, b"good"), Verdict::Ban(A));
    }
}
