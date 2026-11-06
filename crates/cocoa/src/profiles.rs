//! File > Profiles (D42) and a named profile's first start (GTK `fill_profiles`,
//! `start_profile`, `new_profile`, `profile_folder`, `create_profile`):
//!
//! - The menu lists the default profile and the named ones with this process's checked
//!   (`menu::profiles`, `app::state`); choosing another starts it as a process of its own: this
//!   executable with `--profile NAME` (`profile::args_for`), a new process each time, as GTK
//!   does. `open -a` is not used: it would hand the start to the copy already running. A copy of
//!   that profile that runs already takes the start over (the second-start forward, `mac::open`)
//!   and comes to the front.
//! - New Profile asks a name (`dialogs::input::ask`, `profile::validate`), makes its folders and
//!   starts it. Open Profile Folder shows this profile's settings folder in Finder. Create
//!   Shortcut is not offered on the Mac.
//! - A named profile without folders asks "Create it?" before anything else ([`ensure_exists`]);
//!   no ends the application (a mistyped name should not start an empty client).
//!
//! [`confirm`] is the OK/Cancel question this slice's flows share (import, the remote
//! connection's questions, "Create it?"); `ROOKPARCEL_TEST_ACCEPT` answers OK, as for the
//! torrent dialogs (`dialogs`). Another test aid: New Profile's field starts with
//! `ROOKPARCEL_TEST_INPUT` (nothing can be typed while the VM's screen is locked).

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use block2::RcBlock;
use objc2::{MainThreadMarker, Message};
use objc2_app_kit::{NSAlertFirstButtonReturn, NSApplication, NSModalResponse, NSWindow, NSWorkspace};
use objc2_foundation::{NSString, NSURL};
use rookparcel_frontend::app::{Dirs, Os, dirs_for};
use rookparcel_frontend::{profile, tr};

use crate::dialogs;
use crate::paths;
use crate::ui::{self, ns};

/// The OK of [`confirm`]'s test answer.
const ACCEPT: u8 = 1;

/// An OK/Cancel question (OK first and the default, Escape cancels): a sheet on `window`, or
/// application-modal without one (then `done` runs before this returns). `done` gets whether OK
/// was pressed; a sheet ended from outside (Quit) answers no.
pub(crate) fn confirm(
    window: Option<&NSWindow>,
    style: ui::Style,
    message: &str,
    detail: &str,
    done: impl FnOnce(bool) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let alert = ui::make_alert(style, message, detail, &[tr!(BtnOk), tr!(BtnCancel)], mtm);
    // Escape is Cancel's in every language (AppKit gives it only to a button titled "Cancel")
    if let Some(cancel) = alert.buttons().iter().nth(1) {
        cancel.setKeyEquivalent(&ns("\u{1b}"));
    }
    let id = dialogs::new_id();
    let (sheet, parent) = (alert.window(), window.map(|w| w.retain()));
    dialogs::set_handler(id, move |act| {
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
    dialogs::test_accept(id, ACCEPT);
    let answer = move |r: NSModalResponse| {
        dialogs::retire(id);
        done(r == NSAlertFirstButtonReturn);
    };
    match window {
        Some(w) => {
            let answer = Cell::new(Some(answer));
            let handler = RcBlock::new(move |r: NSModalResponse| {
                if let Some(a) = answer.take() {
                    a(r);
                }
            });
            alert.beginSheetModalForWindow_completionHandler(w, Some(&handler));
        }
        None => answer(alert.runModal()),
    }
}

/// The folders of profile `name` (`None`: the default one), in this process's portable folder
/// if it runs portable.
fn dirs_of(name: Option<&str>) -> Dirs {
    let portable = profile::current().portable_root.clone();
    dirs_for(paths::GUI, Os::current(), name, portable.as_deref(), |k| std::env::var_os(k))
}

/// Makes a profile's settings and library folders.
fn create(d: &Dirs) -> std::io::Result<()> {
    [&d.config, &d.data].iter().try_for_each(|p| rookparcel_core::util::private_file::create_dir_all(p))
}

/// Whether this process's profile has its folders: at once for the default profile and an
/// existing named one; after a yes for a named one that has none yet, made then. Asked
/// application-modal before any window (GTK `create_profile`).
pub fn ensure_exists() -> bool {
    let Some(name) = profile::current().name.clone() else { return true };
    let d = paths::dirs();
    if d.config.exists() || d.data.exists() {
        return true;
    }
    let text = tr!(ProfileCreate, name, folder = d.config.display());
    let (message, detail) = text.split_once("\n\n").unwrap_or((&text, ""));
    let yes = Rc::new(Cell::new(false));
    let y = yes.clone();
    confirm(None, ui::Style::Info, message, detail, move |ok| y.set(ok));
    if !yes.get() {
        return false;
    }
    match create(&d) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("cannot create {}: {e}", d.config.display());
            if let Some(mtm) = MainThreadMarker::new() {
                ui::alert(ui::Style::Error, &tr!(ErrStartRookparcel, error = e), "", &[], mtm);
            }
            false
        }
    }
}

