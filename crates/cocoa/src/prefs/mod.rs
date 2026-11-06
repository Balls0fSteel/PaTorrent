//! Preferences: one window, instant apply.
//!
//! Slice E fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

pub mod advanced;
pub mod controls;
pub mod sched_grid;

/// Not yet built (slice E).
pub fn open() {
    crate::app::not_yet("Preferences");
}

/// Not yet built (slice E).
pub fn open_at(key: &str) {
    let _ = key;
    crate::app::not_yet("Preferences at a page");
}
