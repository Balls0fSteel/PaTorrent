//! Identity types shared across the engine API.
//!
//! `Sha1Hash` / `Sha256Hash` / `InfoHashes` correspond to libtorrent's `sha1_hash`, `sha256_hash`
//! and `info_hash_t`.
//!
//! Digest arithmetic ported from libtorrent-rasterbar 2.0.11: include/libtorrent/sha1_hash.hpp,
//! src/sha1_hash.cpp, src/ffs.cpp (count_leading_zeros). Copyright (c) 2003-2020, Arvid Norberg
//! and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
//! Changes: stored as big-endian byte arrays instead of network-order `u32` words, so ordering
//! is plain lexicographic comparison and shifts work byte-wise with the same results.

use std::fmt;

/// Identifies a torrent within one running session.
///
/// It is a generational slab index: a handle to a removed torrent never aliases a newly added
/// one. It is not stable across restarts; persist the info-hash instead.
/// `TorrentId::default()` never refers to a live torrent (generations start at 1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct TorrentId(u64);

impl TorrentId {
    pub(crate) const fn new(index: u32, generation: u32) -> TorrentId {
        TorrentId(((generation as u64) << 32) | index as u64)
    }

    /// Slab slot. Only meaningful to the engine.
    #[cfg_attr(not(feature = "mock"), allow(dead_code))]
    pub(crate) const fn index(self) -> u32 {
        self.0 as u32
    }

    #[cfg_attr(not(feature = "mock"), allow(dead_code))]
    pub(crate) const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// Opaque integer form, for frontends that need to key tables by torrent.
    pub const fn to_raw(self) -> u64 {
        self.0
    }

    pub const fn from_raw(raw: u64) -> TorrentId {
        TorrentId(raw)
    }
}

macro_rules! digest {
    ($(#[$meta:meta])* $name:ident, $len:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(pub [u8; $len]);

        impl $name {
            pub const LEN: usize = $len;

            pub fn is_zero(&self) -> bool {
                self.0.iter().all(|&b| b == 0)
            }

            /// Lower-case hex, as libtorrent's `aux::to_hex`.
            pub fn to_hex(&self) -> String {
                crate::util::hex::encode(&self.0)
            }

            pub fn from_hex(s: &str) -> Option<$name> {
                let mut out = [0u8; $len];
                crate::util::hex::decode_into(s, &mut out).then_some($name(out))
            }

            /// The first `LEN` bytes of `s`, if it is long enough.
            pub fn from_slice(s: &[u8]) -> Option<$name> {
                s.get(..$len).map(|b| $name(b.try_into().expect("length checked")))
            }

            /// All bits set (libtorrent `digest32::max()`).
            pub const fn max() -> $name {
                $name([0xff; $len])
            }

            pub fn clear(&mut self) {
                self.0 = [0; $len];
            }

            /// Number of leading zero bits, `8 * LEN` for zero (DHT distance buckets).
            pub fn count_leading_zeroes(&self) -> u32 {
                match self.0.iter().position(|&b| b != 0) {
                    Some(i) => i as u32 * 8 + self.0[i].leading_zeros(),
                    None => $len * 8,
                }
            }

            pub fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }
        }

        impl std::ops::BitXor for $name {
            type Output = $name;
            fn bitxor(mut self, o: $name) -> $name {
                self ^= o;
                self
            }
        }

        impl std::ops::BitXorAssign for $name {
            fn bitxor_assign(&mut self, o: $name) {
                self.0.iter_mut().zip(o.0).for_each(|(a, b)| *a ^= b);
            }
        }

        impl std::ops::BitAnd for $name {
            type Output = $name;
            fn bitand(mut self, o: $name) -> $name {
                self &= o;
                self
            }
        }

        impl std::ops::BitAndAssign for $name {
            fn bitand_assign(&mut self, o: $name) {
                self.0.iter_mut().zip(o.0).for_each(|(a, b)| *a &= b);
            }
        }

        impl std::ops::BitOrAssign for $name {
            fn bitor_assign(&mut self, o: $name) {
                self.0.iter_mut().zip(o.0).for_each(|(a, b)| *a |= b);
            }
        }

        impl std::ops::Not for $name {
            type Output = $name;
            fn not(mut self) -> $name {
                self.0.iter_mut().for_each(|a| *a = !*a);
                self
            }
        }

        /// Shift toward the most significant bit (byte 0).
        impl std::ops::ShlAssign<usize> for $name {
            fn shl_assign(&mut self, n: usize) {
                shift_left(&mut self.0, n);
            }
        }

        impl std::ops::ShrAssign<usize> for $name {
            fn shr_assign(&mut self, n: usize) {
                shift_right(&mut self.0, n);
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.to_hex())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }
    };
}

