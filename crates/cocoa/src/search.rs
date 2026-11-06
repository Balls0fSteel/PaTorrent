//! The search field's modes and the Search Results window.
//!
//! Slice I fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

/// Not yet built (slice I).
pub fn set_mode(cmd: crate::commands::Command) {
    let _ = cmd;
    crate::app::not_yet("search mode");
}

/// Whether the search field filters the torrent names (the default mode, checked in its menu);
/// `false` while it searches a source.
/// Not yet built (slice I): always the name filter.
pub fn filters_names() -> bool {
    true
}

/// The search field's text in a source mode: the search runs there.
/// Not yet built (slice I).
pub fn run(text: &str) {
    let _ = text;
    crate::app::not_yet("search a source");
}
