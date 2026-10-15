/// PaTorrent version, from Cargo.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default HTTP `User-Agent` for trackers and web seeds (libtorrent setting `user_agent`).
pub const USER_AGENT: &str = concat!("PaTorrent/", env!("CARGO_PKG_VERSION"));

/// Azureus-style peer-ID prefix (libtorrent setting `peer_fingerprint`).
///
/// `PA` is PaTorrent's own code (PLAN.md D16). It is free in libtorrent's `identify_client.cpp` and the
/// BEP 20 list; `LR` there is the unrelated Android LibreTorrent.
/// The four version characters must be kept in sync with the crate version.
pub const PEER_ID_PREFIX: &str = "-PA0100-";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_id_prefix_is_azureus_style() {
        let b = PEER_ID_PREFIX.as_bytes();
        assert_eq!(b.len(), 8);
        assert_eq!((b[0], b[7]), (b'-', b'-'));
        assert!(b[1..3].iter().all(u8::is_ascii_alphabetic));
        assert!(b[3..7].iter().all(u8::is_ascii_alphanumeric));
    }

    #[test]
    fn peer_id_prefix_tracks_version() {
        let mut parts = VERSION.split('.').map(|p| p.parse::<u32>().unwrap());
        let (major, minor, tiny) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
        assert_eq!(&PEER_ID_PREFIX[3..6], format!("{major}{minor}{tiny}"));
    }
}
