// Ported from libtorrent-rasterbar 2.0.11: src/ut_pex.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the torrent plugin becomes `TorrentPex` and the peer plugin `PeerPex`, plain data
// driven by the engine. The diff tracks the address we advertise (the listen port for incoming
// peers) rather than the connection's remote endpoint, so a dropped incoming peer is reported
// with the port it was added with; and a peer beyond the 100-entry cap is not remembered as
// sent (libtorrent records the first one it skips, so that peer is never announced).

//! BEP 11 peer exchange (`ut_pex`): the message codec and the per-torrent / per-peer state,
//! sans-IO.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use crate::bencode::{Entry, bdecode};
use crate::net::ip_voter::is_local;

/// The extension name in BEP 10 handshakes.
pub const NAME: &str = "ut_pex";
/// The id we ask peers to use for ut_pex messages (libtorrent registers it as 1).
pub const OUR_ID: u8 = 1;
/// Larger ut_pex messages disconnect the peer.
pub const MAX_MESSAGE: usize = 500 * 1024;
/// Most peers added in one message we send.
pub const MAX_PEER_ENTRIES: usize = 100;
/// Messages go out (and the diff is rebuilt) at most this often.
pub const INTERVAL: Duration = Duration::from_secs(60);
/// A peer may send at most this many messages per [`INTERVAL`].
const FLOOD_WINDOW: usize = 6;

/// `added.f` flag bits (libtorrent `pex_flags_t`). We set `SEED` and `ENCRYPTION`; `UTP` and
/// `HOLEPUNCH` follow uTP and BEP 55.
#[allow(dead_code)]
pub mod flags {
    pub const ENCRYPTION: u8 = 0x01;
    pub const SEED: u8 = 0x02;
    pub const UTP: u8 = 0x04;
    pub const HOLEPUNCH: u8 = 0x08;
    /// libtorrent-internal (BitTorrent v2 peer); cleared on received flags.
    pub const LT_V2: u8 = 0x80;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PexError {
    /// Longer than [`MAX_MESSAGE`].
    TooBig,
    /// More than six messages within a minute.
    TooFrequent,
    /// Not a bencoded dictionary.
    Invalid,
}

impl std::fmt::Display for PexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PexError::TooBig => "ut_pex message too large",
            PexError::TooFrequent => "too frequent ut_pex messages",
            PexError::Invalid => "invalid ut_pex message",
        })
    }
}

fn read_v4(b: &[u8]) -> SocketAddr {
    SocketAddr::new(Ipv4Addr::new(b[0], b[1], b[2], b[3]).into(), u16::from_be_bytes([b[4], b[5]]))
}

fn read_v6(b: &[u8]) -> SocketAddr {
    let ip: [u8; 16] = b[..16].try_into().expect("18-byte entry");
    SocketAddr::new(Ipv6Addr::from(ip).into(), u16::from_be_bytes([b[16], b[17]]))
}

fn write_endpoint(out: &mut Vec<u8>, a: &SocketAddr) {
    match a.ip() {
        IpAddr::V4(ip) => out.extend_from_slice(&ip.octets()),
        IpAddr::V6(ip) => out.extend_from_slice(&ip.octets()),
    }
    out.extend_from_slice(&a.port().to_be_bytes());
}

/// The six strings of a ut_pex message, built up and then bencoded.
#[derive(Default)]
struct Builder {
    added: Vec<u8>,
    added_f: Vec<u8>,
    added6: Vec<u8>,
    added6_f: Vec<u8>,
    dropped: Vec<u8>,
    dropped6: Vec<u8>,
}

impl Builder {
    fn add(&mut self, a: &SocketAddr, f: u8) {
        if a.is_ipv4() {
            write_endpoint(&mut self.added, a);
            self.added_f.push(f);
        } else {
            write_endpoint(&mut self.added6, a);
            self.added6_f.push(f);
        }
    }

    fn drop(&mut self, a: &SocketAddr) {
        write_endpoint(if a.is_ipv4() { &mut self.dropped } else { &mut self.dropped6 }, a);
    }

    fn bencode(self) -> Vec<u8> {
        let mut d = Entry::new_dict();
        *d.key("added") = Entry::Bytes(self.added);
        *d.key("added.f") = Entry::Bytes(self.added_f);
        *d.key("added6") = Entry::Bytes(self.added6);
        *d.key("added6.f") = Entry::Bytes(self.added6_f);
        *d.key("dropped") = Entry::Bytes(self.dropped);
        *d.key("dropped6") = Entry::Bytes(self.dropped6);
        d.bencode()
    }
}

