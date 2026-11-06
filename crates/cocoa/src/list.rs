//! The torrent list over `Core`: columns kept in prefs, state icons, double-click and keys.
//!
//! Slice C fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

/// Not yet built (slice C).
pub fn activate(id: u64) {
    let _ = id;
    crate::app::not_yet("list double-click");
}

/// Not yet built (slice C).
pub fn key(key: crate::table::Key, ids: &[u64]) -> bool {
    let _ = key;
    let _ = ids;
    crate::app::not_yet("list key");
    false
}
