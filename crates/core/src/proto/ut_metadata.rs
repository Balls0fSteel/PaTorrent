// Ported from libtorrent-rasterbar 2.0.11: src/ut_metadata.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the torrent plugin becomes `MetadataFetch` and the peer plugin `PeerMetadata`, plain
// data driven by the engine (no plugin objects or weak pointers; sources are peer ids). A size
// from an extension handshake is taken only while no block has arrived, and a failed hash check
// also forgets the size (libtorrent resizes on every handshake and keeps the size after a
// failure).

//! BEP 9 metadata exchange (`ut_metadata`): the message codec and the state for downloading an
//! info dictionary from peers, sans-IO.

use std::time::{Duration, Instant};

use crate::bencode::{Entry, bdecode};
use crate::crypto::{Sha1, Sha256};
use crate::proto::peer_wire;
use crate::types::InfoHashes;

/// The extension name in BEP 10 handshakes.
pub const NAME: &str = "ut_metadata";
/// The id we ask peers to use for ut_metadata messages (libtorrent registers it as 2).
pub const OUR_ID: u8 = 2;
/// Metadata travels in 16 KiB blocks.
pub const BLOCK_SIZE: usize = 16 * 1024;
/// Larger ut_metadata messages disconnect the peer.
pub const MAX_MESSAGE: usize = 17 * 1024;
/// We only answer requests while a peer's send buffer holds less than this; the rest queue.
pub const SEND_BUFFER_LIMIT: usize = 0x4000 * 10;
/// Queued requests per peer before further ones are rejected.
pub const MAX_INCOMING_REQUESTS: usize = 1024;
/// Largest `metadata_size` an extension handshake may announce.
const HANDSHAKE_SIZE_LIMIT: i64 = 4 * 1024 * 1024;
/// Requests in flight per peer.
const MAX_SENT_REQUESTS: usize = 2;
/// The same block is asked of a live peer at most this often.
const REQUEST_INTERVAL: Duration = Duration::from_secs(3);
/// After a reject, the peer is not asked again for this long (unless it announced metadata).
const REJECT_PENALTY: Duration = Duration::from_secs(60);

/// `msg_type` values.
pub mod msg {
    pub const REQUEST: i64 = 0;
    pub const DATA: i64 = 1;
    pub const REJECT: i64 = 2;
}

/// A decoded ut_metadata message. `piece` is kept as sent, so out-of-range values can be
/// answered (a reject echoes the piece).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message<'a> {
    Request {
        piece: i64,
    },
    /// `total_size` is 0 when the key is missing.
    Data {
        piece: i64,
        total_size: i64,
        data: &'a [u8],
    },
    Reject {
        piece: i64,
    },
    /// A `msg_type` we don't know; ignored.
    Unknown {
        msg_type: i64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Longer than [`MAX_MESSAGE`].
    TooBig,
    /// The payload does not start with a bencoded dictionary.
    NotADict,
    /// `msg_type` or `piece` is missing or not an integer.
    MissingKeys,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DecodeError::TooBig => "ut_metadata message too big",
            DecodeError::NotADict => "ut_metadata message is not a dictionary",
            DecodeError::MissingKeys => "ut_metadata message without msg_type or piece",
        })
    }
}

/// Decodes the payload of an extended message (after the extension id byte). A data message's
/// block follows the dictionary.
pub fn decode(payload: &[u8]) -> Result<Message<'_>, DecodeError> {
    if payload.len() > MAX_MESSAGE {
        return Err(DecodeError::TooBig);
    }
    let dict_len = bencoded_len(payload).ok_or(DecodeError::NotADict)?;
    let doc = bdecode(&payload[..dict_len]).map_err(|_| DecodeError::NotADict)?;
    let root = doc.root().filter(|r| r.is_dict()).ok_or(DecodeError::NotADict)?;
    let msg_type = root.dict_find_int("msg_type").ok_or(DecodeError::MissingKeys)?.int_value();
    let piece = root.dict_find_int("piece").ok_or(DecodeError::MissingKeys)?.int_value();
    Ok(match msg_type {
        msg::REQUEST => Message::Request { piece },
        msg::DATA => Message::Data {
            piece,
            total_size: root.dict_find_int_value("total_size").unwrap_or(0),
            data: &payload[dict_len..],
        },
        msg::REJECT => Message::Reject { piece },
        _ => Message::Unknown { msg_type },
    })
}

