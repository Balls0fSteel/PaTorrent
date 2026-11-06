//! The detail tabs (General, Trackers, Peers, Pieces, Files, Speed, Logger).
//!
//! Slice D fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

pub mod files;
pub mod general;
pub mod pieces;
pub mod speed;
pub mod tables;

/// Not yet built (slice D).
pub fn show_tab(tab: rookparcel_frontend::detail::Tab) {
    let _ = tab;
    crate::app::not_yet("detail tab");
}

/// Redraws the shown tab from `Core::detail` (after `Events::detail`, a selection change or a
/// tab change).
/// Not yet built (slice D).
pub fn refresh() {}

/// Edit > Find while the Files tab has the focus: its filter takes it (`true`); otherwise the
/// toolbar search does (`false`).
/// Not yet built (slice D).
pub fn find() -> bool {
    false
}
