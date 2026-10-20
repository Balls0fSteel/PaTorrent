// Ported from libtorrent-rasterbar 2.0.11: src/bt_peer_connection.cpp (write_pe1_2_dhkey,
// write_pe3_sync, write_pe4_sync, write_pe_vc_cryptofield, init_bt_handshake and the read_pe_*
// states of on_receive_impl), src/session_impl.cpp (find_encrypted_torrent)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a sans-IO state machine fed from the receive buffer and writing to the send buffer;
// the engine attaches the torrent when the stream key is found, through a lookup callback and a
// `Torrent` step, instead of `attach_to_torrent` inside the state.

//! The MSE/PE encrypted handshake ("Message Stream Encryption"), as the BitTorrent clients that
//! support it speak it:
//!
//! ```text
//! A -> B: Diffie-Hellman public key Ya, PadA (0..=512 random bytes)
//! B -> A: Yb, PadB
//! A -> B: HASH("req1", S), HASH("req2", SKEY) xor HASH("req3", S),
//!         ENCRYPT(VC, crypto_provide, len(PadC), PadC, len(IA)), ENCRYPT(IA)
//! B -> A: ENCRYPT(VC, crypto_select, len(PadD), PadD), ENCRYPT2(payload stream)
//! ```
//!
//! `S` is the shared secret, `SKEY` the info-hash, `VC` eight zero bytes and `IA` the initial
//! payload (our plain BitTorrent handshake). Once both sides agree, the rest of the connection is
//! RC4-encrypted (`crypto_select` 2) or plaintext (1).

use crate::crypto::mse::{self, DH_KEY_LEN, DhKeyExchange, Rc4Handler};
use crate::proto::peer_wire::HANDSHAKE_LEN;
use crate::settings::EncLevel;
use crate::types::Sha1Hash;
use crate::util::random;

/// `crypto_provide` / `crypto_select` bits (libtorrent `pe_plaintext`, `pe_rc4`).
pub const PLAINTEXT: u32 = 1;
pub const RC4: u32 = 2;

/// The crypto bits a setting allows.
pub fn level_bits(level: EncLevel) -> u32 {
    match level {
        EncLevel::Plaintext => PLAINTEXT,
        EncLevel::Rc4 => RC4,
        EncLevel::Both => PLAINTEXT | RC4,
    }
}

/// Why an encrypted handshake failed (libtorrent `errors::` of the same names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeError {
    /// 512 bytes of padding went by without the sync hash.
    SyncHashNotFound,
    /// The verification constant did not decrypt to zeros (or was never found).
    InvalidEncryptionConstant,
    /// The obfuscated stream key names no torrent of ours.
    InvalidInfoHash,
    /// The peer offers no encryption level we allow.
    UnsupportedEncryptionMode,
    /// The peer selected a level we did not offer.
    UnsupportedEncryptionModeSelected,
    /// A pad longer than 512 bytes.
    InvalidPadSize,
    /// `len(IA)` longer than a handshake.
    InvalidEncryptHandshake,
}

impl std::fmt::Display for PeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PeError::SyncHashNotFound => "sync hash not found",
            PeError::InvalidEncryptionConstant => "invalid encryption constant",
            PeError::InvalidInfoHash => "unknown info-hash in encrypted handshake",
            PeError::UnsupportedEncryptionMode => "unsupported encryption mode",
            PeError::UnsupportedEncryptionModeSelected => "peer selected unsupported encryption mode",
            PeError::InvalidPadSize => "invalid pad size",
            PeError::InvalidEncryptHandshake => "invalid encrypted handshake",
        })
    }
}

impl std::error::Error for PeError {}

/// What one call to [`PeHandshake::feed`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeStep {
    /// More bytes are needed.
    Wait,
    /// Bytes were consumed or written; call again.
    Continue,
    /// Incoming: the peer's stream key names this torrent. The caller attaches the connection to
    /// it (or closes it), then calls again.
    Torrent(Sha1Hash),
    /// The encrypted handshake is complete: `rc4` says whether the payload stream is RC4
    /// (otherwise plaintext). What remains in the buffer is the start of the plain BitTorrent
    /// handshake, decrypted.
    Done { rc4: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    DhKey,
    /// Incoming: looking for `HASH("req1", S)`.
    SyncHash,
    /// Incoming: the obfuscated stream key and the verification constant.
    SkeyVc,
    /// Outgoing: looking for the encrypted verification constant.
    SyncVc,
    /// `crypto_provide` / `crypto_select` and `len(pad)`.
    CryptoField,
    /// The pad (plus `len(IA)` when incoming).
    Pad(usize),
    /// Incoming: the initial payload, always RC4.
    Ia(usize),
}

