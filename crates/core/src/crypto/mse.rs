// Ported from libtorrent-rasterbar 2.0.11: src/pe_crypto.cpp, include/libtorrent/pe_crypto.hpp
// (dh_key_exchange, rc4_handler, rc4_init / rc4_encrypt from libTomCrypt), the key derivation of
// src/bt_peer_connection.cpp (init_pe_rc4_handler, write_pe3_sync)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the 768-bit modular exponentiation runs on a fixed-width Montgomery multiplier of our
// own instead of boost::multiprecision; the secret exponent comes from the OS RNG; the xor mask
// hashes the secret at its full 96 bytes (libtorrent hashes an unpadded export, see PORTING.md).

//! MSE/PE ("Message Stream Encryption") primitives: RC4 as the protocol uses it, the fixed
//! 768-bit Diffie-Hellman exchange and the SHA-1 derived keys. The handshake itself is
//! `proto::pe_handshake`.

use std::cmp::Ordering;

use super::Sha1;
use crate::types::Sha1Hash;
use crate::util::random;

/// Public keys and the shared secret are exchanged as 96 big-endian bytes.
pub const DH_KEY_LEN: usize = 96;

const LIMBS: usize = DH_KEY_LEN / 8;

/// The MSE prime (768 bits), big-endian; the generator is 2.
const DH_PRIME: [u8; DH_KEY_LEN] = hex96(
    b"FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DD\
      EF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A63A36210000000000090563",
);

const fn hex96(s: &[u8; 2 * DH_KEY_LEN]) -> [u8; DH_KEY_LEN] {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("not a hex digit"),
        }
    }
    let mut out = [0u8; DH_KEY_LEN];
    let mut i = 0;
    while i < DH_KEY_LEN {
        out[i] = (nibble(s[2 * i]) << 4) | nibble(s[2 * i + 1]);
        i += 1;
    }
    out
}

// --- 768-bit unsigned integers -------------------------------------------------------------------

/// A 768-bit unsigned integer; little-endian limbs (libtorrent `key_t`, a 768-bit `cpp_int`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct U768([u64; LIMBS]);

impl U768 {
    const ZERO: U768 = U768([0; LIMBS]);
    const ONE: U768 = {
        let mut v = [0u64; LIMBS];
        v[0] = 1;
        U768(v)
    };

    fn from_be_bytes(b: &[u8; DH_KEY_LEN]) -> U768 {
        let mut v = [0u64; LIMBS];
        for (i, chunk) in b.chunks_exact(8).enumerate() {
            v[LIMBS - 1 - i] = u64::from_be_bytes(chunk.try_into().expect("8 bytes"));
        }
        U768(v)
    }

    /// Fixed-width big-endian export (libtorrent `export_key`: `export_bits` right-aligned to
    /// 96 bytes).
    fn to_be_bytes(self) -> [u8; DH_KEY_LEN] {
        let mut out = [0u8; DH_KEY_LEN];
        for (i, chunk) in out.chunks_exact_mut(8).enumerate() {
            chunk.copy_from_slice(&self.0[LIMBS - 1 - i].to_be_bytes());
        }
        out
    }

    fn cmp(&self, o: &U768) -> Ordering {
        for i in (0..LIMBS).rev() {
            match self.0[i].cmp(&o.0[i]) {
                Ordering::Equal => continue,
                ord => return ord,
            }
        }
        Ordering::Equal
    }

    /// `self -= o`, returning the borrow out of the top limb.
    fn sub_assign(&mut self, o: &U768) -> bool {
        let mut borrow = false;
        for i in 0..LIMBS {
            let (d, b1) = self.0[i].overflowing_sub(o.0[i]);
            let (d, b2) = d.overflowing_sub(borrow as u64);
            self.0[i] = d;
            borrow = b1 || b2;
        }
        borrow
    }

    /// `self <<= 1`, returning the bit shifted out.
    fn shl1(&mut self) -> bool {
        let mut carry = false;
        for limb in &mut self.0 {
            let next = *limb >> 63 != 0;
            *limb = (*limb << 1) | carry as u64;
            carry = next;
        }
        carry
    }

    fn bit(&self, i: usize) -> bool {
        (self.0[i / 64] >> (i % 64)) & 1 != 0
    }

    fn bits(&self) -> usize {
        for i in (0..LIMBS).rev() {
            if self.0[i] != 0 {
                return i * 64 + 64 - self.0[i].leading_zeros() as usize;
            }
        }
        0
    }
}

/// Montgomery arithmetic modulo the DH prime (`R = 2^768`).
struct Montgomery {
    p: U768,
    /// `-p^-1 mod 2^64`.
    n0inv: u64,
    /// `R^2 mod p`, to enter the Montgomery domain with one multiplication.
    r2: U768,
}

