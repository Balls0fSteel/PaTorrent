// Portable compression ported from libtorrent-rasterbar 2.0.11: src/sha1.cpp
// ("SHA-1 in C", Steve Reid, public domain; C++ conversion by Arvid Norberg). The streaming API
// mirrors libtorrent's `hasher`. The SHA-NI path follows Intel's published instruction sequence
// for the SHA extensions. See THIRD-PARTY-NOTICES.md.
// Changes: the 80 unrolled round macros became a loop over a 16-word circular schedule (the
// same `blk()` expansion); block buffering is shared with SHA-256.

//! SHA-1 (libtorrent `hasher`).

use super::block::BlockBuffer;
use crate::types::Sha1Hash;

const INIT: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];

/// Incremental SHA-1. `Sha1::digest(data)` for one-shot hashing.
#[derive(Clone)]
pub struct Sha1 {
    state: [u32; 5],
    block: BlockBuffer,
}

impl Default for Sha1 {
    fn default() -> Sha1 {
        Sha1::new()
    }
}

impl Sha1 {
    pub const fn new() -> Sha1 {
        Sha1 { state: INIT, block: BlockBuffer::new() }
    }

    pub fn digest(data: &[u8]) -> Sha1Hash {
        let mut h = Sha1::new();
        h.update(data);
        h.finalize()
    }

    pub fn update(&mut self, data: &[u8]) -> &mut Sha1 {
        let state = &mut self.state;
        self.block.update(data, |blocks| compress(state, blocks));
        self
    }

    pub fn finalize(mut self) -> Sha1Hash {
        let state = &mut self.state;
        self.block.finish(|blocks| compress(state, blocks));
        let mut out = [0u8; 20];
        for (o, s) in out.chunks_exact_mut(4).zip(self.state) {
            o.copy_from_slice(&s.to_be_bytes());
        }
        Sha1Hash(out)
    }

    pub fn reset(&mut self) {
        *self = Sha1::new();
    }
}

/// Compresses whole 64-byte blocks with the fastest implementation this CPU supports.
fn compress(state: &mut [u32; 5], blocks: &[u8]) {
    debug_assert_eq!(blocks.len() % 64, 0);
    #[cfg(target_arch = "x86_64")]
    if super::sha_ni_available() {
        // SAFETY: the required CPU features were detected at runtime.
        unsafe { shani::compress(state, blocks) };
        return;
    }
    compress_portable(state, blocks);
}