/// The full peer list for a peer's first message (libtorrent `send_ut_peer_list`): up to
/// [`MAX_PEER_ENTRIES`] of `peers` (advertised address, flags), nothing dropped. Returns the
/// payload of the extended message.
pub fn encode_full(peers: impl IntoIterator<Item = (SocketAddr, u8)>) -> Vec<u8> {
    let mut b = Builder::default();
    for (a, f) in peers.into_iter().take(MAX_PEER_ENTRIES) {
        b.add(&a, f);
    }
    b.bencode()
}

/// Per-torrent state (libtorrent `ut_pex_plugin`): once a minute, the peers added and dropped
/// since the last diff, as one message every peer then gets.
#[derive(Debug, Default)]
pub struct TorrentPex {
    old_peers: HashSet<SocketAddr>,
    last_msg: Option<Instant>,
    msg: Vec<u8>,
    peers_in_msg: usize,
}

impl TorrentPex {
    /// libtorrent `ut_pex_plugin::tick`. `num_peers` counts every connection; `peers` are the
    /// ones worth sending (connected and with a known listen port), as advertised address and
    /// flags.
    pub fn tick(
        &mut self,
        now: Instant,
        num_peers: usize,
        peers: impl IntoIterator<Item = (SocketAddr, u8)>,
    ) {
        if self.last_msg.is_some_and(|t| now.duration_since(t) < INTERVAL) {
            return;
        }
        self.last_msg = Some(now);
        if num_peers == 0 {
            return;
        }
        let mut dropped = std::mem::take(&mut self.old_peers);
        let mut b = Builder::default();
        self.peers_in_msg = 0;
        let mut num_added = 0;
        for (a, f) in peers {
            if dropped.remove(&a) {
                // in the last message too, so not dropped
                self.old_peers.insert(a);
                continue;
            }
            if num_added >= MAX_PEER_ENTRIES {
                continue;
            }
            self.old_peers.insert(a);
            b.add(&a, f);
            num_added += 1;
            self.peers_in_msg += 1;
        }
        for a in &dropped {
            b.drop(a);
            self.peers_in_msg += 1;
        }
        self.msg = b.bencode();
    }

    /// The current diff, unless it is empty (libtorrent sends nothing then).
    pub fn diff(&self) -> Option<&[u8]> {
        (self.peers_in_msg > 0).then_some(&self.msg[..])
    }
}

/// What a peer's tick wants sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Send {
    /// The full peer list ([`encode_full`]), the first time.
    Full,
    /// The torrent's current diff ([`TorrentPex::diff`]), if any.
    Diff,
}

/// Per-peer state (libtorrent `ut_pex_peer_plugin`).
#[derive(Clone, Debug)]
pub struct PeerPex {
    /// The id the peer wants for ut_pex messages; 0 if it doesn't support them.
    pub message_id: u8,
    /// When the last six messages arrived, oldest first.
    last_pex: [Option<Instant>; FLOOD_WINDOW],
    last_msg: Option<Instant>,
    first_time: bool,
    /// Peers this peer told us about and has not dropped (each family capped separately).
    peers4: HashSet<SocketAddr>,
    peers6: HashSet<SocketAddr>,
}

impl Default for PeerPex {
    fn default() -> PeerPex {
        PeerPex {
            message_id: 0,
            last_pex: [None; FLOOD_WINDOW],
            last_msg: None,
            first_time: true,
            peers4: HashSet::new(),
            peers6: HashSet::new(),
        }
    }
}

