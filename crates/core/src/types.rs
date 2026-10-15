//! Identity types shared across the engine API.
//!
//! `Sha1Hash` / `Sha256Hash` / `InfoHashes` correspond to libtorrent's `sha1_hash`, `sha256_hash`
//! and `info_hash_t`. The full `digest32` arithmetic (xor, shifts, leading-zero count for DHT
//! distances) is ported with `sha1_hash.cpp` in Phase 1.

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
