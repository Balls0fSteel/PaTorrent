// Ported from libtorrent-rasterbar 2.0.11: src/bloom_filter.cpp, include/libtorrent/bloom_filter.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a const-generic array instead of a template parameter; same bit layout, so filters
// sent on the wire (DHT scrapes, BEP 33) match libtorrent's byte for byte.

//! A tiny bloom filter keyed by SHA-1 digests (two bit positions from the first four bytes).

use crate::types::Sha1Hash;

/// True if both bits selected by `k` are set in `bits`.
pub fn has_bits(k: &[u8], bits: &[u8]) -> bool {
    let len = bits.len() as u32 * 8;
    let idx1 = (u32::from(k[0]) | (u32::from(k[1]) << 8)) % len;
    let idx2 = (u32::from(k[2]) | (u32::from(k[3]) << 8)) % len;
    bits[(idx1 / 8) as usize] & (1 << (idx1 & 7)) != 0 && bits[(idx2 / 8) as usize] & (1 << (idx2 & 7)) != 0
}

/// Sets the two bits selected by `k`.
pub fn set_bits(k: &[u8], bits: &mut [u8]) {
    let len = bits.len() as u32 * 8;
    let idx1 = (u32::from(k[0]) | (u32::from(k[1]) << 8)) % len;
    let idx2 = (u32::from(k[2]) | (u32::from(k[3]) << 8)) % len;
    bits[(idx1 / 8) as usize] |= 1 << (idx1 & 7);
    bits[(idx2 / 8) as usize] |= 1 << (idx2 & 7);
}

/// Number of bits not set.
pub fn count_zero_bits(bits: &[u8]) -> u32 {
    bits.iter().map(|b| b.count_zeros()).sum()
}

/// libtorrent `bloom_filter<N>`: `N` bytes of bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BloomFilter<const N: usize> {
    bits: [u8; N],
}

impl<const N: usize> Default for BloomFilter<N> {
    fn default() -> Self {
        BloomFilter { bits: [0; N] }
    }
}

impl<const N: usize> BloomFilter<N> {
    pub fn find(&self, k: &Sha1Hash) -> bool {
        has_bits(&k.0, &self.bits)
    }

    pub fn set(&mut self, k: &Sha1Hash) {
        set_bits(&k.0, &mut self.bits);
    }

    pub fn to_bytes(&self) -> [u8; N] {
        self.bits
    }

    pub fn from_bytes(b: &[u8; N]) -> Self {
        BloomFilter { bits: *b }
    }

    pub fn clear(&mut self) {
        self.bits = [0; N];
    }

    /// Estimated number of items in the filter.
    pub fn size(&self) -> f32 {
        let m = (N * 8) as f32;
        let c = (count_zero_bits(&self.bits) as f32).min(m - 1.0);
        (c / m).ln() / (2.0 * (1.0 - 1.0 / m).ln())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Sha1;

    // libtorrent test_bloom_filter.cpp
    #[test]
    fn set_and_get() {
        let mut filter = BloomFilter::<32>::default();
        let k: Vec<Sha1Hash> = (1..=4).map(|i| Sha1::digest(format!("test{i}").as_bytes())).collect();
        assert!(k.iter().all(|k| !filter.find(k)));
        filter.set(&k[0]);
        assert!(filter.find(&k[0]));
        assert!(!filter.find(&k[1]) && !filter.find(&k[2]) && !filter.find(&k[3]));
        filter.set(&k[3]);
        assert!(filter.find(&k[0]) && filter.find(&k[3]));
        assert!(!filter.find(&k[1]) && !filter.find(&k[2]));
    }

    #[test]
    fn set_bits_pattern() {
        let mut bits = [0u8; 4];
        for i in 0..32u8 {
            assert!(!has_bits(&[i, 0, i, 0], &bits[..]));
        }
        for i in (0..32u8).step_by(2) {
            let t = [i, 0, i, 0];
            assert!(!has_bits(&t, &bits));
            set_bits(&t, &mut bits);
            assert!(has_bits(&t, &bits));
        }
        assert_eq!(bits, [0x55; 4]);
    }

    #[test]
    fn count_zeroes() {
        let mut bits = [0x00, 0xff, 0x55, 0xaa];
        assert_eq!(count_zero_bits(&bits), 16);
        set_bits(&[4, 0, 4, 0], &mut bits);
        assert_eq!(count_zero_bits(&bits), 15);
        assert_eq!(bits, [0x10, 0xff, 0x55, 0xaa]);
    }

    #[test]
    fn to_from_bytes() {
        let mut filter = BloomFilter::<4>::from_bytes(&[0x10, 0xff, 0x55, 0xaa]);
        assert_eq!(filter.to_bytes(), [0x10, 0xff, 0x55, 0xaa]);
        let mut k = Sha1Hash([b' '; 20]);
        k.0[..4].copy_from_slice(&[1, 0, 2, 0]);
        assert!(!filter.find(&k));
        filter.set(&k);
        assert!(filter.find(&k));
        assert_eq!(filter.to_bytes(), [0x16, 0xff, 0x55, 0xaa]);
        filter.clear();
        assert_eq!(filter.to_bytes(), [0; 4]);
        assert!(filter.size() < 0.6);
    }
}