impl Montgomery {
    fn new(p: U768) -> Montgomery {
        debug_assert!(p.0[0] & 1 == 1 && p.0[LIMBS - 1] >> 63 == 1, "an odd 768-bit modulus");
        // Newton iteration for the inverse of p[0] modulo 2^64: each step doubles the correct bits.
        let p0 = p.0[0];
        let mut x = 1u64;
        for _ in 0..6 {
            x = x.wrapping_mul(2u64.wrapping_sub(p0.wrapping_mul(x)));
        }
        debug_assert_eq!(p0.wrapping_mul(x), 1);
        let n0inv = x.wrapping_neg();
        // R mod p = R - p (p has its top bit set, so p < R < 2p), then doubled 768 times.
        let mut r = U768::ZERO;
        r.sub_assign(&p);
        let mut r2 = r;
        for _ in 0..(LIMBS * 64) {
            let carry = r2.shl1();
            if carry || r2.cmp(&p) != Ordering::Less {
                r2.sub_assign(&p);
            }
        }
        Montgomery { p, n0inv, r2 }
    }

    /// `a * b / R mod p` for `a, b < p` (CIOS).
    #[allow(clippy::needless_range_loop)] // the limb indices are the algorithm
    fn mul(&self, a: &U768, b: &U768) -> U768 {
        let p = &self.p.0;
        let mut t = [0u64; LIMBS + 2];
        for i in 0..LIMBS {
            let bi = b.0[i] as u128;
            let mut c = 0u64;
            for j in 0..LIMBS {
                let s = (a.0[j] as u128) * bi + t[j] as u128 + c as u128;
                t[j] = s as u64;
                c = (s >> 64) as u64;
            }
            let s = t[LIMBS] as u128 + c as u128;
            t[LIMBS] = s as u64;
            t[LIMBS + 1] = (s >> 64) as u64;

            let m = t[0].wrapping_mul(self.n0inv) as u128;
            let s = m * (p[0] as u128) + t[0] as u128;
            let mut c = (s >> 64) as u64;
            for j in 1..LIMBS {
                let s = m * (p[j] as u128) + t[j] as u128 + c as u128;
                t[j - 1] = s as u64;
                c = (s >> 64) as u64;
            }
            let s = t[LIMBS] as u128 + c as u128;
            t[LIMBS - 1] = s as u64;
            t[LIMBS] = t[LIMBS + 1] + (s >> 64) as u64;
            t[LIMBS + 1] = 0;
        }
        let mut r = U768(t[..LIMBS].try_into().expect("LIMBS limbs"));
        // The result is below 2p: one conditional subtraction.
        if t[LIMBS] != 0 || r.cmp(&self.p) != Ordering::Less {
            r.sub_assign(&self.p);
        }
        r
    }

    /// `base ^ exp mod p` (boost `powm`); `base` may be any 768-bit value.
    fn powm(&self, base: &U768, exp: &U768) -> U768 {
        let mut base = *base;
        if base.cmp(&self.p) != Ordering::Less {
            base.sub_assign(&self.p); // base < 2^768 < 2p
        }
        let b = self.mul(&base, &self.r2);
        let mut acc = self.mul(&U768::ONE, &self.r2); // 1 in the Montgomery domain
        for i in (0..exp.bits()).rev() {
            acc = self.mul(&acc, &acc);
            if exp.bit(i) {
                acc = self.mul(&acc, &b);
            }
        }
        self.mul(&acc, &U768::ONE)
    }
}

fn montgomery() -> &'static Montgomery {
    static M: std::sync::OnceLock<Montgomery> = std::sync::OnceLock::new();
    M.get_or_init(|| Montgomery::new(U768::from_be_bytes(&DH_PRIME)))
}

// --- Diffie-Hellman --------------------------------------------------------------------------------

/// libtorrent `dh_key_exchange`: one side of the 768-bit exchange.
pub struct DhKeyExchange {
    local_key: [u8; DH_KEY_LEN],
    local_secret: U768,
    shared_secret: [u8; DH_KEY_LEN],
    xor_mask: Sha1Hash,
}

impl Default for DhKeyExchange {
    fn default() -> DhKeyExchange {
        DhKeyExchange::new()
    }
}

impl DhKeyExchange {
    /// A random secret and its public key `2 ^ secret mod p`.
    pub fn new() -> DhKeyExchange {
        let mut secret = [0u8; DH_KEY_LEN];
        random::crypto_random_bytes(&mut secret);
        DhKeyExchange::from_secret(&secret)
    }

