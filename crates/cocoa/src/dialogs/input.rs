//! The one-line input dialog (New Label, New Profile).
//!
//! Slice F fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

/// Not yet built (slice F).
pub fn ask(title: &str, prompt: &str, initial: &str, done: impl FnOnce(String) + 'static) {
    let _ = title;
    let _ = prompt;
    let _ = initial;
    let _ = done;
    crate::app::not_yet("input dialog");
}