/// Steve Reid's SHA-1 transform, one 64-byte block at a time.
pub(crate) fn compress_portable(state: &mut [u32; 5], blocks: &[u8]) {
    for block in blocks.chunks_exact(64) {
        let mut w = [0u32; 16];
        for (wi, b) in w.iter_mut().zip(block.chunks_exact(4)) {
            *wi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        let [mut a, mut b, mut c, mut d, mut e] = *state;
        for i in 0..80 {
            // blk(i): expand the schedule in place in the 16-word ring
            let wi = if i < 16 {
                w[i]
            } else {
                let x = (w[(i + 13) & 15] ^ w[(i + 8) & 15] ^ w[(i + 2) & 15] ^ w[i & 15]).rotate_left(1);
                w[i & 15] = x;
                x
            };
            let (f, k) = match i {
                0..=19 => ((b & (c ^ d)) ^ d, 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => (((b | c) & d) | (b & c), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) mod shani {
    use std::arch::x86_64::*;

    /// SHA-1 using the x86 SHA extensions: 4 rounds per `sha1rnds4`, message schedule via
    /// `sha1msg1` / `sha1msg2`, E carried through `sha1nexte`.
    ///
    /// # Safety
    /// The CPU must support `sha`, `sse2`, `ssse3` and `sse4.1`.
    #[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
    pub(crate) unsafe fn compress(state: &mut [u32; 5], blocks: &[u8]) {
        // SAFETY (whole body): loads/stores are unaligned and in bounds (chunks of 64 bytes,
        // `state` has 5 words of which we store 4 + 1); intrinsics are enabled by the attribute.
        unsafe {
            let mask = _mm_set_epi64x(0x0001_0203_0405_0607, 0x0809_0a0b_0c0d_0e0f);
            let mut abcd = _mm_loadu_si128(state.as_ptr() as *const __m128i);
            abcd = _mm_shuffle_epi32(abcd, 0x1B);
            let mut e0 = _mm_set_epi32(state[4] as i32, 0, 0, 0);

            for block in blocks.chunks_exact(64) {
                let p = block.as_ptr() as *const __m128i;
                let abcd_save = abcd;
                let e0_save = e0;

                // Rounds 0-3
                let mut msg0 = _mm_shuffle_epi8(_mm_loadu_si128(p), mask);
                e0 = _mm_add_epi32(e0, msg0);
                let mut e1 = abcd;
                abcd = _mm_sha1rnds4_epu32(abcd, e0, 0);

                // Rounds 4-7
                let mut msg1 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(1)), mask);
                e1 = _mm_sha1nexte_epu32(e1, msg1);
                e0 = abcd;
                abcd = _mm_sha1rnds4_epu32(abcd, e1, 0);
                msg0 = _mm_sha1msg1_epu32(msg0, msg1);

                // Rounds 8-11
                let mut msg2 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(2)), mask);
                e0 = _mm_sha1nexte_epu32(e0, msg2);
                e1 = abcd;
                abcd = _mm_sha1rnds4_epu32(abcd, e0, 0);
                msg1 = _mm_sha1msg1_epu32(msg1, msg2);
                msg0 = _mm_xor_si128(msg0, msg2);

                // Rounds 12-15
                let mut msg3 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(3)), mask);
                e1 = _mm_sha1nexte_epu32(e1, msg3);
                e0 = abcd;
                msg0 = _mm_sha1msg2_epu32(msg0, msg3);
                abcd = _mm_sha1rnds4_epu32(abcd, e1, 0);
                msg2 = _mm_sha1msg1_epu32(msg2, msg3);
                msg1 = _mm_xor_si128(msg1, msg3);

                // Rounds 16-63 follow one pattern per 4 rounds: the current message word feeds
                // E, the next one gets sha1msg2, the previous sha1msg1, the one after next xor.
                macro_rules! group {
                    ($ea:ident, $eb:ident, $cur:ident, $next:ident, $prev:ident, $other:ident, $f:literal) => {
                        $ea = _mm_sha1nexte_epu32($ea, $cur);
                        $eb = abcd;
                        $next = _mm_sha1msg2_epu32($next, $cur);
                        abcd = _mm_sha1rnds4_epu32(abcd, $ea, $f);
                        $prev = _mm_sha1msg1_epu32($prev, $cur);
                        $other = _mm_xor_si128($other, $cur);
                    };
                }
                group!(e0, e1, msg0, msg1, msg3, msg2, 0); // 16-19
                group!(e1, e0, msg1, msg2, msg0, msg3, 1); // 20-23
                group!(e0, e1, msg2, msg3, msg1, msg0, 1); // 24-27
                group!(e1, e0, msg3, msg0, msg2, msg1, 1); // 28-31
                group!(e0, e1, msg0, msg1, msg3, msg2, 1); // 32-35
                group!(e1, e0, msg1, msg2, msg0, msg3, 1); // 36-39
                group!(e0, e1, msg2, msg3, msg1, msg0, 2); // 40-43
                group!(e1, e0, msg3, msg0, msg2, msg1, 2); // 44-47
                group!(e0, e1, msg0, msg1, msg3, msg2, 2); // 48-51
                group!(e1, e0, msg1, msg2, msg0, msg3, 2); // 52-55
                group!(e0, e1, msg2, msg3, msg1, msg0, 2); // 56-59
                group!(e1, e0, msg3, msg0, msg2, msg1, 3); // 60-63
                group!(e0, e1, msg0, msg1, msg3, msg2, 3); // 64-67

                // Rounds 68-71: the schedule is winding down (no more sha1msg1)
                e1 = _mm_sha1nexte_epu32(e1, msg1);
                e0 = abcd;
                msg2 = _mm_sha1msg2_epu32(msg2, msg1);
                abcd = _mm_sha1rnds4_epu32(abcd, e1, 3);
                msg3 = _mm_xor_si128(msg3, msg1);

                // Rounds 72-75
                e0 = _mm_sha1nexte_epu32(e0, msg2);
                e1 = abcd;
                msg3 = _mm_sha1msg2_epu32(msg3, msg2);
                abcd = _mm_sha1rnds4_epu32(abcd, e0, 3);

                // Rounds 76-79
                e1 = _mm_sha1nexte_epu32(e1, msg3);
                e0 = abcd;
                abcd = _mm_sha1rnds4_epu32(abcd, e1, 3);

                // Add this block's result to the state
                e0 = _mm_sha1nexte_epu32(e0, e0_save);
                abcd = _mm_add_epi32(abcd, abcd_save);
            }

            abcd = _mm_shuffle_epi32(abcd, 0x1B);
            _mm_storeu_si128(state.as_mut_ptr() as *mut __m128i, abcd);
            state[4] = _mm_extract_epi32(e0, 3) as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    //! Vectors from libtorrent-rasterbar 2.0.11 test/test_hasher.cpp (RFC 3174 and
    //! di-mgt.com.au), run through both implementations.
    use super::*;

    const VECTORS: &[(&[u8], usize, &str)] = &[
        (b"abc", 1, "a9993e364706816aba3e25717850c26c9cd0d89d"),
        (b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq", 1, "84983e441c3bd26ebaae4aa1f95129e5e54670f1"),
        (
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
            1,
            "a49b2446a02c645bf419f995b67091253a04a259",
        ),
        (b"a", 1_000_000, "34aa973cd4c4daa4f61eeb2bdbad27316534016f"),
        (
            b"0123456701234567012345670123456701234567012345670123456701234567",
            10,
            "dea356a2cddd90c7a7ecedc5ebb563934f460452",
        ),
        (b"", 1, "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
    ];

    /// Runs `input` x `reps` through a hasher whose compression function is `f`.
    fn hash_with(f: fn(&mut [u32; 5], &[u8]), input: &[u8], reps: usize) -> String {
        let mut state = INIT;
        let mut block = BlockBuffer::new();
        for _ in 0..reps {
            block.update(input, |b| f(&mut state, b));
        }
        block.finish(|b| f(&mut state, b));
        state.iter().map(|s| format!("{s:08x}")).collect()
    }

    #[test]
    fn vectors_portable() {
        for &(input, reps, expected) in VECTORS {
            assert_eq!(hash_with(compress_portable, input, reps), expected);
        }
    }

    #[test]
    fn vectors_dispatch() {
        for &(input, reps, expected) in VECTORS {
            let mut h = Sha1::new();
            for _ in 0..reps {
                h.update(input);
            }
            assert_eq!(h.finalize().to_hex(), expected);
        }
        assert_eq!(Sha1::digest(b"abc").to_hex(), VECTORS[0].2);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn shani_matches_portable() {
        if !crate::crypto::sha_ni_available() {
            eprintln!("SHA-NI not available on this CPU; skipping");
            return;
        }
        fn shani(state: &mut [u32; 5], blocks: &[u8]) {
            // SAFETY: feature checked above.
            unsafe { shani::compress(state, blocks) }
        }
        for &(input, reps, expected) in VECTORS {
            assert_eq!(hash_with(shani, input, reps), expected);
        }
        // random lengths and split points
        let mut x = 0x1234_5678_9abc_def0u64;
        let data: Vec<u8> = (0..10_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        for len in [0, 1, 55, 56, 63, 64, 65, 127, 128, 1000, 9999] {
            assert_eq!(
                hash_with(shani, &data[..len], 1),
                hash_with(compress_portable, &data[..len], 1),
                "len {len}"
            );
        }
    }

    #[test]
    fn split_updates_match_one_shot() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        let whole = Sha1::digest(&data);
        for split in [0, 1, 63, 64, 65, 500, 999, 1000] {
            let mut h = Sha1::new();
            h.update(&data[..split]).update(&data[split..]);
            assert_eq!(h.finalize(), whole, "split at {split}");
        }
        let mut h = Sha1::new();
        h.update(b"garbage");
        h.reset();
        h.update(&data);
        assert_eq!(h.finalize(), whole);
    }

    /// 1 GiB through the hasher (test/test_hasher.cpp `hasher_test_vec1`). Slow in debug
    /// builds: run with `cargo test --release -- --ignored`.
    #[test]
    #[ignore]
    fn one_gibibyte() {
        let block = b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmno";
        let chunk: Vec<u8> = block.iter().copied().cycle().take(64 * 4096).collect();
        let mut h = Sha1::new();
        for _ in 0..(16_777_216 / 4096) {
            h.update(&chunk);
        }
        assert_eq!(h.finalize().to_hex(), "7789f0c9ef7bfc40d93311143dfbe69e2017f592");
    }
}