/// One side of the encrypted handshake (the `read_pe_*` states of `bt_peer_connection`).
pub struct PeHandshake {
    outgoing: bool,
    stage: Stage,
    dh: Option<DhKeyExchange>,
    rc4: Option<Rc4Handler>,
    /// Bytes skipped so far while looking for a sync point (at most 512 of padding).
    sync_bytes_read: usize,
    /// The sync point we look for: 20 bytes of hash (incoming) or 8 of encrypted VC (outgoing).
    sync: Option<([u8; 20], usize)>,
    info_hash: Option<Sha1Hash>,
    /// Outgoing: the initial payload, our plain handshake.
    ia: Vec<u8>,
    allowed: u32,
    prefer_rc4: bool,
    rc4_selected: bool,
}

impl PeHandshake {
    /// Starts an outgoing encrypted handshake: writes our DH key and pad (`write_pe1_2_dhkey`)
    /// to `out`. `ia` is the plain BitTorrent handshake to send once the keys are agreed.
    pub fn outgoing(
        info_hash: Sha1Hash,
        ia: Vec<u8>,
        level: EncLevel,
        prefer_rc4: bool,
        out: &mut Vec<u8>,
    ) -> Self {
        let mut hs = PeHandshake {
            outgoing: true,
            stage: Stage::DhKey,
            dh: None,
            rc4: None,
            sync_bytes_read: 0,
            sync: None,
            info_hash: Some(info_hash),
            ia,
            allowed: level_bits(level),
            prefer_rc4,
            rc4_selected: false,
        };
        hs.write_pe1_2_dhkey(out);
        hs
    }

    /// An incoming connection whose first bytes were not a plain handshake: they are the start
    /// of the peer's DH key.
    pub fn incoming(level: EncLevel, prefer_rc4: bool) -> Self {
        PeHandshake {
            outgoing: false,
            stage: Stage::DhKey,
            dh: None,
            rc4: None,
            sync_bytes_read: 0,
            sync: None,
            info_hash: None,
            ia: Vec::new(),
            allowed: level_bits(level),
            prefer_rc4,
            rc4_selected: false,
        }
    }

    /// The RC4 streams, positioned after the handshake, for the payload.
    pub fn into_rc4(self) -> Option<Rc4Handler> {
        self.rc4
    }

    fn write_pe1_2_dhkey(&mut self, out: &mut Vec<u8>) {
        let dh = self.dh.get_or_insert_with(DhKeyExchange::new);
        out.extend_from_slice(dh.local_key());
        write_pad(out, random_pad_size());
    }

    /// `write_pe3_sync` plus the RC4-encrypted initial payload.
    fn write_pe3_sync(&mut self, out: &mut Vec<u8>) {
        let dh = self.dh.take().expect("key exchanged");
        let info_hash = self.info_hash.expect("outgoing knows its torrent");
        let secret = dh.secret();
        out.extend_from_slice(&mse::sync_hash(secret).0);
        out.extend_from_slice(&(mse::stream_key_hash(&info_hash) ^ *dh.hash_xor_mask()).0);
        let mut rc4 = Rc4Handler::for_connection(secret, &info_hash, true);
        // This is an invalid setting, but let's just make the best of the situation.
        let crypto_provide =
            if self.allowed & (PLAINTEXT | RC4) == 0 { PLAINTEXT | RC4 } else { self.allowed };
        let start = out.len();
        write_vc_cryptofield(out, crypto_provide, random_pad_size(), Some(self.ia.len() as u16));
        out.extend_from_slice(&self.ia);
        rc4.encrypt(&mut out[start..]);
        self.rc4 = Some(rc4);
    }

    /// `write_pe4_sync`: the reply with our choice.
    fn write_pe4_sync(&mut self, out: &mut Vec<u8>, crypto_select: u32) {
        let start = out.len();
        write_vc_cryptofield(out, crypto_select, random_pad_size(), None);
        self.rc4.as_mut().expect("keys derived").encrypt(&mut out[start..]);
        self.rc4_selected = crypto_select == RC4;
    }