impl PeerPex {
    /// libtorrent `on_extended`: a ut_pex message from the peer at `remote`. Returns the peers
    /// new to this connection (address, flags) for the torrent's peer list; at most
    /// `max_pex_peers` per address family are remembered. Local addresses are skipped unless
    /// the sender is local too.
    pub fn on_message(
        &mut self,
        payload: &[u8],
        now: Instant,
        remote: IpAddr,
        max_pex_peers: usize,
    ) -> Result<Vec<(SocketAddr, u8)>, PexError> {
        if self.message_id == 0 {
            return Ok(Vec::new());
        }
        if payload.len() > MAX_MESSAGE {
            return Err(PexError::TooBig);
        }
        if self.last_pex[0].is_some_and(|t| now.duration_since(t) < INTERVAL) {
            return Err(PexError::TooFrequent);
        }
        self.last_pex.rotate_left(1);
        self.last_pex[FLOOD_WINDOW - 1] = Some(now);

        let doc = bdecode(payload).map_err(|_| PexError::Invalid)?;
        let root = doc.root().filter(|r| r.is_dict()).ok_or(PexError::Invalid)?;
        let remote_local = is_local(&remote);
        let mut out = Vec::new();

        for (key, added_key, flags_key, len) in
            [("dropped", "added", "added.f", 6), ("dropped6", "added6", "added6.f", 18)]
        {
            let known = if len == 6 { &mut self.peers4 } else { &mut self.peers6 };
            let read = if len == 6 { read_v4 } else { read_v6 };
            if let Some(d) = root.dict_find_string_value(key) {
                for e in d.chunks_exact(len) {
                    known.remove(&read(e));
                }
            }
            let (Some(a), Some(f)) =
                (root.dict_find_string_value(added_key), root.dict_find_string_value(flags_key))
            else {
                continue;
            };
            if f.len() != a.len() / len {
                continue;
            }
            for (e, &fl) in a.chunks_exact(len).zip(f) {
                if known.len() >= max_pex_peers {
                    break;
                }
                let addr = read(e);
                if is_local(&addr.ip()) && !remote_local {
                    continue;
                }
                if known.insert(addr) {
                    out.push((addr, fl & !flags::LT_V2));
                }
            }
        }
        Ok(out)
    }

