/// PaTorrent version, from Cargo. Bumped with every change set (D37), so every build is
/// distinguishable; the frontend's `BUILD` adds a `-dev.<commit>` suffix to a build that is not
/// a tagged release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The name with the version. The GUIs' title bars and tray tooltips show the frontend's
/// `NAME_BUILD` instead, which carries the development suffix.
pub const NAME_VERSION: &str = concat!("PaTorrent ", env!("CARGO_PKG_VERSION"));

/// Default HTTP `User-Agent` for trackers and web seeds (libtorrent setting `user_agent`).
pub const USER_AGENT: &str = concat!("PaTorrent/", env!("CARGO_PKG_VERSION"));

/// Azureus-style peer-ID prefix (libtorrent setting `peer_fingerprint`).
///
/// `PA` is PaTorrent's own code (PLAN.md D16). It is free in libtorrent's `identify_client.cpp` and the
/// BEP 20 list; `LR` there is the unrelated Android LibreTorrent.
/// The four characters are a counter, not the version (D37): `-PA1010-` went out with 1.0.1,
/// and the number goes up by one (`-PA1011-`, `-PA1012-`, ...) only with an engine change that
/// alters how the client behaves on the wire. Peers only need to tell engine behaviour apart,
/// and the version moves with every change set, far faster than four characters can follow.
pub const PEER_ID_PREFIX: &str = "-PA1010-";

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
    fn peer_id_prefix_is_a_pa_counter() {
        // D37: `-PA` + a four-digit counter from 1010 on, not the version
        assert!(PEER_ID_PREFIX.starts_with("-PA"));
        let n: u32 = PEER_ID_PREFIX[3..7].parse().expect("four digits");
        assert!(n >= 1010, "{n}");
    }
}