/// Length of the first bencoded value in `buf` (the dictionary of a data message is followed by
/// raw bytes, which the decoder would reject as trailing garbage).
fn bencoded_len(buf: &[u8]) -> Option<usize> {
    let mut pos = 0;
    let mut depth = 0usize;
    loop {
        match *buf.get(pos)? {
            b'd' | b'l' => {
                depth += 1;
                pos += 1;
            }
            b'e' => {
                depth = depth.checked_sub(1)?;
                pos += 1;
            }
            b'i' => {
                pos += buf[pos..].iter().position(|&c| c == b'e')? + 1;
            }
            b'0'..=b'9' => {
                let colon = pos + buf[pos..].iter().position(|&c| c == b':')?;
                let len: usize = std::str::from_utf8(&buf[pos..colon]).ok()?.parse().ok()?;
                pos = colon.checked_add(1)?.checked_add(len)?;
                if pos > buf.len() {
                    return None;
                }
            }
            _ => return None,
        }
        if depth == 0 {
            return Some(pos);
        }
    }
}

/// Appends a complete extended message (`ext_id` is the id the peer chose for ut_metadata).
/// libtorrent `write_metadata_packet`: `total_size` goes out whenever we have the metadata.
pub fn encode(
    out: &mut Vec<u8>,
    ext_id: u8,
    msg_type: i64,
    piece: i64,
    total_size: Option<usize>,
    data: &[u8],
) {
    let mut d = Entry::new_dict();
    *d.key("msg_type") = Entry::Int(msg_type);
    *d.key("piece") = Entry::Int(piece);
    if let Some(n) = total_size {
        *d.key("total_size") = Entry::Int(n as i64);
    }
    let mut payload = d.bencode();
    payload.extend_from_slice(data);
    peer_wire::encode_extended(out, ext_id, &payload);
}

/// The block `piece` of an info dictionary of `size` bytes, if it exists.
pub fn block_range(size: usize, piece: i64) -> Option<std::ops::Range<usize>> {
    let start = usize::try_from(piece).ok()?.checked_mul(BLOCK_SIZE)?;
    (start < size).then(|| start..(start + BLOCK_SIZE).min(size))
}

/// Per-peer ut_metadata state (libtorrent `ut_metadata_peer_plugin`).
#[derive(Clone, Debug)]
pub struct PeerMetadata {
    /// The id the peer wants for ut_metadata messages; 0 if it doesn't support them.
    pub message_id: u8,
    /// False when the peer's handshake announced no `metadata_size`.
    pub has_metadata: bool,
    /// No requests to this peer before this time (after a reject or bad data).
    pub request_limit: Option<Instant>,
    /// The limit came from bad data, so it holds even for a peer that announced a size
    /// (SEC-21); a reject's limit only holds for peers that announced none.
    pub sent_bad_data: bool,
    /// Blocks we asked this peer for.
    pub sent_requests: Vec<i64>,
    /// Blocks the peer asked for while its send buffer was full.
    pub incoming_requests: std::collections::VecDeque<u32>,
}

impl Default for PeerMetadata {
    fn default() -> PeerMetadata {
        PeerMetadata {
            message_id: 0,
            has_metadata: true,
            request_limit: None,
            sent_bad_data: false,
            sent_requests: Vec::new(),
            incoming_requests: std::collections::VecDeque::new(),
        }
    }
}

impl PeerMetadata {
    /// libtorrent `has_metadata()` in the peer plugin: worth asking now.
    pub fn may_request(&self, now: Instant) -> bool {
        self.message_id != 0
            && self.sent_requests.len() < MAX_SENT_REQUESTS
            && ((self.has_metadata && !self.sent_bad_data) || self.request_limit.is_none_or(|l| now > l))
    }

    /// The peer rejected a request.
    pub fn rejected(&mut self, piece: i64, now: Instant) {
        let limit = now + REJECT_PENALTY;
        self.request_limit = Some(self.request_limit.map_or(limit, |l| l.max(limit)));
        if let Some(i) = self.sent_requests.iter().position(|&p| p == piece) {
            self.sent_requests.remove(i);
        }
    }

