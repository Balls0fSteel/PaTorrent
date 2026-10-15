// Portable compression ported from libtorrent-rasterbar 2.0.11: src/sha256.cpp ("SHA-256.
// Adapted from LibTomCrypt. This code is Public Domain"). The streaming API mirrors libtorrent's
// `hasher256`. The SHA-NI path follows Intel's published instruction sequence for the SHA
// extensions. See THIRD-PARTY-NOTICES.md.
// Changes: block buffering is shared with SHA-1.

//! SHA-256 (libtorrent `hasher256`), used for BitTorrent v2.

use super::block::BlockBuffer;
use crate::types::Sha256Hash;

const INIT: [u32; 8] =
    [0x6A09_E667, 0xBB67_AE85, 0x3C6E_F372, 0xA54F_F53A, 0x510E_527F, 0x9B05_688C, 0x1F83_D9AB, 0x5BE0_CD19];

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Incremental SHA-256. `Sha256::digest(data)` for one-shot hashing.
#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    block: BlockBuffer,
}

impl Default for Sha256 {
    fn default() -> Sha256 {
        Sha256::new()
    }
}

impl Sha256 {
    pub const fn new() -> Sha256 {
        Sha256 { state: INIT, block: BlockBuffer::new() }
    }

    pub fn digest(data: &[u8]) -> Sha256Hash {
        let mut h = Sha256::new();
        h.update(data);
        h.finalize()
    }

    pub fn update(&mut self, data: &[u8]) -> &mut Sha256 {
        let state = &mut self.state;
        self.block.update(data, |blocks| compress(state, blocks));
        self
    }

    pub fn finalize(mut self) -> Sha256Hash {
        let state = &mut self.state;
        self.block.finish(|blocks| compress(state, blocks));
        let mut out = [0u8; 32];
        for (o, s) in out.chunks_exact_mut(4).zip(self.state) {
            o.copy_from_slice(&s.to_be_bytes());
        }
        Sha256Hash(out)
    }

    pub fn reset(&mut self) {
        *self = Sha256::new();
    }
}

fn compress(state: &mut [u32; 8], blocks: &[u8]) {
    debug_assert_eq!(blocks.len() % 64, 0);
    #[cfg(target_arch = "x86_64")]
    if super::sha_ni_available() {
        // SAFETY: the required CPU features were detected at runtime.
        unsafe { shani::compress(state, blocks) };
        return;
    }
    compress_portable(state, blocks);
}

