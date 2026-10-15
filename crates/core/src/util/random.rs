//! Non-cryptographic randomness (libtorrent `aux::random`, `aux::random_shuffle`).
//!
//! Seeded from the standard library's per-process random hash keys, which come from the OS RNG,
//! so there are no extra dependencies. NOT for keys, tokens or peer ids: a CSPRNG for those
//! arrives with the network engine (PLAN.md Phase 2).

use std::cell::Cell;
use std::hash::{BuildHasher, Hasher};

thread_local! {
    static STATE: Cell<u64> = Cell::new(seed());
}

fn seed() -> u64 {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(0x9e37_79b9_7f4a_7c15);
    h.finish() | 1
}

/// A random `u64` from a thread-local xorshift64* generator.
pub fn random_u64() -> u64 {
    STATE.with(|s| {
        let mut x = s.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        s.set(x);
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    })
}

/// Uniform in `0..n` (`0` when `n == 0`), using rejection to avoid modulo bias.
pub fn random_below(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let zone = u64::MAX - u64::MAX % n;
    loop {
        let v = random_u64();
        if v < zone {
            return v % n;
        }
    }
}

/// Fisher–Yates shuffle.
pub fn shuffle<T>(v: &mut [T]) {
    for i in (1..v.len()).rev() {
        let j = random_below(i as u64 + 1) as usize;
        v.swap(i, j);
    }
}

/// Fills `buf` from the OS cryptographic RNG (libtorrent `aux::crypto_random_bytes`):
/// `BCryptGenRandom` on Windows, `/dev/urandom` elsewhere. Used for DHT write-token secrets.
/// Falls back to the non-cryptographic generator only if the OS source fails.
pub fn crypto_random_bytes(buf: &mut [u8]) {
    if !os_random(buf) {
        for chunk in buf.chunks_mut(8) {
            let r = random_u64().to_le_bytes();
            chunk.copy_from_slice(&r[..chunk.len()]);
        }
    }
}

#[cfg(windows)]
fn os_random(buf: &mut [u8]) -> bool {
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    // SAFETY: `buf` is a valid writable slice of the given length; a null algorithm handle is
    // allowed together with BCRYPT_USE_SYSTEM_PREFERRED_RNG.
    unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        ) == 0
    }
}

#[cfg(not(windows))]
fn os_random(buf: &mut [u8]) -> bool {
    use std::io::Read;
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(buf)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffle_is_a_permutation() {
        let mut v: Vec<u32> = (0..100).collect();
        shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..100).collect::<Vec<_>>());
        assert!(random_below(10) < 10);
        assert_eq!(random_below(0), 0);
    }
}
