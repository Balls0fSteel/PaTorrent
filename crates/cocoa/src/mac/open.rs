//! `application:openURLs:`, files dropped, a second start forwarding its arguments.
//!
//! Slice G fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

/// Not yet built (slice G).
pub fn open(urls: Vec<String>) {
    let _ = urls;
    crate::app::not_yet("open files and links");
}