    /// Reads what it can of `buf` (the unprocessed receive buffer), decrypting in place where the
    /// protocol does, and appends anything to send to `out`. Returns how many leading bytes of
    /// `buf` are consumed, and what happened. `find` maps an obfuscated stream-key hash
    /// (`HASH("req2", SKEY)`) to the info-hash of one of our torrents.
    pub fn feed(
        &mut self,
        buf: &mut [u8],
        out: &mut Vec<u8>,
        find: &mut dyn FnMut(&Sha1Hash) -> Option<Sha1Hash>,
    ) -> Result<(usize, PeStep), PeError> {
        match self.stage {
            Stage::DhKey => {
                if buf.len() < DH_KEY_LEN {
                    return Ok((0, PeStep::Wait));
                }
                // The responder writes its own key now; the initiator did at the start.
                if !self.outgoing {
                    self.write_pe1_2_dhkey(out);
                }
                let remote: [u8; DH_KEY_LEN] = buf[..DH_KEY_LEN].try_into().expect("96 bytes");
                self.dh.as_mut().expect("created").compute_secret(&remote);
                if self.outgoing {
                    self.stage = Stage::SyncVc;
                    self.write_pe3_sync(out);
                } else {
                    self.stage = Stage::SyncHash;
                }
                Ok((DH_KEY_LEN, PeStep::Continue))
            }
            Stage::SyncHash => {
                if self.sync.is_none() {
                    let secret = self.dh.as_ref().expect("key exchanged").secret();
                    self.sync = Some((mse::sync_hash(secret).0, 20));
                }
                self.sync_to(buf, 20, Stage::SkeyVc, PeError::SyncHashNotFound)
            }
            Stage::SkeyVc => {
                if buf.len() < 28 {
                    return Ok((0, PeStep::Wait));
                }
                let dh = self.dh.as_ref().expect("key exchanged");
                let Some(info_hash) = self.info_hash else {
                    let obfuscated =
                        Sha1Hash::from_slice(&buf[..20]).expect("20 bytes") ^ *dh.hash_xor_mask();
                    let Some(ih) = find(&obfuscated) else { return Err(PeError::InvalidInfoHash) };
                    self.info_hash = Some(ih);
                    return Ok((0, PeStep::Torrent(ih)));
                };
                let mut rc4 = Rc4Handler::for_connection(dh.secret(), &info_hash, false);
                rc4.decrypt(&mut buf[20..28]);
                if buf[20..28] != [0u8; 8] {
                    return Err(PeError::InvalidEncryptionConstant);
                }
                self.rc4 = Some(rc4);
                self.dh = None;
                self.stage = Stage::CryptoField;
                Ok((28, PeStep::Continue))
            }
            Stage::SyncVc => {
                if self.sync.is_none() {
                    // What eight zero bytes look like under the peer's first keystream bytes.
                    let mut vc = [0u8; 20];
                    self.rc4.as_mut().expect("keys derived").decrypt(&mut vc[..8]);
                    self.sync = Some((vc, 8));
                }
                self.sync_to(buf, 8, Stage::CryptoField, PeError::InvalidEncryptionConstant)
            }
            Stage::CryptoField => {
                if buf.len() < 6 {
                    return Ok((0, PeStep::Wait));
                }
                let rc4 = self.rc4.as_mut().expect("keys derived");
                rc4.decrypt(&mut buf[..6]);
                let crypto_field = u32::from_be_bytes(buf[..4].try_into().expect("4 bytes"));
                let mut len_pad = u16::from_be_bytes(buf[4..6].try_into().expect("2 bytes")) as usize;
                if self.outgoing {
                    // Check if the crypto select is valid.
                    let selected = crypto_field & self.allowed;
                    if selected == 0 {
                        return Err(PeError::UnsupportedEncryptionModeSelected);
                    }
                    if selected == PLAINTEXT {
                        self.rc4_selected = false;
                    } else if selected == RC4 {
                        self.rc4_selected = true;
                    }
                } else {
                    // Select a crypto method: with prefer_rc4 keep the most significant bit,
                    // otherwise the least significant one.
                    let mut select = crypto_field & self.allowed;
                    if self.prefer_rc4 {
                        let mut mask = u32::MAX;
                        while select & (mask << 1) != 0 {
                            mask <<= 1;
                            select &= mask;
                        }
                    } else {
                        let mut mask = u32::MAX;
                        while select & (mask >> 1) != 0 {
                            mask >>= 1;
                            select &= mask;
                        }
                    }
                    if select == 0 {
                        return Err(PeError::UnsupportedEncryptionMode);
                    }
                    self.write_pe4_sync(out, select);
                }
                if len_pad > 512 {
                    return Err(PeError::InvalidPadSize);
                }
                if !self.outgoing {
                    len_pad += 2; // len(IA) follows the pad
                }
                if len_pad > 0 {
                    self.stage = Stage::Pad(len_pad);
                    Ok((6, PeStep::Continue))
                } else {
                    Ok(self.init_bt_handshake(buf, 6))
                }
            }
            Stage::Pad(n) => {
                if buf.len() < n {
                    return Ok((0, PeStep::Wait));
                }
                self.rc4.as_mut().expect("keys derived").decrypt(&mut buf[..n]);
                if self.outgoing {
                    return Ok(self.init_bt_handshake(buf, n));
                }
                let len_ia = u16::from_be_bytes(buf[n - 2..n].try_into().expect("2 bytes")) as usize;
                if len_ia > HANDSHAKE_LEN {
                    return Err(PeError::InvalidEncryptHandshake);
                }
                if len_ia == 0 {
                    Ok(self.init_bt_handshake(buf, n))
                } else {
                    // The peer starts the BitTorrent protocol with len_ia bytes that are always
                    // RC4, whatever was selected; they are decrypted and left in the buffer.
                    self.stage = Stage::Ia(len_ia);
                    Ok((n, PeStep::Continue))
                }
            }
            Stage::Ia(n) => {
                if buf.len() < n {
                    return Ok((0, PeStep::Wait));
                }
                let rc4 = self.rc4.as_mut().expect("keys derived");
                rc4.decrypt(&mut buf[..n]);
                if self.rc4_selected {
                    rc4.decrypt(&mut buf[n..]);
                }
                Ok((0, PeStep::Done { rc4: self.rc4_selected }))
            }
        }
    }

