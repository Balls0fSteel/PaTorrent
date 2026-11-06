//! The helper layer every window is built with (docs/MACOS_GUI.md, slice B): text, controls,
//! stacks and grids (Auto Layout only, so translations and right-to-left fit), alerts, sheets and
//! open panels, and `RPTarget`, the one action target that turns a sender's tag into a
//! [`crate::commands::Command`].
//!
//! - [`text`]: a string-table text as a Mac control shows it (no `&` mnemonic, `…`).
//! - `controls`: [`label`](controls::label), [`button`](controls::button),
//!   [`check`](controls::check), [`popup`](controls::popup), [`field`](controls::field), ...
//! - `layout`: [`vstack`](layout::vstack), [`hstack`](layout::hstack), [`grid`](layout::grid),
//!   [`pin`](layout::pin) and size constraints.
//! - `dialogs`: [`alert`](dialogs::alert), [`sheet`](dialogs::sheet),
//!   [`open_panel`](dialogs::open_panel), [`save_panel`](dialogs::save_panel).
//! - `target`: `RPTarget` and [`target()`](target::target), the shared instance.

#[cfg(target_os = "macos")]
pub mod controls;
#[cfg(target_os = "macos")]
pub mod dialogs;
#[cfg(target_os = "macos")]
pub mod layout;
#[cfg(target_os = "macos")]
pub mod target;

#[cfg(target_os = "macos")]
pub use controls::*;
#[cfg(target_os = "macos")]
pub use dialogs::*;
#[cfg(target_os = "macos")]
pub use target::target;

/// A string-table text as Mac menus and controls show it (convention 3): without the `&`
/// mnemonic (macOS has none; `lang::plain`, which also drops the CJK style's `(&F)`), and with
/// a trailing `...` as the ellipsis character.
pub fn text(s: &str) -> String {
    let plain = rookparcel_frontend::lang::plain(s);
    match plain.strip_suffix("...") {
        Some(head) => format!("{head}\u{2026}"),
        None => plain,
    }
}

/// [`text`] as an `NSString`.
#[cfg(target_os = "macos")]
pub fn ns(s: &str) -> objc2::rc::Retained<objc2_foundation::NSString> {
    objc2_foundation::NSString::from_str(s)
}

/// [`text`] of a string-table text, as an `NSString`.
#[cfg(target_os = "macos")]
pub fn nstext(s: &str) -> objc2::rc::Retained<objc2_foundation::NSString> {
    ns(&text(s))
}

#[cfg(test)]
mod tests {
    use super::text;

    #[test]
    fn mac_texts() {
        assert_eq!(text("&Add Torrent..."), "Add Torrent\u{2026}");
        assert_eq!(text("Add Torrent from &URL..."), "Add Torrent from URL\u{2026}");
        assert_eq!(text("Save && E&xit"), "Save & Exit");
        assert_eq!(text("文件(&F)"), "文件");
        assert_eq!(text("添加 Torrent(&A)..."), "添加 Torrent\u{2026}");
        // only a trailing ellipsis: "..." inside a text is the user's or a placeholder's
        assert_eq!(text("a...b"), "a...b");
        assert_eq!(text("Done"), "Done");
    }
}