digest!(
    /// 160-bit SHA-1 digest: v1 info-hashes, piece hashes, DHT node ids.
    Sha1Hash,
    20
);
digest!(
    /// 256-bit SHA-256 digest: v2 info-hashes and merkle roots.
    Sha256Hash,
    32
);

/// Big-endian multi-byte shift by `n` bits toward byte 0 (libtorrent `bits_shift_left`).
fn shift_left(b: &mut [u8], n: usize) {
    let (bytes, bits) = (n / 8, n % 8);
    let len = b.len();
    if bytes >= len {
        b.fill(0);
        return;
    }
    b.copy_within(bytes.., 0);
    b[len - bytes..].fill(0);
    if bits > 0 {
        for i in 0..len {
            let carry = if i + 1 < len { b[i + 1] >> (8 - bits) } else { 0 };
            b[i] = (b[i] << bits) | carry;
        }
    }
}

/// Big-endian multi-byte shift by `n` bits away from byte 0 (libtorrent `bits_shift_right`).
fn shift_right(b: &mut [u8], n: usize) {
    let (bytes, bits) = (n / 8, n % 8);
    let len = b.len();
    if bytes >= len {
        b.fill(0);
        return;
    }
    b.copy_within(..len - bytes, bytes);
    b[..bytes].fill(0);
    if bits > 0 {
        for i in (0..len).rev() {
            let carry = if i > 0 { b[i - 1] << (8 - bits) } else { 0 };
            b[i] = (b[i] >> bits) | carry;
        }
    }
}

/// The v1 and/or v2 info-hash of a torrent (libtorrent `info_hash_t`). Hybrid torrents have both.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct InfoHashes {
    pub v1: Option<Sha1Hash>,
    pub v2: Option<Sha256Hash>,
}

impl InfoHashes {
    pub fn v1(h: Sha1Hash) -> InfoHashes {
        InfoHashes { v1: Some(h), v2: None }
    }

    pub fn has_v1(&self) -> bool {
        self.v1.is_some()
    }

    pub fn has_v2(&self) -> bool {
        self.v2.is_some()
    }

    /// The 20-byte hash used on the wire and in the DHT: v1 if present, otherwise the truncated
    /// v2 hash (libtorrent `info_hash_t::get_best()` semantics for v2-only torrents).
    pub fn best_v1_compatible(&self) -> Sha1Hash {
        match (self.v1, self.v2) {
            (Some(v1), _) => v1,
            (None, Some(v2)) => {
                let mut out = [0u8; 20];
                out.copy_from_slice(&v2.0[..20]);
                Sha1Hash(out)
            }
            (None, None) => Sha1Hash::default(),
        }
    }