    /// Looks for the sync point in `buf` (libtorrent `search` plus the 512-byte pad limit).
    fn sync_to(
        &mut self,
        buf: &[u8],
        len: usize,
        next: Stage,
        err: PeError,
    ) -> Result<(usize, PeStep), PeError> {
        if buf.len() < len {
            return Ok((0, PeStep::Wait));
        }
        let (needle, _) = self.sync.expect("set by the caller");
        let needle = &needle[..len];
        match buf.windows(len).position(|w| w == needle) {
            None => {
                // Keep the last len - 1 bytes: the sync point may straddle two reads.
                let processed = buf.len() - len;
                self.sync_bytes_read += processed;
                if self.sync_bytes_read >= 512 {
                    return Err(err);
                }
                Ok((processed, if processed > 0 { PeStep::Continue } else { PeStep::Wait }))
            }
            Some(offset) => {
                self.sync = None;
                self.stage = next;
                Ok((offset + len, PeStep::Continue))
            }
        }
    }

    /// `init_bt_handshake`: the encrypted part is over; `consumed` bytes of `buf` belong to it
    /// and, with RC4 selected, everything after them is payload stream to decrypt.
    fn init_bt_handshake(&mut self, buf: &mut [u8], consumed: usize) -> (usize, PeStep) {
        if self.rc4_selected {
            self.rc4.as_mut().expect("keys derived").decrypt(&mut buf[consumed..]);
        }
        (consumed, PeStep::Done { rc4: self.rc4_selected })
    }
}

/// libtorrent `random(512)`: a pad of 0 to 512 bytes.
fn random_pad_size() -> usize {
    random::random_below(513) as usize
}

fn write_pad(out: &mut Vec<u8>, n: usize) {
    let start = out.len();
    out.resize(start + n, 0);
    for chunk in out[start..].chunks_mut(8) {
        let r = random::random_u64().to_le_bytes();
        chunk.copy_from_slice(&r[..chunk.len()]);
    }
}