/// The program to start again: this executable (inside the bundle, so the new process is an
/// application of its own with its own Dock icon).
fn program() -> std::io::Result<PathBuf> {
    std::env::current_exe()
}

/// Starts this executable again with `args`, as a process of its own (reaped when it ends). A
/// test aid: with `ROOKPARCEL_SNAPSHOTS` set, the new process's pictures go to its subfolder
/// `subfolder`, so they do not overwrite this one's.
pub(crate) fn launch(args: &[String], subfolder: &str) -> std::io::Result<()> {
    let mut c = std::process::Command::new(program()?);
    c.args(args);
    if let Some(dir) = std::env::var_os("ROOKPARCEL_SNAPSHOTS").filter(|d| !d.is_empty()) {
        c.env("ROOKPARCEL_SNAPSHOTS", PathBuf::from(dir).join(subfolder));
    }
    let mut child = c.spawn()?;
    eprintln!("started process {} with {args:?}", child.id());
    // reaped when it ends, so it leaves no zombie behind
    drop(std::thread::spawn(move || child.wait()));
    Ok(())
}

/// Tells the user why another Rookparcel did not start.
fn cannot_start(e: std::io::Error) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    dialogs::tell(None, ui::Style::Error, &tr!(ErrStartRookparcel, error = e), "", mtm);
}

/// Starts profile `name` (`None`: the default one) in a process of its own.
fn start_profile(name: Option<&str>) {
    let folder = format!("profile-{}", name.unwrap_or(profile::DEFAULT_NAME));
    if let Err(e) = launch(&profile::args_for(name), &folder) {
        cannot_start(e);
    }
}

/// File > Profiles > the profile at `index` of `profile::menu`: this one comes to the front,
/// another starts.
pub fn start(index: usize) {
    let list = profile::menu(&paths::profiles_root());
    let Some((name, me)) = list.get(index) else { return };
    if *me {
        crate::mac::lifecycle::show_window();
    } else {
        start_profile(name.as_deref());
    }
}

/// File > Profiles > New Profile: asks a name, makes its folders and starts it.
pub fn new_profile() {
    let initial = std::env::var("ROOKPARCEL_TEST_INPUT").unwrap_or_default();
    dialogs::input::ask(tr!(ProfileNewTitle), tr!(ProfileNewPrompt), &initial, |text| {
        if text.is_empty() {
            return;
        }
        match profile::validate(&text) {
            Err(e) => {
                if let Some(mtm) = MainThreadMarker::new() {
                    let message = tr!(ProfileBadName, name = e.given());
                    dialogs::tell(None, ui::Style::Warning, &message, "", mtm);
                }
            }
            Ok(n) if n == profile::DEFAULT_NAME => start_profile(None),
            Ok(n) => match create(&dirs_of(Some(&n))) {
                Ok(()) => start_profile(Some(&n)),
                Err(e) => cannot_start(e),
            },
        }
    });
}

/// File > Profiles > Open Profile Folder: this profile's settings folder in Finder.
pub fn open_folder() {
    let dir = paths::dirs().config;
    if let Err(e) = rookparcel_core::util::private_file::create_dir_all(&dir) {
        eprintln!("cannot create {}: {e}", dir.display());
    }
    let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&dir.to_string_lossy()), true);
    if !NSWorkspace::sharedWorkspace().openURL(&url) {
        eprintln!("cannot open {}", dir.display());
    }
}