    /// Stable display / file-name key: v1 hex if present, otherwise v2 hex.
    pub fn key_hex(&self) -> String {
        match (self.v1, self.v2) {
            (Some(v1), _) => v1.to_hex(),
            (None, Some(v2)) => v2.to_hex(),
            (None, None) => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torrent_id_round_trip() {
        let id = TorrentId::new(7, 3);
        assert_eq!((id.index(), id.generation()), (7, 3));
        assert_eq!(TorrentId::from_raw(id.to_raw()), id);
        assert_ne!(TorrentId::new(7, 4), id);
    }

    #[test]
    fn digest_hex_round_trip() {
        let h = Sha1Hash::from_hex("dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c").unwrap();
        assert_eq!(h.to_hex(), "dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c");
        assert_eq!(h.0[0], 0xdd);
        assert!(Sha1Hash::from_hex("dd82").is_none());
        assert!(Sha1Hash::from_hex("zz8255ecdc7ca55fb0bbf81323d87062db1f6d1c").is_none());
        assert!(Sha1Hash::default().is_zero());
    }

    fn to_hash(s: &str) -> Sha1Hash {
        Sha1Hash::from_hex(s).unwrap()
    }

    /// Ported from libtorrent-rasterbar 2.0.11 test/test_sha1_hash.cpp.
    #[test]
    fn sha1_hash_ops() {
        let mut h1 = Sha1Hash::default();
        let mut h2 = Sha1Hash::default();
        assert!(h1 == h2 && h1 >= h2 && h1.is_zero());
        h1 = to_hash("0123456789012345678901234567890123456789");
        h2 = to_hash("0113456789012345678901234567890123456789");
        assert!(h2 < h1);
        h2.clear();
        assert!(h2.is_zero());

        h2 = to_hash("ffffffffff0000000000ffffffffff0000000000");
        h1 = to_hash("fffff00000fffff00000fffff00000fffff00000");
        h1 &= h2;
        assert_eq!(h1, to_hash("fffff000000000000000fffff000000000000000"));

        h1 = to_hash("fffff00000fffff00000fffff00000fffff00000");
        h1 |= h2;
        assert_eq!(h1, to_hash("fffffffffffffff00000fffffffffffffff00000"));
        h2 = to_hash("0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f");
        h1 ^= h2;
        assert_eq!(h1, to_hash("f0f0f0f0f0f0f0ff0f0ff0f0f0f0f0f0f0ff0f0f"));
        assert_ne!(h1, h2);

        assert_eq!(
            Sha1Hash::from_slice(b"                    ").unwrap(),
            to_hash("2020202020202020202020202020202020202020")
        );
        assert!(Sha1Hash::from_slice(b"short").is_none());

        h1 = to_hash("ffffffffff0000000000ffffffffff0000000000");
        h1 <<= 12;
        assert_eq!(h1, to_hash("fffffff0000000000ffffffffff0000000000000"));
        h1 >>= 12;
        assert_eq!(h1, to_hash("000fffffff0000000000ffffffffff0000000000"));

        h1 = to_hash("7000000000000000000000000000000000000000");
        h1 <<= 1;
        assert_eq!(h1, to_hash("e000000000000000000000000000000000000000"));
        h1 = to_hash("0000000000000000000000000000000000000007");
        h1 <<= 1;
        assert_eq!(h1, to_hash("000000000000000000000000000000000000000e"));
        h1 = to_hash("0000000000000000000000000000000000000007");
        h1 >>= 1;
        assert_eq!(h1, to_hash("0000000000000000000000000000000000000003"));
        h1 = to_hash("7000000000000000000000000000000000000000");
        h1 >>= 1;
        assert_eq!(h1, to_hash("3800000000000000000000000000000000000000"));

        h1 = to_hash("7000000000000000000000000000000000000000");
        h1 >>= 32;
        assert_eq!(h1, to_hash("0000000070000000000000000000000000000000"));
        h1 >>= 33;
        assert_eq!(h1, to_hash("0000000000000000380000000000000000000000"));
        h1 <<= 33;
        assert_eq!(h1, to_hash("0000000070000000000000000000000000000000"));
        h1 <<= 200;
        assert!(h1.is_zero());

        assert_eq!(!Sha1Hash::default(), Sha1Hash::max());
        assert_eq!(h2 ^ h2, Sha1Hash::default());
        assert_eq!(Sha1Hash::max() & h2, h2);
    }

    /// Ported from libtorrent-rasterbar 2.0.11 test/test_sha1_hash.cpp.
    #[test]
    fn count_leading_zeroes() {
        let tests = [
            ("ffffffffffffffffffffffffffffffffffffffff", 0),
            ("0000000000000000000000000000000000000000", 160),
            ("fff0000000000000000000000000000000000000", 0),
            ("7ff0000000000000000000000000000000000000", 1),
            ("3ff0000000000000000000000000000000000000", 2),
            ("1ff0000000000000000000000000000000000000", 3),
            ("0ff0000000000000000000000000000000000000", 4),
            ("07f0000000000000000000000000000000000000", 5),
            ("03f0000000000000000000000000000000000000", 6),
            ("01f0000000000000000000000000000000000000", 7),
            ("00f0000000000000000000000000000000000000", 8),
            ("0070000000000000000000000000000000000000", 9),
            ("0030000000000000000000000000000000000000", 10),
            ("0010000000000000000000000000000000000000", 11),
            ("0000000ffff00000000000000000000000000000", 28),
            ("00000007fff00000000000000000000000000000", 29),
            ("00000003fff00000000000000000000000000000", 30),
            ("00000001fff00000000000000000000000000000", 31),
            ("00000000fff00000000000000000000000000000", 32),
            ("000000007ff00000000000000000000000000000", 33),
            ("000000003ff00000000000000000000000000000", 34),
            ("000000001ff00000000000000000000000000000", 35),
        ];
        for (hex, n) in tests {
            assert_eq!(to_hash(hex).count_leading_zeroes(), n, "{hex}");
        }
        assert_eq!(Sha256Hash::default().count_leading_zeroes(), 256);
    }

    #[test]
    fn best_v1_compatible_truncates_v2() {
        let v2 = Sha256Hash([0xab; 32]);
        let ih = InfoHashes { v1: None, v2: Some(v2) };
        assert_eq!(ih.best_v1_compatible(), Sha1Hash([0xab; 20]));
        assert_eq!(ih.key_hex().len(), 64);
        let v1 = Sha1Hash([1; 20]);
        assert_eq!(InfoHashes { v1: Some(v1), v2: Some(v2) }.best_v1_compatible(), v1);
    }
}