/// `write_pe_vc_cryptofield`: VC, the crypto field, `len(pad)`, the pad and, for the initiator,
/// `len(IA)`.
fn write_vc_cryptofield(out: &mut Vec<u8>, crypto_field: u32, pad_size: usize, len_ia: Option<u16>) {
    debug_assert!(crypto_field > 0 && crypto_field <= 3);
    out.extend_from_slice(&[0u8; 8]);
    out.extend_from_slice(&crypto_field.to_be_bytes());
    out.extend_from_slice(&(pad_size as u16).to_be_bytes());
    write_pad(out, pad_size);
    if let Some(len) = len_ia {
        out.extend_from_slice(&len.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Sha1;
    use crate::proto::peer_wire;

    fn handshake_for(ih: &Sha1Hash) -> Vec<u8> {
        let mut v = Vec::new();
        peer_wire::encode_handshake(&mut v, &peer_wire::Reserved::ours(), ih, b"-PA0100-abcdefghijkl");
        v
    }

    /// Runs both sides to completion, delivering bytes in `chunk`-sized pieces (0 = all at once).
    /// Returns (initiator's choice, responder's choice, what the responder sees first).
    fn exchange(
        a_level: EncLevel,
        b_level: EncLevel,
        b_prefer_rc4: bool,
        chunk: usize,
    ) -> Result<(bool, bool, Vec<u8>), PeError> {
        let ih = Sha1::digest(b"torrent");
        let other = Sha1::digest(b"another torrent");
        let mut find = |h: &Sha1Hash| [ih, other].into_iter().find(|c| mse::stream_key_hash(c) == *h);
        let mut a_out = Vec::new();
        let mut a = PeHandshake::outgoing(ih, handshake_for(&ih), a_level, false, &mut a_out);
        let mut b = PeHandshake::incoming(b_level, b_prefer_rc4);
        let mut b_out = Vec::new();
        // Bytes in flight towards each side.
        let (mut to_a, mut to_b): (Vec<u8>, Vec<u8>) = (Vec::new(), a_out.split_off(0));
        // Unprocessed receive buffers.
        let (mut a_buf, mut b_buf): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
        let mut a_done = None;
        let mut b_done = None;
        let mut attached = false;
        for _ in 0..10_000 {
            if a_done.is_some() && b_done.is_some() {
                break;
            }
            let take = |from: &mut Vec<u8>, to: &mut Vec<u8>| {
                let n = if chunk == 0 { from.len() } else { chunk.min(from.len()) };
                to.extend(from.drain(..n));
            };
            take(&mut to_b, &mut b_buf);
            take(&mut to_a, &mut a_buf);
            if b_done.is_none() {
                let (n, step) = b.feed(&mut b_buf, &mut b_out, &mut find)?;
                b_buf.drain(..n);
                to_a.append(&mut b_out);
                match step {
                    PeStep::Torrent(h) => {
                        assert_eq!(h, ih);
                        attached = true;
                    }
                    PeStep::Done { rc4 } => b_done = Some(rc4),
                    _ => {}
                }
            }
            if a_done.is_none() {
                let (n, step) = a.feed(&mut a_buf, &mut a_out, &mut find)?;
                a_buf.drain(..n);
                to_b.append(&mut a_out);
                if let PeStep::Done { rc4 } = step {
                    a_done = Some(rc4);
                }
            }
        }
        assert!(attached);
        let (a_rc4, b_rc4) = (a_done.expect("initiator finished"), b_done.expect("responder finished"));
        // After the handshake, the payload streams must line up.
        if a_rc4 {
            let mut ar = a.into_rc4().unwrap();
            let mut br = b.into_rc4().unwrap();
            let mut msg = *b"\0\0\0\x01\x02";
            ar.encrypt(&mut msg);
            br.decrypt(&mut msg);
            assert_eq!(&msg, b"\0\0\0\x01\x02");
            br.encrypt(&mut msg);
            ar.decrypt(&mut msg);
            assert_eq!(&msg, b"\0\0\0\x01\x02");
        }
        Ok((a_rc4, b_rc4, b_buf))
    }

    #[test]
    fn both_sides_agree_and_the_handshake_comes_through() {
        for chunk in [0, 1, 7, 100] {
            let (a, b, ia) = exchange(EncLevel::Both, EncLevel::Both, false, chunk).unwrap();
            // Both allow both; the responder keeps the least significant bit: plaintext.
            assert!(!a && !b, "chunk {chunk}");
            let ih = Sha1::digest(b"torrent");
            assert_eq!(ia, handshake_for(&ih), "chunk {chunk}");
        }
    }

    #[test]
    fn prefer_rc4_and_single_levels() {
        assert_eq!(exchange(EncLevel::Both, EncLevel::Both, true, 0).map(|r| (r.0, r.1)), Ok((true, true)));
        assert_eq!(exchange(EncLevel::Rc4, EncLevel::Both, false, 3).map(|r| (r.0, r.1)), Ok((true, true)));
        assert_eq!(exchange(EncLevel::Both, EncLevel::Rc4, false, 0).map(|r| (r.0, r.1)), Ok((true, true)));
        assert_eq!(
            exchange(EncLevel::Plaintext, EncLevel::Both, true, 0).map(|r| (r.0, r.1)),
            Ok((false, false))
        );
        assert_eq!(exchange(EncLevel::Rc4, EncLevel::Rc4, false, 1).map(|r| (r.0, r.1)), Ok((true, true)));
    }

    #[test]
    fn incompatible_levels_are_refused_by_the_responder() {
        assert_eq!(
            exchange(EncLevel::Rc4, EncLevel::Plaintext, false, 0),
            Err(PeError::UnsupportedEncryptionMode)
        );
        assert_eq!(
            exchange(EncLevel::Plaintext, EncLevel::Rc4, true, 0),
            Err(PeError::UnsupportedEncryptionMode)
        );
    }

    #[test]
    fn an_unknown_torrent_is_refused() {
        let ih = Sha1::digest(b"nobody has this");
        let mut out = Vec::new();
        let mut a = PeHandshake::outgoing(ih, handshake_for(&ih), EncLevel::Both, false, &mut out);
        let mut b = PeHandshake::incoming(EncLevel::Both, false);
        let mut none = |_: &Sha1Hash| None;
        let mut b_out = Vec::new();
        let (n, step) = b.feed(&mut out, &mut b_out, &mut none).unwrap();
        assert_eq!((n, step), (DH_KEY_LEN, PeStep::Continue));
        let mut a_out = Vec::new();
        let (n, step) = a.feed(&mut b_out, &mut a_out, &mut none).unwrap();
        assert_eq!((n, step), (DH_KEY_LEN, PeStep::Continue));
        let mut rest = out[DH_KEY_LEN..].to_vec();
        rest.extend_from_slice(&a_out);
        // Skip the pad up to the sync hash, then fail on the stream key.
        let mut err = None;
        for _ in 0..10 {
            match b.feed(&mut rest, &mut b_out, &mut none) {
                Ok((n, _)) => {
                    rest.drain(..n);
                }
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        assert_eq!(err, Some(PeError::InvalidInfoHash));
    }

    #[test]
    fn garbage_instead_of_a_sync_hash_fails_after_512_bytes() {
        let mut b = PeHandshake::incoming(EncLevel::Both, false);
        let mut none = |_: &Sha1Hash| None;
        let mut out = Vec::new();
        let mut buf = vec![0x5a; DH_KEY_LEN];
        let (n, _) = b.feed(&mut buf, &mut out, &mut none).unwrap();
        assert_eq!(n, DH_KEY_LEN);
        assert!(out.len() >= DH_KEY_LEN && out.len() <= DH_KEY_LEN + 512);
        let mut garbage = vec![0x33u8; 300];
        let (n, step) = b.feed(&mut garbage, &mut out, &mut none).unwrap();
        assert_eq!((n, step), (280, PeStep::Continue));
        let mut more = vec![0x33u8; 300];
        assert_eq!(b.feed(&mut more, &mut out, &mut none), Err(PeError::SyncHashNotFound));
    }

    #[test]
    fn a_short_buffer_waits() {
        let mut b = PeHandshake::incoming(EncLevel::Both, false);
        let mut none = |_: &Sha1Hash| None;
        let mut out = Vec::new();
        let mut buf = vec![1u8; DH_KEY_LEN - 1];
        assert_eq!(b.feed(&mut buf, &mut out, &mut none), Ok((0, PeStep::Wait)));
        assert!(out.is_empty());
    }
}
