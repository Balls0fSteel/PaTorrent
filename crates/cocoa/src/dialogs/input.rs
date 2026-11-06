//! The one-line input dialog (New Label, New Profile; Add Torrent from URL is one too): an alert
//! with a text field, OK and Cancel, as a sheet on the main window (application-modal while the
//! window is hidden).

use std::cell::Cell;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlertFirstButtonReturn, NSApplication, NSModalResponse, NSTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use rookparcel_frontend::tr;

use crate::ui::{self, ns};

/// Asks for one line: `title` in bold, `prompt` under it, the field starting with `initial`;
/// `done` gets the trimmed text on OK (nothing on Cancel).
pub fn ask(title: &str, prompt: &str, initial: &str, done: impl FnOnce(String) + 'static) {
    line(title, prompt, initial, "", done);
}

/// [`ask`] with a grey hint in the empty field.
pub(super) fn line(
    title: &str,
    prompt: &str,
    initial: &str,
    placeholder: &str,
    done: impl FnOnce(String) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let alert = ui::make_alert(
        ui::Style::Info,
        &ui::text(title),
        &ui::text(prompt),
        &[tr!(BtnOk), tr!(BtnCancel)],
        mtm,
    );
    // an alert's accessory view is laid out by its frame
    let field = NSTextField::textFieldWithString(&ns(initial), mtm);
    field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(340.0, 24.0)));
    if !placeholder.is_empty() {
        field.setPlaceholderString(Some(&ns(placeholder)));
    }
    alert.setAccessoryView(Some(&field));
    // Escape is Cancel's in every language (AppKit gives it only to a button titled "Cancel")
    if let Some(cancel) = alert.buttons().iter().nth(1) {
        cancel.setKeyEquivalent(&ns("\u{1b}"));
    }
    alert.layout();
    alert.window().setInitialFirstResponder(Some(&field));
    let main = super::main_window();
    // the dialog's number, for `ROOKPARCEL_TEST_ACCEPT` (OK as if clicked)
    let id = super::new_id();
    let (sheet, parent) = (alert.window(), main.clone());
    super::set_handler(id, move |act| {
        if act != ACCEPT {
            return;
        }
        match (&parent, MainThreadMarker::new()) {
            (Some(p), _) => p.endSheet_returnCode(&sheet, NSAlertFirstButtonReturn),
            (None, Some(mtm)) => {
                NSApplication::sharedApplication(mtm).stopModalWithCode(NSAlertFirstButtonReturn)
            }
            (None, None) => {}
        }
    });
    super::test_accept(id, ACCEPT);
    let answer = move |r: NSModalResponse, text: String| {
        super::retire(id);
        if r == NSAlertFirstButtonReturn {
            done(text.trim().to_owned());
        }
    };
    match main {
        Some(window) => {
            let (f, answer) = (field.clone(), Cell::new(Some(answer)));
            let handler = RcBlock::new(move |r: NSModalResponse| {
                if let Some(a) = answer.take() {
                    a(r, ui::value(&f));
                }
            });
            alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
            alert.window().makeFirstResponder(Some(&field));
        }
        None => {
            let r = alert.runModal();
            answer(r, ui::value(&field));
        }
    }
}

/// The OK of [`super::test_accept`].
const ACCEPT: u8 = 1;
