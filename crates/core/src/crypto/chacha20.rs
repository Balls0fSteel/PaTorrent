// Written from RFC 8439 section 2.3 (the ChaCha20 block function); not ported from libtorrent,
// which uses the OS or OpenSSL generators.

//! The ChaCha20 block function, for the session's random number generator
//! ([`crate::util::random`]). Not used for encryption.

const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

#[inline(always)]
fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// One 64-byte keystream block for `key`, block `counter` and `nonce` (RFC 8439 2.3).
pub fn block(key: &[u8; 32], counter: u32, nonce: &[u8; 12], out: &mut [u8; 64]) {
    let word = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut init = [0u32; 16];
    init[..4].copy_from_slice(&CONSTANTS);
    for i in 0..8 {
        init[4 + i] = word(&key[4 * i..]);
    }
    init[12] = counter;
    for i in 0..3 {
        init[13 + i] = word(&nonce[4 * i..]);
    }
    let mut s = init;
    for _ in 0..10 {
        quarter_round(&mut s, 0, 4, 8, 12);
        quarter_round(&mut s, 1, 5, 9, 13);
        quarter_round(&mut s, 2, 6, 10, 14);
        quarter_round(&mut s, 3, 7, 11, 15);
        quarter_round(&mut s, 0, 5, 10, 15);
        quarter_round(&mut s, 1, 6, 11, 12);
        quarter_round(&mut s, 2, 7, 8, 13);
        quarter_round(&mut s, 3, 4, 9, 14);
    }
    for i in 0..16 {
        out[4 * i..4 * i + 4].copy_from_slice(&s[i].wrapping_add(init[i]).to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex::encode as to_hex;

    /// RFC 8439 2.3.2.
    #[test]
    fn rfc8439_block() {
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        let nonce = [0, 0, 0, 9, 0, 0, 0, 0x4a, 0, 0, 0, 0];
        let mut out = [0u8; 64];
        block(&key, 1, &nonce, &mut out);
        assert_eq!(
            to_hex(&out),
            "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e\
             d2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e"
        );
    }
}
