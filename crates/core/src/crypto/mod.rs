//! Hashing and checksums: SHA-1 (v1 pieces, info-hashes, DHT ids), SHA-256 (v2), CRC32C
//! (BEP 42 node ids); the ChaCha20 block function behind the random number generator.
//!
//! Each hash has a portable implementation ported from libtorrent's bundled fallback code and a
//! hardware path (x86-64 SHA-NI, SSE4.2) chosen at runtime. Both paths are tested against the
//! same vectors and against each other.

mod block;
pub mod chacha20;
pub mod crc32c;
pub mod mse;
pub mod sha1;
pub mod sha256;

pub use crc32c::crc32c;
pub use sha1::Sha1;
pub use sha256::Sha256;

/// True if this CPU has the x86 SHA extensions used for SHA-1 and SHA-256.
pub fn sha_ni_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("sha")
            && std::arch::is_x86_feature_detected!("sse4.1")
            && std::arch::is_x86_feature_detected!("ssse3")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}
