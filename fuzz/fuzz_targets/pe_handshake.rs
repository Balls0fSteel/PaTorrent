//! The MSE/PE encrypted handshake (`proto::pe_handshake`), on both sides: raw bytes straight
//! into each state machine, then a real exchange between an initiator and a responder where the
//! input decides the chunk sizes and corrupts bytes in flight, so the later states (verification
//! constant, crypto field, pad, initial payload) see hostile data too.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::EncLevel;
use patorrent_core::crypto::Sha1;
use patorrent_core::fuzzing::{PeHandshake, PeStep};

const LEVELS: [EncLevel; 3] = [EncLevel::Plaintext, EncLevel::Rc4, EncLevel::Both];

fn handshake(ih: &patorrent_core::Sha1Hash) -> Vec<u8> {
    let mut v = vec![19];
    v.extend_from_slice(b"BitTorrent protocol");
    v.extend_from_slice(&[0u8; 8]);
    v.extend_from_slice(&ih.0);
    v.extend_from_slice(b"-PA0100-fuzzfuzzfuzz");
    v
}

/// Feeds `data` to one machine in the given chunks until it fails, finishes or runs dry.
fn feed_raw(
    hs: &mut PeHandshake,
    data: &[u8],
    chunk: usize,
    find: &mut dyn FnMut(&patorrent_core::Sha1Hash) -> Option<patorrent_core::Sha1Hash>,
) {
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut rest = data;
    loop {
        let n = chunk.clamp(1, rest.len().max(1)).min(rest.len());
        buf.extend_from_slice(&rest[..n]);
        rest = &rest[n..];
        loop {
            match hs.feed(&mut buf, &mut out, find) {
                Ok((consumed, step)) => {
                    assert!(consumed <= buf.len());
                    buf.drain(..consumed);
                    match step {
                        PeStep::Wait => break,
                        PeStep::Done { .. } => return,
                        PeStep::Continue | PeStep::Torrent(_) => {}
                    }
                }
                Err(_) => return,
            }
        }
        // A responder must never send more than its key, pad and the pe4 reply.
        assert!(out.len() <= 96 + 512 + 8 + 4 + 2 + 512);
        if rest.is_empty() {
            return;
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let ih = Sha1::digest(b"fuzz torrent");
    let mut find = |h: &patorrent_core::Sha1Hash| (*h != patorrent_core::Sha1Hash::default()).then_some(ih);
    let chunk = data.first().map_or(0, |&b| b as usize);
    let level = LEVELS[data.get(1).map_or(0, |&b| b as usize) % 3];
    let prefer_rc4 = data.get(2).is_some_and(|b| b & 1 != 0);

    // 1. Raw input into a responder and into an initiator.
    let mut b = PeHandshake::incoming(level, prefer_rc4);
    feed_raw(&mut b, data, chunk, &mut find);
    let mut out = Vec::new();
    let mut a = PeHandshake::outgoing(ih, handshake(&ih), level, prefer_rc4, &mut out);
    feed_raw(&mut a, data, chunk, &mut find);

    // 2. A real exchange, with the input picking chunk sizes and bytes to corrupt.
    let mut script = data.iter().copied().skip(3);
    let mut a_out = Vec::new();
    let mut a = PeHandshake::outgoing(ih, handshake(&ih), level, prefer_rc4, &mut a_out);
    let mut b = PeHandshake::incoming(LEVELS[data.get(3).map_or(2, |&b| b as usize) % 3], !prefer_rc4);
    let mut b_out = Vec::new();
    let (mut to_b, mut to_a) = (std::mem::take(&mut a_out), Vec::new());
    let (mut a_buf, mut b_buf) = (Vec::new(), Vec::new());
    let (mut a_done, mut b_done) = (false, false);
    let mut exact_find = |h: &patorrent_core::Sha1Hash| {
        let mut want = Sha1::new();
        want.update(b"req2").update(&ih.0);
        (want.finalize() == *h).then_some(ih)
    };
    for _ in 0..2_000 {
        if a_done && b_done {
            break;
        }
        let mut deliver = |from: &mut Vec<u8>, to: &mut Vec<u8>| {
            let n = script.next().map_or(from.len(), |b| (b as usize * 7).min(from.len()));
            let start = to.len();
            to.extend(from.drain(..n));
            // Flip a byte now and then.
            let corrupt =
                script.next().filter(|&p| p & 3 == 0).and_then(|pos| to[start..].get_mut(pos as usize));
            if let Some(byte) = corrupt {
                *byte ^= script.next().unwrap_or(0xff);
            }
        };
        deliver(&mut to_b, &mut b_buf);
        deliver(&mut to_a, &mut a_buf);
        if !b_done {
            match b.feed(&mut b_buf, &mut b_out, &mut exact_find) {
                Ok((n, step)) => {
                    b_buf.drain(..n);
                    to_a.append(&mut b_out);
                    b_done = matches!(step, PeStep::Done { .. });
                }
                Err(_) => b_done = true,
            }
        }
        if !a_done {
            match a.feed(&mut a_buf, &mut a_out, &mut exact_find) {
                Ok((n, step)) => {
                    a_buf.drain(..n);
                    to_b.append(&mut a_out);
                    a_done = matches!(step, PeStep::Done { .. });
                }
                Err(_) => a_done = true,
            }
        }
    }
});
