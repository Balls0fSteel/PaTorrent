//! Random numbers (libtorrent `aux::random`, `aux::random_shuffle`, `aux::crypto_random_bytes`).
//!
//! All of it is cryptographic (SEC-02): these values go on the wire as MSE padding, peer ids,
//! tracker keys and DHT, UDP tracker and uTP ids, so one must not tell anything about the
//! others. Each thread has a ChaCha20 generator keyed from the OS. Every refill makes a few
//! blocks and takes the first 32 bytes as the next key (fast key erasure), and handed-out bytes
//! are wiped, so neither earlier nor later output follows from what was sent or from the state.
//!
//! [`crypto_random_bytes`] reads the OS directly and stops the process when it cannot (SEC-01):
//! a predictable stand-in would weaken every key and token without a sign.

use std::cell::RefCell;

use crate::crypto::chacha20;

const REFILL_BLOCKS: usize = 4;
/// The output of one refill, after the next key.
const BUF: usize = 64 * REFILL_BLOCKS - 32;

struct Rng {
    key: [u8; 32],
    buf: [u8; BUF],
    pos: usize,
}

impl Rng {
    fn new() -> Rng {
        let mut key = [0u8; 32];
        crypto_random_bytes(&mut key);
        Rng { key, buf: [0; BUF], pos: BUF }
    }

    fn refill(&mut self) {
        let mut out = [0u8; 64 * REFILL_BLOCKS];
        for (i, block) in out.chunks_exact_mut(64).enumerate() {
            chacha20::block(&self.key, i as u32, &[0; 12], block.try_into().expect("64 bytes"));
        }
        self.key.copy_from_slice(&out[..32]);
        self.buf.copy_from_slice(&out[32..]);
        self.pos = 0;
    }

    fn fill(&mut self, dst: &mut [u8]) {
        let mut done = 0;
        while done < dst.len() {
            if self.pos == BUF {
                self.refill();
            }
            let n = (BUF - self.pos).min(dst.len() - done);
            let src = &mut self.buf[self.pos..self.pos + n];
            dst[done..done + n].copy_from_slice(src);
            src.fill(0);
            self.pos += n;
            done += n;
        }
    }
}

thread_local! {
    static RNG: RefCell<Rng> = RefCell::new(Rng::new());
}

/// Fills `buf` from this thread's generator.
pub fn random_bytes(buf: &mut [u8]) {
    RNG.with(|r| r.borrow_mut().fill(buf));
}

/// A random `u64`.
pub fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    random_bytes(&mut b);
    u64::from_le_bytes(b)
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

type Source = fn(&mut [u8]) -> bool;

#[cfg(windows)]
const OS_SOURCES: &[Source] = &[bcrypt];
#[cfg(not(windows))]
const OS_SOURCES: &[Source] = &[entropy, urandom];

/// Fills `buf` from the OS cryptographic RNG: `BCryptGenRandom` on Windows; `getentropy`
/// (no file descriptor needed), else `/dev/urandom`, elsewhere. Panics, which ends the process
/// in release builds, when none works.
pub fn crypto_random_bytes(buf: &mut [u8]) {
    fill_from(buf, OS_SOURCES);
}

fn fill_from(buf: &mut [u8], sources: &[Source]) {
    if !sources.iter().any(|source| source(buf)) {
        panic!("no OS random number source works; refusing to continue with predictable keys");
    }
}

#[cfg(windows)]
fn bcrypt(buf: &mut [u8]) -> bool {
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

/// `getentropy(3)`: glibc 2.25+, musl, macOS 10.12+, the BSDs. At most 256 bytes per call.
#[cfg(not(windows))]
fn entropy(buf: &mut [u8]) -> bool {
    unsafe extern "C" {
        fn getentropy(buf: *mut std::ffi::c_void, len: usize) -> std::ffi::c_int;
    }
    buf.chunks_mut(256).all(|chunk| {
        // SAFETY: `chunk` is a valid writable slice of at most 256 bytes.
        unsafe { getentropy(chunk.as_mut_ptr().cast(), chunk.len()) == 0 }
    })
}

#[cfg(not(windows))]
fn urandom(buf: &mut [u8]) -> bool {
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

    /// SEC-02: the old generator returned `state * M` for an odd `M`, so one output read from an
    /// MSE pad gave the state and with it every later id. That prediction must now fail.
    #[test]
    fn one_output_does_not_predict_the_next() {
        const M: u64 = 0x2545_f491_4f6c_dd1d;
        let mut inverse = M;
        for _ in 0..5 {
            inverse = inverse.wrapping_mul(2u64.wrapping_sub(M.wrapping_mul(inverse)));
        }
        assert_eq!(M.wrapping_mul(inverse), 1);
        let predict = |out: u64| {
            let mut x = out.wrapping_mul(inverse);
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            x.wrapping_mul(M)
        };
        let mut hits = 0;
        for _ in 0..1000 {
            let seen = random_u64();
            if predict(seen) == random_u64() {
                hits += 1;
            }
        }
        assert_eq!(hits, 0);
    }

    #[test]
    fn output_spans_refills_and_threads() {
        let mut a = vec![0u8; 3 * BUF + 7];
        random_bytes(&mut a);
        // no block repeats across refills
        let chunks: std::collections::HashSet<&[u8]> = a.chunks(32).collect();
        assert_eq!(chunks.len(), a.len().div_ceil(32));
        let other = std::thread::spawn(random_u64).join().unwrap();
        assert_ne!(other, random_u64());
    }

    #[test]
    fn crypto_random_bytes_fills_long_buffers() {
        let mut b = [0u8; 1000];
        crypto_random_bytes(&mut b);
        assert!(b[..256].iter().any(|&x| x != 0) && b[744..].iter().any(|&x| x != 0));
        #[cfg(not(windows))]
        {
            let mut c = [0u8; 300];
            assert!(entropy(&mut c) && c.iter().any(|&x| x != 0));
        }
    }

    /// SEC-01: when no OS source works there is no weak fallback.
    #[test]
    #[should_panic(expected = "no OS random number source")]
    fn no_os_source_is_fatal() {
        fill_from(&mut [0u8; 32], &[|_| false, |_| false]);
    }
}