    /// Removes `piece` from the requests we sent. False if we never asked (or gave up on it).
    pub fn take_sent(&mut self, piece: i64) -> bool {
        match self.sent_requests.iter().position(|&p| p == piece) {
            Some(i) => {
                self.sent_requests.remove(i);
                true
            }
            None => false,
        }
    }

    /// libtorrent `failed_hash_check`: a random pause of 20-70 s from `from`. Unlike
    /// libtorrent, the pause also holds for a peer that announced a size (SEC-21).
    pub fn failed_hash_check(&mut self, from: Instant) {
        let secs = 20 + crate::util::random::random_below(50);
        self.request_limit = Some(from + Duration::from_secs(secs));
        self.sent_bad_data = true;
    }
}

#[derive(Clone, Debug, Default)]
struct Block {
    /// `u32::MAX` once the block is here.
    num_requests: u32,
    last_request: Option<Instant>,
    /// The peer asked last, or the peer the block came from.
    source: Option<u64>,
}

const HAVE: u32 = u32::MAX;

/// What [`MetadataFetch::received`] did with a block.
#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    /// Invalid, inconsistent or oversized; dropped.
    Rejected,
    /// Stored; more blocks are missing.
    Stored,
    /// The last block arrived and the info dictionary matches the info-hash.
    Complete(Vec<u8>),
    /// Every block arrived but the hash did not match. Everything was reset; `sources` are the
    /// peers that sent blocks, `single_peer` is true if the metadata was a single block.
    HashFailed { sources: Vec<u64>, single_peer: bool },
}

/// A fetch with some blocks in that gets no further for this long starts over (SEC-21).
pub const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Downloading one torrent's info dictionary (libtorrent `ut_metadata_plugin`).
#[derive(Clone, Debug, Default)]
pub struct MetadataFetch {
    metadata: Vec<u8>,
    blocks: Vec<Block>,
    /// Blocks stored so far.
    stored: usize,
    /// `stored` when [`MetadataFetch::stalled`] last saw it change, and when.
    watch: Option<(usize, Instant)>,
}

impl MetadataFetch {
    pub fn new() -> MetadataFetch {
        MetadataFetch::default()
    }

    /// A `metadata_size` from an extension handshake.
    pub fn size_hint(&mut self, size: i64) {
        if size <= 0 || size > HANDSHAKE_SIZE_LIMIT || self.blocks.iter().any(|b| b.num_requests == HAVE) {
            return;
        }
        self.resize(size as usize);
    }

    fn resize(&mut self, size: usize) {
        if self.metadata.len() != size {
            self.metadata = vec![0; size];
            self.blocks = vec![Block::default(); size.div_ceil(BLOCK_SIZE)];
            self.stored = 0;
        }
    }

    /// Drops everything; the next data message decides the size again.
    fn reset(&mut self) {
        self.metadata = Vec::new();
        self.blocks = Vec::new();
        self.stored = 0;
        self.watch = None;
    }

    /// Called on every tick. The first data message fixes the size, so one peer's false
    /// `total_size` makes every honest block a mismatch (or a request past the real end) and
    /// the fetch never finishes. When some blocks are in and none has followed for
    /// [`STALL_TIMEOUT`], everything is dropped and the peers that sent the kept blocks are
    /// returned, to be paused like after a failed hash check (SEC-21).
    pub fn stalled(&mut self, now: Instant) -> Option<Vec<u64>> {
        if self.stored == 0 {
            self.watch = None;
            return None;
        }
        match self.watch {
            Some((n, since)) if n == self.stored => {
                if now.saturating_duration_since(since) < STALL_TIMEOUT {
                    return None;
                }
            }
            _ => {
                self.watch = Some((self.stored, now));
                return None;
            }
        }
        let mut sources: Vec<u64> =
            self.blocks.iter().filter(|b| b.num_requests == HAVE).filter_map(|b| b.source).collect();
        sources.sort_unstable();
        sources.dedup();
        self.reset();
        Some(sources)
    }

    /// (blocks received, blocks in total); (0, 0) while the size is unknown.
    pub fn progress(&self) -> (usize, usize) {
        (self.blocks.iter().filter(|b| b.num_requests == HAVE).count(), self.blocks.len())
    }

