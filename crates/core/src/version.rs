/// Rookparcel version, from Cargo. Bumped with every change set (D37), so every build is
/// distinguishable; the frontend's `BUILD` adds a `-dev.<commit>` suffix to a build that is not
/// a tagged release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The name with the version. The GUIs' title bars and tray tooltips show the frontend's
/// `NAME_BUILD` instead, which carries the development suffix.
pub const NAME_VERSION: &str = concat!("Rookparcel ", env!("CARGO_PKG_VERSION"));

/// Default HTTP `User-Agent` for trackers and web seeds (libtorrent setting `user_agent`).
pub const USER_AGENT: &str = concat!("Rookparcel/", env!("CARGO_PKG_VERSION"));

/// Azureus-style peer-ID prefix (libtorrent setting `peer_fingerprint`).
///
/// `RP` is Rookparcel's own code (D59). It is free in libtorrent's `identify_client.cpp` and the
/// BEP 20 list; `LR` there is the unrelated Android LibreTorrent. Under its former name PaTorrent
/// (D16) the client sent `PA`, which the client table still names.
/// The four characters are a counter, not the version (D37): `-PA1010-` went out with 1.0.1,
/// `-PA1011-` with 1.1.0 (the security fixes' wire changes), `-PA1012-` with 1.1.18 (libtorrent
/// 2.0.13's v2 request sizes and the other upstream fixes; still unreleased, it also covers
/// 1.1.20's v2 hash-request fixes), `-PA1013-` with 1.2.0 (the `reqq` fix of 1.1.66, the bug
/// hunt's engine fixes in 1.1.70 and 1.1.74, upload mode's cancelled requests in 1.1.76),
/// `-RP1014-` with the rename to Rookparcel (the client code itself changed), and
/// the number goes up by one only
/// with an engine change that alters how the client behaves on the wire. Peers only need to
/// tell engine behaviour apart, and the version moves with every change set, far faster than
/// four characters can follow.
pub const PEER_ID_PREFIX: &str = "-RP1014-";

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
    fn peer_id_prefix_is_an_rp_counter() {
        // D37, D59: `-RP` + a four-digit counter (from 1014 on; 1010-1013 were `-PA`), not the version
        assert!(PEER_ID_PREFIX.starts_with("-RP"));
        let n: u32 = PEER_ID_PREFIX[3..7].parse().expect("four digits");
        assert!(n >= 1010, "{n}");
    }
}
