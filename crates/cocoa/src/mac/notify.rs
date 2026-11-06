//! Completed-download notifications.
//!
//! Slice G fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

/// Not yet built (slice G).
pub fn completed(names: &[String]) {
    let _ = names;
    crate::app::not_yet("notification");
}