    /// libtorrent `metadata_request`: the block to ask `peer` for, or `None` to hold off.
    /// `peer_has_metadata` is false for peers that announced no `metadata_size`; requests to
    /// them don't block the block for others. `alive` tells whether a peer id still exists.
    pub fn request(
        &mut self,
        peer: u64,
        peer_has_metadata: bool,
        now: Instant,
        alive: impl Fn(u64) -> bool,
    ) -> Option<u32> {
        if self.blocks.is_empty() {
            // Size unknown: ask for the first block; its reply carries the total size.
            self.blocks.push(Block::default());
        }
        let (piece, b) = self.blocks.iter_mut().enumerate().min_by_key(|(_, b)| b.num_requests)?;
        if b.num_requests == HAVE {
            return None;
        }
        if let (Some(last), Some(src)) = (b.last_request, b.source) {
            if alive(src) && now.duration_since(last) < REQUEST_INTERVAL {
                return None;
            }
        }
        b.num_requests += 1;
        if peer_has_metadata {
            b.last_request = Some(now);
            b.source = Some(peer);
        }
        Some(piece as u32)
    }

    /// libtorrent `received_metadata`. `max_size` is the `max_metadata_size` setting.
    pub fn received(
        &mut self,
        source: u64,
        piece: i64,
        total_size: i64,
        data: &[u8],
        max_size: usize,
        info_hashes: &InfoHashes,
    ) -> Received {
        let no_data_yet = !self.blocks.iter().any(|b| b.num_requests == HAVE);
        if self.metadata.is_empty()
            || (no_data_yet && total_size > 0 && total_size as usize != self.metadata.len())
        {
            if total_size <= 0 || total_size as u64 > max_size as u64 {
                return Received::Rejected;
            }
            // The handshake size may have been wrong; the first data message decides.
            self.resize(total_size as usize);
        }
        if total_size as u64 != self.metadata.len() as u64 {
            return Received::Rejected;
        }
        let Some(range) = block_range(self.metadata.len(), piece) else { return Received::Rejected };
        if data.len() != range.len() {
            return Received::Rejected;
        }
        self.metadata[range].copy_from_slice(data);
        let b = &mut self.blocks[piece as usize];
        if b.num_requests != HAVE {
            self.stored += 1;
        }
        b.num_requests = HAVE;
        b.source = Some(source);
        if self.stored < self.blocks.len() {
            return Received::Stored;
        }
        if verify(&self.metadata, info_hashes) {
            let info = std::mem::take(&mut self.metadata);
            self.reset();
            return Received::Complete(info);
        }
        let single_peer = self.blocks.len() == 1;
        let mut sources: Vec<u64> = self.blocks.iter().filter_map(|b| b.source).collect();
        sources.sort_unstable();
        sources.dedup();
        self.reset();
        Received::HashFailed { sources, single_peer }
    }
}

