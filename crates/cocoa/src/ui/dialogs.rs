//! Alerts, sheets and file panels.
//!
//! - [`alert`]: an application-modal alert, answered at once (the index of the button).
//! - [`sheet`]: the same as a sheet on a window; the answer comes to a callback later.
//! - [`open_panel`] / [`save_panel`]: the system's panels as sheets (or alone without a
//!   window); the chosen paths come to a callback.
//!
//! Callbacks run from AppKit's completion handlers on the main thread, outside any borrow of
//! the application's state (re-entrancy rule).

use std::cell::Cell;
use std::path::PathBuf;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSModalResponse, NSModalResponseOK, NSOpenPanel,
    NSSavePanel, NSWindow,
};
use objc2_foundation::{NSArray, NSString, NSURL};
use objc2_uniform_type_identifiers::UTType;

use super::{ns, text};

/// How an alert looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Info,
    Warning,
    Error,
}

/// An alert of `message` (bold) and `info` (under it) with `buttons` (string-table texts, the
/// first the default; none: OK).
pub fn make_alert(
    style: Style,
    message: &str,
    info: &str,
    buttons: &[&str],
    mtm: MainThreadMarker,
) -> Retained<NSAlert> {
    let a = NSAlert::new(mtm);
    a.setAlertStyle(match style {
        Style::Info => NSAlertStyle::Informational,
        Style::Warning => NSAlertStyle::Warning,
        Style::Error => NSAlertStyle::Critical,
    });
    a.setMessageText(&ns(message));
    a.setInformativeText(&ns(info));
    for b in buttons {
        a.addButtonWithTitle(&ns(&text(b)));
    }
    a
}

/// The index of the button an alert's response stands for.
/// A sheet ended from outside (`endSheet:returnCode:` with `NSModalResponseAbort`, as Quit does
/// to the sheets that would hold it up) answers no button: `usize::MAX`.
fn button_of(r: NSModalResponse) -> usize {
    usize::try_from(r - NSAlertFirstButtonReturn).unwrap_or(usize::MAX)
}

/// Shows an application-modal alert and returns the index of the button pressed.
pub fn alert(style: Style, message: &str, info: &str, buttons: &[&str], mtm: MainThreadMarker) -> usize {
    button_of(make_alert(style, message, info, buttons, mtm).runModal())
}

/// An alert as a sheet on `window` (application-modal without one); `done` gets the button's
/// index.
pub fn sheet(
    window: Option<&NSWindow>,
    style: Style,
    message: &str,
    info: &str,
    buttons: &[&str],
    done: impl FnOnce(usize) + 'static,
    mtm: MainThreadMarker,
) {
    let a = make_alert(style, message, info, buttons, mtm);
    let Some(window) = window else {
        done(button_of(a.runModal()));
        return;
    };
    let done = Cell::new(Some(done));
    let handler = RcBlock::new(move |r: NSModalResponse| {
        if let Some(f) = done.take() {
            f(button_of(r));
        }
    });
    a.beginSheetModalForWindow_completionHandler(window, Some(&handler));
}

/// What an open panel picks.
#[derive(Clone, Debug, Default)]
pub struct Pick {
    pub files: bool,
    pub folders: bool,
    pub multiple: bool,
    /// File name extensions the panel offers (`torrent`); empty: any file.
    pub extensions: Vec<&'static str>,
    /// The folder it opens in.
    pub start: Option<PathBuf>,
    /// The button's text (a string-table text); empty: AppKit's Open.
    pub prompt: String,
    /// A line above the files.
    pub message: String,
}

fn url_path(u: &NSURL) -> Option<PathBuf> {
    u.path().map(|p| PathBuf::from(p.to_string()))
}

fn dir_url(p: &std::path::Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&p.to_string_lossy()), true)
}

fn run_panel(panel: &NSSavePanel, window: Option<&NSWindow>, done: impl FnOnce(bool) + 'static) {
    let done = Cell::new(Some(done));
    let handler = RcBlock::new(move |r: NSModalResponse| {
        if let Some(f) = done.take() {
            f(r == NSModalResponseOK);
        }
    });
    match window {
        Some(w) => panel.beginSheetModalForWindow_completionHandler(w, &handler),
        None => panel.beginWithCompletionHandler(&handler),
    }
}

/// The system's open panel; `done` gets the chosen paths (none when cancelled).
pub fn open_panel(
    window: Option<&NSWindow>,
    pick: Pick,
    done: impl FnOnce(Vec<PathBuf>) + 'static,
    mtm: MainThreadMarker,
) {
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(pick.files);
    panel.setCanChooseDirectories(pick.folders);
    panel.setAllowsMultipleSelection(pick.multiple);
    panel.setCanCreateDirectories(pick.folders);
    let types: Vec<Retained<UTType>> = pick
        .extensions
        .iter()
        .filter_map(|e| UTType::typeWithFilenameExtension(&NSString::from_str(e)))
        .collect();
    if !types.is_empty() {
        let refs: Vec<&UTType> = types.iter().map(|t| &**t).collect();
        panel.setAllowedContentTypes(&NSArray::from_slice(&refs));
    }
    if let Some(s) = &pick.start {
        panel.setDirectoryURL(Some(&dir_url(s)));
    }
    if !pick.prompt.is_empty() {
        panel.setPrompt(Some(&ns(&text(&pick.prompt))));
    }
    if !pick.message.is_empty() {
        panel.setMessage(Some(&ns(&pick.message)));
    }
    let p = panel.clone();
    run_panel(&panel, window, move |ok| {
        let paths = if ok { p.URLs().iter().filter_map(|u| url_path(&u)).collect() } else { Vec::new() };
        done(paths);
    });
}

/// The system's save panel, suggesting `name` in `start`; `done` gets the path (none when
/// cancelled).
pub fn save_panel(
    window: Option<&NSWindow>,
    name: &str,
    start: Option<PathBuf>,
    done: impl FnOnce(Option<PathBuf>) + 'static,
    mtm: MainThreadMarker,
) {
    let panel = NSSavePanel::savePanel(mtm);
    panel.setNameFieldStringValue(&ns(name));
    panel.setCanCreateDirectories(true);
    if let Some(s) = &start {
        panel.setDirectoryURL(Some(&dir_url(s)));
    }
    let p = panel.clone();
    run_panel(&panel, window, move |ok| done(if ok { p.URL().and_then(|u| url_path(&u)) } else { None }));
}