/// LibTomCrypt's `sha_compress`, one 64-byte block at a time.
pub(crate) fn compress_portable(state: &mut [u32; 8], blocks: &[u8]) {
    for block in blocks.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (wi, b) in w.iter_mut().zip(block.chunks_exact(4)) {
            *wi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 16..64 {
            let gamma0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let gamma1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = gamma1.wrapping_add(w[i - 7]).wrapping_add(gamma0).wrapping_add(w[i - 16]);
        }
        let mut s = *state;
        for i in 0..64 {
            let [a, b, c, _, e, f, g, h] = s;
            let sigma1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = g ^ (e & (f ^ g));
            let t0 = h.wrapping_add(sigma1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let sigma0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = ((a | b) & c) | (a & b);
            let t1 = sigma0.wrapping_add(maj);
            s[3] = s[3].wrapping_add(t0);
            s[7] = t0.wrapping_add(t1);
            s.rotate_right(1);
        }
        for (st, v) in state.iter_mut().zip(s) {
            *st = st.wrapping_add(v);
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) mod shani {
    use super::K;
    use std::arch::x86_64::*;

    /// SHA-256 using the x86 SHA extensions (`sha256rnds2` does two rounds; the state is kept as
    /// ABEF / CDGH register pairs).
    ///
    /// # Safety
    /// The CPU must support `sha`, `sse2`, `ssse3` and `sse4.1`.
    #[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
    pub(crate) unsafe fn compress(state: &mut [u32; 8], blocks: &[u8]) {
        // SAFETY (whole body): unaligned loads/stores within `state`, `K` and 64-byte chunks;
        // intrinsics are enabled by the attribute.
        unsafe {
            let mask = _mm_set_epi64x(0x0c0d_0e0f_0809_0a0b, 0x0405_0607_0001_0203);
            let k = |i: usize| _mm_loadu_si128(K.as_ptr().add(4 * i) as *const __m128i);

            let tmp = _mm_shuffle_epi32(_mm_loadu_si128(state.as_ptr() as *const __m128i), 0xB1); // CDAB
            let mut state1 =
                _mm_shuffle_epi32(_mm_loadu_si128(state.as_ptr().add(4) as *const __m128i), 0x1B); // EFGH
            let mut state0 = _mm_alignr_epi8(tmp, state1, 8); // ABEF
            state1 = _mm_blend_epi16(state1, tmp, 0xF0); // CDGH

            for block in blocks.chunks_exact(64) {
                let p = block.as_ptr() as *const __m128i;
                let abef_save = state0;
                let cdgh_save = state1;

                // Four rounds: two `sha256rnds2` with the low then high half of message + K.
                macro_rules! rounds {
                    ($m:expr, $g:expr) => {
                        let msg = _mm_add_epi32($m, k($g));
                        state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
                        state0 = _mm_sha256rnds2_epu32(state0, state1, _mm_shuffle_epi32(msg, 0x0E));
                    };
                }
                // Message schedule: next += alignr(cur, prev) then sha256msg2; prev gets msg1.
                macro_rules! schedule {
                    ($cur:ident, $prev:ident, $next:ident) => {
                        $next =
                            _mm_sha256msg2_epu32(_mm_add_epi32($next, _mm_alignr_epi8($cur, $prev, 4)), $cur);
                    };
                }

                let mut msg0 = _mm_shuffle_epi8(_mm_loadu_si128(p), mask);
                rounds!(msg0, 0);
                let mut msg1 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(1)), mask);
                rounds!(msg1, 1);
                msg0 = _mm_sha256msg1_epu32(msg0, msg1);
                let mut msg2 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(2)), mask);
                rounds!(msg2, 2);
                msg1 = _mm_sha256msg1_epu32(msg1, msg2);
                let mut msg3 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(3)), mask);
                rounds!(msg3, 3);
                schedule!(msg3, msg2, msg0);
                msg2 = _mm_sha256msg1_epu32(msg2, msg3);

                // Rounds 16-51: the same step with the message registers rotating.
                macro_rules! step {
                    ($cur:ident, $prev:ident, $next:ident, $g:expr) => {
                        rounds!($cur, $g);
                        schedule!($cur, $prev, $next);
                        $prev = _mm_sha256msg1_epu32($prev, $cur);
                    };
                }
                step!(msg0, msg3, msg1, 4);
                step!(msg1, msg0, msg2, 5);
                step!(msg2, msg1, msg3, 6);
                step!(msg3, msg2, msg0, 7);
                step!(msg0, msg3, msg1, 8);
                step!(msg1, msg0, msg2, 9);
                step!(msg2, msg1, msg3, 10);
                step!(msg3, msg2, msg0, 11);
                step!(msg0, msg3, msg1, 12);

                // Rounds 52-63: the schedule winds down.
                rounds!(msg1, 13);
                schedule!(msg1, msg0, msg2);
                rounds!(msg2, 14);
                schedule!(msg2, msg1, msg3);
                rounds!(msg3, 15);

                state0 = _mm_add_epi32(state0, abef_save);
                state1 = _mm_add_epi32(state1, cdgh_save);
            }

            let tmp = _mm_shuffle_epi32(state0, 0x1B); // FEBA
            state1 = _mm_shuffle_epi32(state1, 0xB1); // DCHG
            let dcba = _mm_blend_epi16(tmp, state1, 0xF0);
            let hgfe = _mm_alignr_epi8(state1, tmp, 8);
            _mm_storeu_si128(state.as_mut_ptr() as *mut __m128i, dcba);
            _mm_storeu_si128(state.as_mut_ptr().add(4) as *mut __m128i, hgfe);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Vectors from libtorrent-rasterbar 2.0.11 test/test_hasher.cpp (dlitz.net), plus the
    //! empty string, run through both implementations.
    use super::*;

    const VECTORS: &[(&[u8], usize, &str)] = &[
        (b"abc", 1, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        (
            b"\xde\x18\x89\x41\xa3\x37\x5d\x3a\x8a\x06\x1e\x67\x57\x6e\x92\x6d",
            1,
            "067c531269735ca7f541fdaca8f0dc76305d3cada140f89372a410fe5eff6e4d",
        ),
        (b"a", 1_000_000, "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"),
        (b"", 1, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            1,
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
    ];

    fn hash_with(f: fn(&mut [u32; 8], &[u8]), input: &[u8], reps: usize) -> String {
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
            let mut h = Sha256::new();
            for _ in 0..reps {
                h.update(input);
            }
            assert_eq!(h.finalize().to_hex(), expected);
        }
        assert_eq!(Sha256::digest(b"abc").to_hex(), VECTORS[0].2);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn shani_matches_portable() {
        if !crate::crypto::sha_ni_available() {
            eprintln!("SHA-NI not available on this CPU; skipping");
            return;
        }
        fn shani(state: &mut [u32; 8], blocks: &[u8]) {
            // SAFETY: feature checked above.
            unsafe { shani::compress(state, blocks) }
        }
        for &(input, reps, expected) in VECTORS {
            assert_eq!(hash_with(shani, input, reps), expected);
        }
        let mut x = 0xdead_beef_cafe_f00du64;
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
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 13 + 1) as u8).collect();
        let whole = Sha256::digest(&data);
        for split in [0, 1, 63, 64, 65, 500, 1000] {
            let mut h = Sha256::new();
            h.update(&data[..split]).update(&data[split..]);
            assert_eq!(h.finalize(), whole, "split at {split}");
        }
    }
}