/// libtorrent `torrent::set_metadata`'s hash checks: SHA-1 (or a v2 hash truncated to 20 bytes)
/// against the v1 info-hash, SHA-256 against the v2 one.
pub fn verify(info: &[u8], ih: &InfoHashes) -> bool {
    if let Some(v1) = ih.v1 {
        if Sha1::digest(info) != v1 && Sha256::digest(info).0[..20] != v1.0 {
            return false;
        }
    }
    if let Some(v2) = ih.v2 {
        if Sha256::digest(info) != v2 {
            return false;
        }
    }
    ih.v1.is_some() || ih.v2.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Sha1Hash;

    fn payload(out: &[u8]) -> &[u8] {
        // length (4), msg id 20, extension id
        assert_eq!(out[4], peer_wire::msg::EXTENDED);
        &out[6..]
    }

    #[test]
    fn codec_round_trips() {
        let mut out = Vec::new();
        encode(&mut out, 3, msg::REQUEST, 7, None, &[]);
        assert_eq!(u32::from_be_bytes(out[..4].try_into().unwrap()) as usize, out.len() - 4);
        assert_eq!(out[5], 3);
        assert_eq!(payload(&out), b"d8:msg_typei0e5:piecei7ee");
        assert_eq!(decode(payload(&out)), Ok(Message::Request { piece: 7 }));

        out.clear();
        encode(&mut out, 1, msg::DATA, 0, Some(5), b"hello");
        assert_eq!(decode(payload(&out)), Ok(Message::Data { piece: 0, total_size: 5, data: b"hello" }));

        out.clear();
        encode(&mut out, 1, msg::REJECT, -1, None, &[]);
        assert_eq!(decode(payload(&out)), Ok(Message::Reject { piece: -1 }));
        assert_eq!(decode(b"d8:msg_typei9e5:piecei0ee"), Ok(Message::Unknown { msg_type: 9 }));
    }

    #[test]
    fn bad_messages_are_errors() {
        assert_eq!(decode(b"li1ee"), Err(DecodeError::NotADict));
        assert_eq!(decode(b"d8:msg_typei1e"), Err(DecodeError::NotADict));
        assert_eq!(decode(b"garbage"), Err(DecodeError::NotADict));
        assert_eq!(decode(b"d5:piecei1ee"), Err(DecodeError::MissingKeys));
        assert_eq!(decode(b"d8:msg_type1:a5:piecei1ee"), Err(DecodeError::MissingKeys));
        assert_eq!(decode(&vec![b'x'; MAX_MESSAGE + 1]), Err(DecodeError::TooBig));
        // A string length that runs past the end, and one that overflows.
        assert_eq!(decode(b"d8:msg_typei1e5:piece999:x"), Err(DecodeError::NotADict));
        assert_eq!(decode(b"d99999999999999999999999:x"), Err(DecodeError::NotADict));
    }

    #[test]
    fn block_ranges() {
        assert_eq!(block_range(40_000, 0), Some(0..16384));
        assert_eq!(block_range(40_000, 2), Some(32768..40_000));
        assert_eq!(block_range(40_000, 3), None);
        assert_eq!(block_range(40_000, -1), None);
        assert_eq!(block_range(0, 0), None);
    }

    fn info(len: usize) -> (Vec<u8>, InfoHashes) {
        let data: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
        let ih = InfoHashes::v1(Sha1::digest(&data));
        (data, ih)
    }

    #[test]
    fn fetch_completes_and_verifies() {
        let (data, ih) = info(40_000);
        let now = Instant::now();
        let mut f = MetadataFetch::new();
        // Unknown size: block 0 first.
        assert_eq!(f.request(1, true, now, |_| true), Some(0));
        // Block 0 was just asked of a live peer: hold off.
        assert_eq!(f.request(2, true, now, |_| true), None);
        // ...unless that peer is gone.
        assert_eq!(f.request(2, true, now, |_| false), Some(0));
        let r = f.received(1, 0, 40_000, &data[..16384], 1 << 20, &ih);
        assert_eq!(r, Received::Stored);
        assert_eq!(f.progress(), (1, 3));
        let a = f.request(1, true, now, |_| true).unwrap();
        let b = f.request(2, true, now, |_| true).unwrap();
        assert_eq!((a.min(b), a.max(b)), (1, 2));
        // Wrong length, bad piece, inconsistent size: dropped.
        assert_eq!(f.received(1, 1, 40_000, &data[..100], 1 << 20, &ih), Received::Rejected);
        assert_eq!(f.received(1, 3, 40_000, &data[..100], 1 << 20, &ih), Received::Rejected);
        assert_eq!(f.received(1, 1, 50_000, &data[16384..32768], 1 << 20, &ih), Received::Rejected);
        assert_eq!(f.received(2, 2, 40_000, &data[32768..], 1 << 20, &ih), Received::Stored);
        assert_eq!(f.received(1, 1, 40_000, &data[16384..32768], 1 << 20, &ih), Received::Complete(data));
    }

    #[test]
    fn hash_failure_resets_and_names_sources() {
        let (data, ih) = info(20_000);
        let mut bad = data.clone();
        bad[0] ^= 1;
        let mut f = MetadataFetch::new();
        f.size_hint(20_000);
        assert_eq!(f.progress(), (0, 2));
        assert_eq!(f.received(5, 0, 20_000, &bad[..16384], 1 << 20, &ih), Received::Stored);
        let r = f.received(9, 1, 20_000, &bad[16384..], 1 << 20, &ih);
        assert_eq!(r, Received::HashFailed { sources: vec![5, 9], single_peer: false });
        assert_eq!(f.progress(), (0, 0));
        // A fresh start works.
        f.size_hint(20_000);
        assert_eq!(f.received(5, 0, 20_000, &data[..16384], 1 << 20, &ih), Received::Stored);
        assert!(matches!(f.received(5, 1, 20_000, &data[16384..], 1 << 20, &ih), Received::Complete(_)));
    }

    #[test]
    fn sizes_are_limited_and_correctable() {
        let (data, ih) = info(1000);
        let mut f = MetadataFetch::new();
        f.size_hint(HANDSHAKE_SIZE_LIMIT + 1);
        assert_eq!(f.progress(), (0, 0));
        // Too big for the setting, or no size at all.
        assert_eq!(f.received(1, 0, 1000, &data, 999, &ih), Received::Rejected);
        assert_eq!(f.received(1, 0, 0, &data, 1 << 20, &ih), Received::Rejected);
        // A wrong handshake size is replaced by the first data message's size.
        f.size_hint(70_000);
        assert_eq!(f.progress(), (0, 5));
        assert!(matches!(f.received(1, 0, 1000, &data, 1 << 20, &ih), Received::Complete(_)));
    }

    #[test]
    fn a_false_size_cannot_stall_the_fetch() {
        // SEC-21: one peer answers first with a 30 MiB total_size; the real info is one block.
        let (data, ih) = info(1000);
        let max = 32 << 20;
        let now = Instant::now();
        let mut f = MetadataFetch::new();
        assert_eq!(f.received(66, 0, 30 << 20, &[0; BLOCK_SIZE], max, &ih), Received::Stored);
        assert_eq!(f.received(1, 0, 1000, &data, max, &ih), Received::Rejected, "the size is fixed");
        assert_eq!(f.stalled(now), None);
        assert_eq!(f.stalled(now + STALL_TIMEOUT - Duration::from_secs(1)), None);
        // Progress restarts the clock.
        assert_eq!(f.received(66, 1, 30 << 20, &[0; BLOCK_SIZE], max, &ih), Received::Stored);
        assert_eq!(f.stalled(now + STALL_TIMEOUT), None);
        let later = now + STALL_TIMEOUT * 2;
        assert_eq!(f.stalled(later), Some(vec![66]), "no progress for a while: start over");
        assert_eq!(f.progress(), (0, 0));
        assert_eq!(f.stalled(later + STALL_TIMEOUT), None, "nothing in, nothing to stall");
        assert!(matches!(f.received(1, 0, 1000, &data, max, &ih), Received::Complete(_)));
        // The source is paused even though it announced a size.
        let mut p = PeerMetadata { message_id: 3, ..PeerMetadata::default() };
        p.failed_hash_check(later);
        assert!(!p.may_request(later));
        assert!(p.may_request(later + Duration::from_secs(71)));
    }

    #[test]
    fn peers_back_off_after_rejects() {
        let now = Instant::now();
        let mut p = PeerMetadata { message_id: 3, ..PeerMetadata::default() };
        assert!(p.may_request(now));
        p.sent_requests = vec![0, 1];
        assert!(!p.may_request(now), "two requests in flight");
        p.rejected(1, now);
        assert_eq!(p.sent_requests, [0]);
        assert!(p.may_request(now), "peers that announced metadata ignore the penalty");
        p.has_metadata = false;
        assert!(!p.may_request(now));
        assert!(p.may_request(now + REJECT_PENALTY + Duration::from_secs(1)));
        assert!(p.take_sent(0));
        assert!(!p.take_sent(0));
    }

    #[test]
    fn verify_checks_v1_and_v2() {
        let data = b"d4:name1:xe";
        let v1 = Sha1::digest(data);
        let v2 = Sha256::digest(data);
        assert!(verify(data, &InfoHashes::v1(v1)));
        assert!(verify(data, &InfoHashes { v1: Some(v1), v2: Some(v2) }));
        assert!(verify(data, &InfoHashes { v1: None, v2: Some(v2) }));
        let truncated = Sha1Hash(v2.0[..20].try_into().unwrap());
        assert!(verify(data, &InfoHashes::v1(truncated)), "a v2 hash truncated to 20 bytes");
        assert!(!verify(b"other", &InfoHashes::v1(v1)));
        assert!(!verify(data, &InfoHashes::default()));
    }
}