    /// The exchange for a given secret exponent (tests and known answers).
    pub fn from_secret(secret: &[u8; DH_KEY_LEN]) -> DhKeyExchange {
        let local_secret = U768::from_be_bytes(secret);
        let two = U768([2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let local_key = montgomery().powm(&two, &local_secret).to_be_bytes();
        DhKeyExchange {
            local_key,
            local_secret,
            shared_secret: [0; DH_KEY_LEN],
            xor_mask: Sha1Hash::default(),
        }
    }

    /// Our public key, as sent on the wire.
    pub fn local_key(&self) -> &[u8; DH_KEY_LEN] {
        &self.local_key
    }

    /// Derives the shared secret `remote ^ secret mod p` from the peer's public key, and the
    /// xor mask `SHA-1("req3", S)` for the obfuscated stream-key hash. `false` (nothing derived)
    /// for a key outside `2..=p-2`: 0, 1 and p-1 (or anything from p up) give a secret that
    /// anyone can tell without our exponent (SEC-10).
    pub fn compute_secret(&mut self, remote_pubkey: &[u8; DH_KEY_LEN]) -> bool {
        let remote = U768::from_be_bytes(remote_pubkey);
        let two = U768([2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let mut p_minus_2 = montgomery().p;
        p_minus_2.sub_assign(&two);
        if remote.cmp(&two) == Ordering::Less || remote.cmp(&p_minus_2) == Ordering::Greater {
            return false;
        }
        self.shared_secret = montgomery().powm(&remote, &self.local_secret).to_be_bytes();
        self.xor_mask = req_hash(b"req3", &self.shared_secret);
        true
    }

    /// The shared secret `S`, big-endian, after [`compute_secret`](Self::compute_secret).
    pub fn secret(&self) -> &[u8; DH_KEY_LEN] {
        &self.shared_secret
    }

    pub fn hash_xor_mask(&self) -> &Sha1Hash {
        &self.xor_mask
    }
}

fn req_hash(tag: &[u8; 4], data: &[u8]) -> Sha1Hash {
    let mut h = Sha1::new();
    h.update(tag).update(data);
    h.finalize()
}

/// The sync hash `SHA-1("req1", S)` that starts the initiator's third message.
pub fn sync_hash(secret: &[u8; DH_KEY_LEN]) -> Sha1Hash {
    req_hash(b"req1", secret)
}

/// `SHA-1("req2", SKEY)`: the stream key (info-hash) hash, sent xor-ed with the mask.
pub fn stream_key_hash(info_hash: &Sha1Hash) -> Sha1Hash {
    req_hash(b"req2", &info_hash.0)
}

// --- RC4 ---------------------------------------------------------------------------------------------

/// RC4 state (libTomCrypt via libtorrent `rc4`).
#[derive(Clone)]
pub struct Rc4 {
    x: u8,
    y: u8,
    s: [u8; 256],
}

impl Rc4 {
    /// Key schedule (`rc4_init`); keys longer than 256 bytes are truncated.
    pub fn new(key: &[u8]) -> Rc4 {
        let key = &key[..key.len().min(256)];
        let mut s = [0u8; 256];
        for (i, v) in s.iter_mut().enumerate() {
            *v = i as u8;
        }
        if !key.is_empty() {
            let mut y = 0u8;
            let mut j = 0usize;
            for x in 0..256 {
                y = y.wrapping_add(s[x]).wrapping_add(key[j]);
                j += 1;
                if j == key.len() {
                    j = 0;
                }
                s.swap(x, y as usize);
            }
        }
        Rc4 { x: 0, y: 0, s }
    }

    /// Xors the keystream over `buf` (`rc4_encrypt`); encryption and decryption are the same.
    pub fn apply(&mut self, buf: &mut [u8]) {
        let (mut x, mut y) = (self.x, self.y);
        for b in buf {
            x = x.wrapping_add(1);
            y = y.wrapping_add(self.s[x as usize]);
            self.s.swap(x as usize, y as usize);
            let k = self.s[x as usize].wrapping_add(self.s[y as usize]);
            *b ^= self.s[k as usize];
        }
        self.x = x;
        self.y = y;
    }
}

/// libtorrent `rc4_handler`: one RC4 stream per direction, each with its first 1024 keystream
/// bytes discarded.
#[derive(Clone)]
pub struct Rc4Handler {
    incoming: Rc4,
    outgoing: Rc4,
    encrypt: bool,
    decrypt: bool,
}

impl Default for Rc4Handler {
    fn default() -> Rc4Handler {
        Rc4Handler::new()
    }
}

impl Rc4Handler {
    /// No keys yet: `encrypt` and `decrypt` leave data alone until the keys are set.
    pub fn new() -> Rc4Handler {
        Rc4Handler { incoming: Rc4::new(&[]), outgoing: Rc4::new(&[]), encrypt: false, decrypt: false }
    }

    /// The handler for a connection (`init_pe_rc4_handler`): the initiator sends with
    /// `SHA-1("keyA", S, SKEY)` and receives with `SHA-1("keyB", S, SKEY)`; the other side the
    /// reverse.
    pub fn for_connection(secret: &[u8; DH_KEY_LEN], stream_key: &Sha1Hash, outgoing: bool) -> Rc4Handler {
        let key = |tag: &[u8; 4]| {
            let mut h = Sha1::new();
            h.update(tag).update(secret).update(&stream_key.0);
            h.finalize()
        };
        let (key_a, key_b) = (key(b"keyA"), key(b"keyB"));
        let (local, remote) = if outgoing { (key_a, key_b) } else { (key_b, key_a) };
        let mut rc4 = Rc4Handler::new();
        rc4.set_incoming_key(&remote.0);
        rc4.set_outgoing_key(&local.0);
        rc4
    }

    pub fn set_incoming_key(&mut self, key: &[u8]) {
        self.decrypt = true;
        self.incoming = Rc4::new(key);
        self.incoming.apply(&mut [0u8; 1024]);
    }

    pub fn set_outgoing_key(&mut self, key: &[u8]) {
        self.encrypt = true;
        self.outgoing = Rc4::new(key);
        self.outgoing.apply(&mut [0u8; 1024]);
    }

    /// Encrypts `buf` in place. Returns the bytes processed (libtorrent's "next barrier").
    pub fn encrypt(&mut self, buf: &mut [u8]) -> usize {
        if !self.encrypt {
            return 0;
        }
        self.outgoing.apply(buf);
        buf.len()
    }

    /// Decrypts `buf` in place. Returns the bytes produced.
    pub fn decrypt(&mut self, buf: &mut [u8]) -> usize {
        if !self.decrypt {
            return 0;
        }
        self.incoming.apply(buf);
        buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    // Wikipedia's RC4 test vectors (no keystream discarded).
    #[test]
    fn rc4_known_answers() {
        for (key, plain, cipher) in [
            ("Key", "Plaintext", "BBF316E8D940AF0AD3"),
            ("Wiki", "pedia", "1021BF0420"),
            ("Secret", "Attack at dawn", "45A01F645FC35B383552544B9BF5"),
        ] {
            let mut buf = plain.as_bytes().to_vec();
            Rc4::new(key.as_bytes()).apply(&mut buf);
            assert_eq!(buf, hex(cipher), "{key}");
            Rc4::new(key.as_bytes()).apply(&mut buf);
            assert_eq!(buf, plain.as_bytes());
        }
    }

    /// libtorrent `test_enc_handler`: random buffers go through one handler and back through
    /// the other, in both directions.
    fn test_enc_handler(a: &mut Rc4Handler, b: &mut Rc4Handler) {
        for _ in 0..128 {
            let len = random::random_below(512 * 1024) as usize;
            let mut buf = vec![0u8; len];
            random::crypto_random_bytes(&mut buf);
            let cmp = buf.clone();

            assert_eq!(a.encrypt(&mut buf), len);
            assert!(len == 0 || buf != cmp);
            assert_eq!(b.decrypt(&mut buf), len);
            assert_eq!(buf, cmp);

            assert_eq!(b.encrypt(&mut buf), len);
            assert!(len == 0 || buf != cmp);
            assert_eq!(a.decrypt(&mut buf), len);
            assert_eq!(buf, cmp);
        }
    }

    // libtorrent test_pe_crypto.cpp TORRENT_TEST(rc4)
    #[test]
    fn rc4() {
        let test1_key = Sha1::digest(b"test1_ke");
        let test2_key = Sha1::digest(b"test2_ke");
        let mut rc41 = Rc4Handler::new();
        rc41.set_incoming_key(&test2_key.0);
        rc41.set_outgoing_key(&test1_key.0);
        let mut rc42 = Rc4Handler::new();
        rc42.set_incoming_key(&test1_key.0);
        rc42.set_outgoing_key(&test2_key.0);
        test_enc_handler(&mut rc41, &mut rc42);
    }

    #[test]
    fn rc4_handler_without_keys_leaves_data_alone() {
        let mut h = Rc4Handler::new();
        let mut buf = [1u8, 2, 3];
        assert_eq!(h.encrypt(&mut buf), 0);
        assert_eq!(h.decrypt(&mut buf), 0);
        assert_eq!(buf, [1, 2, 3]);
    }

    // libtorrent test_pe_crypto.cpp TORRENT_TEST(diffie_hellman)
    #[test]
    fn diffie_hellman() {
        for _ in 0..128 {
            let mut dh1 = DhKeyExchange::new();
            let mut dh2 = DhKeyExchange::new();
            let (k1, k2) = (*dh1.local_key(), *dh2.local_key());
            assert!(dh1.compute_secret(&k2));
            assert!(dh2.compute_secret(&k1));
            assert_eq!(dh1.secret(), dh2.secret());
            assert_eq!(dh1.hash_xor_mask(), dh2.hash_xor_mask());
            assert_eq!(*dh1.hash_xor_mask(), req_hash(b"req3", dh1.secret()));
        }
    }

    /// Known answers for the modular exponentiation: `2^0 = 1`, `2^1 = 2`, `2^767` is below the
    /// prime and `2^768 mod p = 2^768 - p` (the prime's top bit is set).
    #[test]
    fn powm_known_answers() {
        let m = montgomery();
        let two = U768([2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(m.powm(&two, &U768::ZERO), U768::ONE);
        assert_eq!(m.powm(&two, &U768::ONE), two);
        let mut e767 = U768::ZERO;
        e767.0[11] = 1 << 63;
        let mut e = U768::ZERO;
        e.0[0] = 767;
        assert_eq!(m.powm(&two, &e), e767);
        e.0[0] = 768;
        let mut r_minus_p = U768::ZERO;
        r_minus_p.sub_assign(&m.p);
        assert_eq!(m.powm(&two, &e), r_minus_p);
        // Fermat: 2^(p-1) = 1 mod p.
        let mut p1 = m.p;
        p1.sub_assign(&U768::ONE);
        assert_eq!(m.powm(&two, &p1), U768::ONE);
        // A public key at or above the prime is reduced first.
        assert_eq!(m.powm(&m.p, &U768::ONE), U768::ZERO);
    }

    #[test]
    fn exported_keys_are_fixed_width_big_endian() {
        let mut secret = [0u8; DH_KEY_LEN];
        secret[DH_KEY_LEN - 1] = 1; // exponent 1: the public key is 2
        let dh = DhKeyExchange::from_secret(&secret);
        let mut want = [0u8; DH_KEY_LEN];
        want[DH_KEY_LEN - 1] = 2;
        assert_eq!(*dh.local_key(), want);
        assert_eq!(U768::from_be_bytes(&DH_PRIME).to_be_bytes(), DH_PRIME);
    }

    /// SEC-10: public keys that pin the shared secret (0, 1, p-1, p and above) are refused.
    #[test]
    fn degenerate_public_keys_are_refused() {
        let mut dh = DhKeyExchange::new();
        let int = |v: u8| {
            let mut k = [0u8; DH_KEY_LEN];
            k[DH_KEY_LEN - 1] = v;
            k
        };
        let p_minus = |d: u64| {
            let mut v = U768::from_be_bytes(&DH_PRIME);
            v.sub_assign(&U768([d, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
            v.to_be_bytes()
        };
        for bad in [int(0), int(1), p_minus(1), p_minus(0), [0xff; DH_KEY_LEN]] {
            assert!(!dh.compute_secret(&bad));
            assert_eq!(*dh.secret(), [0; DH_KEY_LEN], "nothing derived");
        }
        assert!(dh.compute_secret(&int(2)));
        assert!(dh.compute_secret(&p_minus(2)));
    }

    #[test]
    fn derived_keys_pair_up_across_the_connection() {
        let mut dh1 = DhKeyExchange::new();
        let mut dh2 = DhKeyExchange::new();
        let (k1, k2) = (*dh1.local_key(), *dh2.local_key());
        assert!(dh1.compute_secret(&k2));
        assert!(dh2.compute_secret(&k1));
        let ih = Sha1::digest(b"a torrent");
        let mut out = Rc4Handler::for_connection(dh1.secret(), &ih, true);
        let mut inc = Rc4Handler::for_connection(dh2.secret(), &ih, false);
        let mut buf = *b"the standard handshake follows";
        out.encrypt(&mut buf);
        inc.decrypt(&mut buf);
        assert_eq!(&buf, b"the standard handshake follows");
        inc.encrypt(&mut buf);
        out.decrypt(&mut buf);
        assert_eq!(&buf, b"the standard handshake follows");
        assert_eq!(sync_hash(dh1.secret()), sync_hash(dh2.secret()));
        assert_ne!(stream_key_hash(&ih), sync_hash(dh1.secret()));
    }
}