    /// libtorrent `ut_pex_peer_plugin::tick`: once a minute, while the torrent has more than
    /// this one peer, the full list the first time and the torrent's diff after that.
    pub fn tick(&mut self, now: Instant, num_peers: usize) -> Option<Send> {
        if self.message_id == 0 || self.last_msg.is_some_and(|t| now.duration_since(t) < INTERVAL) {
            return None;
        }
        if num_peers <= 1 {
            return None;
        }
        self.last_msg = Some(now);
        Some(if std::mem::take(&mut self.first_time) { Send::Full } else { Send::Diff })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn peer() -> PeerPex {
        PeerPex { message_id: 1, ..PeerPex::default() }
    }

    fn msg(added: &[(&str, u8)], dropped: &[&str]) -> Vec<u8> {
        let mut b = Builder::default();
        for (a, f) in added {
            b.add(&ep(a), *f);
        }
        for a in dropped {
            b.drop(&ep(a));
        }
        b.bencode()
    }

    #[test]
    fn full_list_round_trip() {
        let peers = [(ep("1.2.3.4:5"), flags::SEED), (ep("[2001:db8::1]:6881"), 0)];
        let m = encode_full(peers);
        assert_eq!(
            m,
            b"d5:added6:\x01\x02\x03\x04\x00\x057:added.f1:\x026:added618:\x20\x01\x0d\xb8\
              \0\0\0\0\0\0\0\0\0\0\0\x01\x1a\xe18:added6.f1:\x007:dropped0:8:dropped60:e"
        );
        let got = peer().on_message(&m, Instant::now(), "9.9.9.9".parse().unwrap(), 50).unwrap();
        assert_eq!(got, peers);
        let many = (0..150u16).map(|i| (SocketAddr::from(([1, 1, 1, 1], i + 1)), 0));
        let got = peer().on_message(&encode_full(many), Instant::now(), "9.9.9.9".parse().unwrap(), 500);
        assert_eq!(got.unwrap().len(), MAX_PEER_ENTRIES);
    }

    #[test]
    fn receiving_filters_and_caps() {
        let now = Instant::now();
        let public: IpAddr = "9.9.9.9".parse().unwrap();
        let mut p = peer();
        let m =
            msg(&[("1.1.1.1:1", flags::SEED | flags::LT_V2), ("192.168.1.2:3", 0), ("1.1.1.2:1", 0)], &[]);
        assert_eq!(
            p.on_message(&m, now, public, 50).unwrap(),
            [(ep("1.1.1.1:1"), flags::SEED), (ep("1.1.1.2:1"), 0)],
            "local addresses only from local peers; lt_v2 cleared"
        );
        let mut local = peer();
        assert_eq!(local.on_message(&m, now, "10.0.0.1".parse().unwrap(), 50).unwrap().len(), 3);

        // Known peers are not reported twice until the sender drops them.
        let m2 = msg(&[("1.1.1.1:1", 0)], &[]);
        assert!(p.on_message(&m2, now, public, 50).unwrap().is_empty());
        let m3 = msg(&[("1.1.1.1:1", 0)], &["1.1.1.1:1"]);
        assert_eq!(p.on_message(&m3, now, public, 50).unwrap().len(), 1, "dropped, then added again");

        // The cap counts what the sender told us about.
        let mut capped = peer();
        let m4 = msg(&[("1.1.1.1:1", 0), ("1.1.1.2:1", 0), ("1.1.1.3:1", 0)], &[]);
        assert_eq!(capped.on_message(&m4, now, public, 2).unwrap().len(), 2);

        // Flag strings of the wrong length make the family's added list ignored.
        let bad = b"d5:added6:\x01\x01\x01\x01\x00\x017:added.f2:\0\0e";
        assert!(peer().on_message(bad, now, public, 50).unwrap().is_empty());
        // Without a handshake id the message is ignored.
        assert!(PeerPex::default().on_message(&m, now, public, 50).unwrap().is_empty());
        assert_eq!(peer().on_message(b"li1ee", now, public, 50), Err(PexError::Invalid));
        assert_eq!(peer().on_message(b"d5:added", now, public, 50), Err(PexError::Invalid));
        let big = vec![b'x'; MAX_MESSAGE + 1];
        assert_eq!(peer().on_message(&big, now, public, 50), Err(PexError::TooBig));
    }

    #[test]
    fn flooding_disconnects() {
        let now = Instant::now();
        let public: IpAddr = "9.9.9.9".parse().unwrap();
        let mut p = peer();
        let m = msg(&[], &[]);
        for _ in 0..6 {
            p.on_message(&m, now, public, 50).unwrap();
        }
        assert_eq!(p.on_message(&m, now, public, 50), Err(PexError::TooFrequent));
        let mut p = peer();
        for i in 0..20u64 {
            p.on_message(&m, now + Duration::from_secs(11 * i), public, 50).unwrap();
        }
    }

    #[test]
    fn diffs_track_added_and_dropped() {
        let t0 = Instant::now();
        let a = (ep("1.1.1.1:1"), 0);
        let b = (ep("[2001:db8::2]:2"), flags::SEED);
        let c = (ep("1.1.1.3:3"), 0);
        let mut tp = TorrentPex::default();
        tp.tick(t0, 0, []);
        assert!(tp.diff().is_none(), "no peers, no message");
        tp.tick(t0 + INTERVAL, 2, [a, b]);
        let mut rx = peer();
        let public: IpAddr = "9.9.9.9".parse().unwrap();
        assert_eq!(rx.on_message(tp.diff().unwrap(), t0, public, 50).unwrap(), [a, b]);

        let before = tp.diff().unwrap().to_vec();
        tp.tick(t0 + INTERVAL + Duration::from_secs(1), 2, [a, c]);
        assert_eq!(tp.diff().unwrap(), before, "too soon to rebuild");
        tp.tick(t0 + INTERVAL * 2, 2, [a, c]);
        assert_eq!(tp.diff().unwrap(), msg(&[("1.1.1.3:3", 0)], &["[2001:db8::2]:2"]));
        tp.tick(t0 + INTERVAL * 3, 2, [a, c]);
        assert!(tp.diff().is_none(), "nothing changed");

        // More than 100 new peers: the rest wait for the next diff.
        let many: Vec<_> = (0..130u16).map(|i| (SocketAddr::from(([2, 2, 2, 2], i + 1)), 0)).collect();
        tp.tick(t0 + INTERVAL * 4, 130, many.iter().copied());
        let first = rx.on_message(tp.diff().unwrap(), t0 + INTERVAL * 4, public, 1000).unwrap();
        assert_eq!(first.len(), MAX_PEER_ENTRIES);
        tp.tick(t0 + INTERVAL * 5, 130, many.iter().copied());
        let rest = rx.on_message(tp.diff().unwrap(), t0 + INTERVAL * 5, public, 1000).unwrap();
        assert_eq!(rest.len(), 30);
    }

    #[test]
    fn peer_tick_sends_full_then_diffs() {
        let t0 = Instant::now();
        let mut p = PeerPex::default();
        assert_eq!(p.tick(t0, 5), None, "no handshake yet");
        p.message_id = 3;
        assert_eq!(p.tick(t0, 1), None, "only this peer");
        assert_eq!(p.tick(t0, 2), Some(Send::Full));
        assert_eq!(p.tick(t0 + Duration::from_secs(59), 2), None);
        assert_eq!(p.tick(t0 + INTERVAL, 2), Some(Send::Diff));
    }
}
