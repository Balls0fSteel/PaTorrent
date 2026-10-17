// Ported from libtorrent-rasterbar 2.0.11: src/torrent.cpp (set_super_seeding,
// get_piece_to_super_seed, the super-seeding check of files_checked), src/peer_connection.cpp
// (superseed_piece, the super-seeding branches of incoming_have, incoming_request and
// second_tick), src/bt_peer_connection.cpp (write_bitfield while super seeding)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: engine methods taking a peer slot. Only peers that got the super-seeding handshake
// (`PeerConn::superseed` is `Some`) are limited to their offered pieces: a peer connected before
// super seeding was switched on keeps our full piece set, where libtorrent would start rejecting
// its requests. Switching it off sends the real piece set to every super-seeded peer, also one
// with no offer left (libtorrent skips those). libtorrent's per-second re-offer to an
// uninterested peer runs from our session tick, rate-limited to once a second. The deprecated
// `strict_super_seeding` setting (re-offer when any peer reports the piece) is not ported.

//! BEP 16 super seeding (µTorrent's "initial seeding").

use std::time::{Duration, Instant};

use super::Engine;
use super::peer::PeerState;
use crate::proto::peer_wire::{self, msg};
use crate::util::bitfield::Bitfield;
use crate::util::random::random_below;

/// An uninterested peer gets another piece offered after this long (libtorrent `second_tick`).
const REOFFER_AFTER: Duration = Duration::from_secs(10);

/// What `piece_to_super_seed` needs to know about one connected peer.
pub(super) struct PeerView<'a> {
    pub has: &'a Bitfield,
    pub seed: bool,
    pub offered: [Option<u32>; 2],
}

impl PeerView<'_> {
    fn has_piece(&self, piece: u32) -> bool {
        self.seed || ((piece as usize) < self.has.len() && self.has.get(piece as usize))
    }
}

