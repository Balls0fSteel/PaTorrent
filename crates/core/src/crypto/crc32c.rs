// Ported from libtorrent-rasterbar 2.0.11: src/crc32c.cpp, include/libtorrent/aux_/crc32c.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: one byte-slice entry point (libtorrent has 32-bit, 64-bit-word and buffer variants);
// the software fallback is a table instead of boost::crc_optimal.

//! CRC32C (Castagnoli), used for BEP 42 DHT node-id verification.

const POLY_REFLECTED: u32 = 0x82F6_3B78; // 0x1EDC6F41 bit-reversed

const TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ POLY_REFLECTED } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// CRC32C of `data` (initial value and final xor 0xFFFFFFFF, reflected).
pub fn crc32c(data: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("sse4.2") {
        // SAFETY: SSE4.2 was detected at runtime.
        return unsafe { crc32c_sse42(data) };
    }
    crc32c_portable(data)
}

/// Incremental CRC32C, for hashing data that is produced piecewise (libtorrent feeds
/// `boost::crc_optimal` byte by byte when hashing file paths).
#[derive(Clone, Copy, Debug)]
pub struct Crc32c(u32);

impl Default for Crc32c {
    fn default() -> Crc32c {
        Crc32c::new()
    }
}

impl Crc32c {
    pub const fn new() -> Crc32c {
        Crc32c(0xFFFF_FFFF)
    }

    pub fn update_byte(&mut self, b: u8) {
        self.0 = TABLE[((self.0 ^ b as u32) & 0xff) as usize] ^ (self.0 >> 8);
    }

    pub fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.update_byte(b);
        }
    }

    pub fn checksum(&self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

fn crc32c_portable(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = TABLE[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
unsafe fn crc32c_sse42(data: &[u8]) -> u32 {
    use std::arch::x86_64::{_mm_crc32_u8, _mm_crc32_u64};
    // The intrinsics are safe to call here from Rust 1.87 on; the block keeps the 1.85 MSRV
    // building. SAFETY: SSE4.2 is enabled for this function.
    #[allow(unused_unsafe)]
    unsafe {
        let mut crc = 0xFFFF_FFFFu64;
        let mut words = data.chunks_exact(8);
        for w in &mut words {
            crc = _mm_crc32_u64(crc, u64::from_le_bytes(w.try_into().expect("8-byte chunk")));
        }
        let mut crc = crc as u32;
        for &b in words.remainder() {
            crc = _mm_crc32_u8(crc, b);
        }
        crc ^ 0xFFFF_FFFF
    }
}

#[cfg(test)]
mod tests {
    //! Ported from libtorrent-rasterbar 2.0.11 test/test_crc32.cpp (RFC 3720 B.4 vectors).
    use super::*;

    #[test]
    fn rfc3720_vectors() {
        // crc32c_32(host_to_network(0xeffea55a)) hashes the big-endian bytes
        assert_eq!(crc32c(&0xeffe_a55au32.to_be_bytes()), 0x5ee3_b9d5);
        let ascending: Vec<u8> = (0..32).collect();
        for (input, expected) in
            [(&[0u8; 32][..], 0x8a91_36aa), (&[0xffu8; 32][..], 0x62a8_ab43), (&ascending[..], 0x46dd_794e)]
        {
            assert_eq!(crc32c(input), expected);
            assert_eq!(crc32c_portable(input), expected);
        }
    }

    #[test]
    fn hardware_matches_portable_on_odd_lengths() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        for len in [0, 1, 7, 8, 9, 15, 16, 17, 999, 1000] {
            assert_eq!(crc32c(&data[..len]), crc32c_portable(&data[..len]), "len {len}");
        }
    }
}
