//! Add Torrent from URL: one line for a `magnet:` link or the `http(s)://` URL of a `.torrent`,
//! starting with the clipboard's text when it is such a link. OK hands it to `Core::add_url`: a
//! magnet joins the session (its dialog comes with `Events::magnet_ready`), a URL is fetched on a
//! worker thread (its dialog comes with `Events::adds`), anything else is an error.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use rookparcel_frontend::tr;

use crate::app::with_core;

/// File > Add Torrent from URL.
pub fn open() {
    let initial = clipboard_link().unwrap_or_default();
    super::input::line(tr!(AddUrlTitle), tr!(AddUrlPrompt), &initial, tr!(UrlPlaceholder), |text| {
        if let Some(step) = with_core(|c| c.add_url(&text)) {
            super::add::handle(step);
        }
    });
}

/// The clipboard's text when it is a link Rookparcel can add.
fn clipboard_link() -> Option<String> {
    // SAFETY: the string type's constant.
    let text = NSPasteboard::generalPasteboard().stringForType(unsafe { NSPasteboardTypeString })?;
    rookparcel_frontend::add::offered_link(&text.to_string())
}