/// libtorrent `torrent::get_piece_to_super_seed`: a piece the peer does not have (`bits`, which
/// may be shorter than `num_pieces` before its bitfield), of the lowest availability among the
/// connected `peers`, avoiding pieces already offered to any peer; ties are broken by `random`
/// (given the number of candidates, returns an index below it). `peers` includes the peer
/// itself.
pub(super) fn piece_to_super_seed(
    num_pieces: u32,
    bits: &Bitfield,
    peers: &[PeerView<'_>],
    random: impl FnOnce(usize) -> usize,
) -> Option<u32> {
    let mut min_availability = u32::MAX;
    let mut candidates = Vec::new();
    for piece in 0..num_pieces {
        if (piece as usize) < bits.len() && bits.get(piece as usize) {
            continue;
        }
        let mut availability = 0;
        for p in peers {
            if p.offered.contains(&Some(piece)) {
                // Offering the same piece to two peers is avoided where possible.
                availability = 999;
                break;
            }
            if p.has_piece(piece) {
                availability += 1;
            }
        }
        if availability > min_availability {
            continue;
        }
        if availability < min_availability {
            min_availability = availability;
            candidates.clear();
        }
        candidates.push(piece);
    }
    if candidates.is_empty() {
        return None;
    }
    let i = random(candidates.len()).min(candidates.len() - 1);
    Some(candidates[i])
}

impl Engine {
    /// The next piece to offer the peer in `slot`.
    pub(super) fn superseed_pick(&self, slot: usize) -> Option<u32> {
        let p = self.peers.get(slot)?;
        let t = self.torrent_ref(p.torrent?)?;
        let views: Vec<PeerView<'_>> = t
            .peers
            .iter()
            .filter_map(|&s| self.peers.get(s))
            .map(|q| PeerView {
                has: &q.peer_has,
                seed: q.peer_seed,
                offered: q.superseed.unwrap_or_default(),
            })
            .collect();
        piece_to_super_seed(t.num_pieces, &p.peer_has, &views, |n| random_below(n as u64) as usize)
    }

    /// libtorrent `peer_connection::superseed_piece`: offers `new` with a HAVE, in place of
    /// `replace` (or of the older offer). `None` ends the offers and sends the piece set
    /// `write_bitfield` decides on.
    pub(super) fn superseed_piece(&mut self, slot: usize, replace: Option<u32>, new: Option<u32>) {
        let Some(p) = self.peers.get_mut(slot) else { return };
        if p.state != PeerState::Active {
            return;
        }
        let Some(mut offered) = p.superseed else { return };
        match new {
            None => {
                if offered[0].is_none() {
                    return;
                }
                p.superseed = Some([None, None]);
                self.write_bitfield(slot);
            }
            Some(piece) => {
                peer_wire::encode_u32(p.out(), msg::HAVE, piece);
                if replace.is_some() && offered[0] == replace {
                    offered.swap(0, 1);
                }
                offered[1] = offered[0];
                offered[0] = Some(piece);
                p.superseed = Some(offered);
                self.to_flush.push(slot);
            }
        }
    }

    /// The peer announced piece `piece`: if it was one we offered, offer the next one
    /// (libtorrent `incoming_have`).
    pub(super) fn superseed_on_have(&mut self, slot: usize, piece: u32) {
        let offered =
            self.peers.get(slot).and_then(|p| p.superseed).is_some_and(|o| o.contains(&Some(piece)));
        if offered {
            let next = self.superseed_pick(slot);
            self.superseed_piece(slot, Some(piece), next);
        }
    }

    /// libtorrent `second_tick`: a peer that has not been interested for 10 s gets another
    /// piece, once a second, until it is.
    pub(super) fn superseed_tick(&mut self, slot: usize, now: Instant) {
        let Some((p, Some(t))) = self.peer_and_torrent(slot) else { return };
        if p.superseed.is_none()
            || p.peer_interested
            || !t.super_seeding()
            || now.duration_since(p.became_uninterested) <= REOFFER_AFTER
        {
            return;
        }
        // Due again in a second.
        p.became_uninterested = now.checked_sub(REOFFER_AFTER - Duration::from_secs(1)).unwrap_or(now);
        let next = self.superseed_pick(slot);
        self.superseed_piece(slot, None, next);
    }

    /// The torrent's super seeding stopped (switched off, or we are no longer a seed): every
    /// super-seeded peer gets our real piece set (libtorrent `set_super_seeding(false)`).
    pub(super) fn superseed_end(&mut self, id: crate::types::TorrentId) {
        let Some(t) = self.torrent_ref(id) else { return };
        if t.super_seeding() {
            return;
        }
        let slots: Vec<usize> = t
            .peers
            .iter()
            .copied()
            .filter(|&s| self.peers.get(s).is_some_and(|p| p.superseed.is_some()))
            .collect();
        for slot in slots {
            self.write_bitfield(slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(n: usize, set: &[usize]) -> Bitfield {
        let mut b = Bitfield::new(n, false);
        for &i in set {
            b.set(i, true);
        }
        b
    }

    fn view(has: &Bitfield, offered: [Option<u32>; 2]) -> PeerView<'_> {
        PeerView { has, seed: false, offered }
    }

    #[test]
    fn picks_the_rarest_piece_the_peer_lacks() {
        let me = bits(4, &[]);
        let a = bits(4, &[0, 1, 2]);
        let b = bits(4, &[0, 1]);
        let peers = [view(&me, [None, None]), view(&a, [None, None]), view(&b, [None, None])];
        // Piece 3: nobody has it.
        assert_eq!(piece_to_super_seed(4, &me, &peers, |_| 0), Some(3));
        // The peer has 3: piece 2 (one copy) beats 0 and 1 (two copies).
        let me = bits(4, &[3]);
        let peers = [view(&me, [None, None]), view(&a, [None, None]), view(&b, [None, None])];
        assert_eq!(piece_to_super_seed(4, &me, &peers, |_| 0), Some(2));
    }

    #[test]
    fn offered_pieces_are_avoided_and_ties_are_random() {
        let none = bits(6, &[]);
        let peers = [view(&none, [Some(0), Some(1)]), view(&none, [Some(2), None])];
        let mut seen = Vec::new();
        for k in 0..3 {
            let pick = piece_to_super_seed(6, &none, &peers, |n| {
                assert_eq!(n, 3, "3, 4 and 5 tie");
                k
            });
            seen.push(pick.unwrap());
        }
        assert_eq!(seen, [3, 4, 5]);
        // An out-of-range random index is clamped.
        assert_eq!(piece_to_super_seed(6, &none, &peers, |_| 99), Some(5));
        // When every piece is offered somewhere, an offered one is picked anyway.
        let peers = [view(&none, [Some(0), Some(1)]), view(&none, [Some(2), None])];
        assert!(piece_to_super_seed(3, &none, &peers, |_| 0).is_some());
    }

    #[test]
    fn seeds_count_and_a_complete_peer_gets_nothing() {
        let me = bits(3, &[0]);
        let seed_bits = Bitfield::default();
        let other = bits(3, &[1]);
        let peers = [
            view(&me, [None, None]),
            PeerView { has: &seed_bits, seed: true, offered: [None, None] },
            view(&other, [None, None]),
        ];
        // 1: seed + other = 2, 2: seed = 1.
        assert_eq!(piece_to_super_seed(3, &me, &peers, |_| 0), Some(2));
        let all = bits(3, &[0, 1, 2]);
        assert_eq!(piece_to_super_seed(3, &all, &peers, |_| 0), None);
        // A bitfield not sized yet (no bitfield received) counts as having nothing.
        assert_eq!(piece_to_super_seed(3, &Bitfield::default(), &[], |_| 0), Some(0));
    }
}
