//! Quitting, logging out and the main window's close and minimize (GTK `quit_app`, `shutdown`,
//! `toggle_window`, `start_tray`):
//!
//! - Quit (the menus, the status item, Auto Shutdown) and a logout, restart or shutdown of the
//!   Mac all come to `applicationShouldTerminate:` ([`should_terminate`]): the main window goes,
//!   `Core::shutdown` saves the settings and starts stopping the engine, and the answer is
//!   `NSTerminateLater` until the closing window ([`super::closing`], D47) sees the engine done.
//!   Before a power-off (`NSWorkspaceWillPowerOffNotification`, which comes first) the closing
//!   window shows at once.
//! - The close button follows Preferences' "Close to tray" (`ui.close_to_tray`, on by default):
//!   the window hides and the Dock icon or the status item brings it back; off, closing quits.
//!   "Minimize to tray" (`ui.minimize_to_tray`) hides a minimized window from the Dock.
//! - `--minimized` (the LaunchAgent's "Start minimized") starts with the window hidden.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, available};
use objc2_app_kit::{
    NSApplication, NSApplicationTerminateReply, NSNormalWindowLevel, NSWindow, NSWindowNumberListOptions,
    NSWorkspace, NSWorkspaceWillPowerOffNotification,
};
use objc2_foundation::{NSActivityOptions, NSNotification, NSProcessInfo, NSString};
use rookparcel_frontend::integration::MINIMIZED;
use rookparcel_frontend::{profile, tr};

use crate::app::{peek, ui};
use crate::mainq;

thread_local! {
    /// The Mac is logging out, restarting or shutting down.
    static LOGOUT: Cell<bool> = const { Cell::new(false) };
    /// The application is ending (`applicationShouldTerminate:` came): windows that close now
    /// are not answered (a first-start Setup Guide quit leaves the settings to ask again, D53).
    static QUITTING: Cell<bool> = const { Cell::new(false) };
    /// A minimized window is on its way from the Dock into the status item.
    static TO_TRAY: Cell<bool> = const { Cell::new(false) };
    static OBSERVERS: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>> = const { RefCell::new(Vec::new()) };
}

/// `ROOKPARCEL_DEMO_SLOW_EXIT=N`: the simulated engine takes N seconds to stop (`--demo`), to
/// look at the closing window.
pub fn demo_slow_exit() -> Option<std::time::Duration> {
    std::env::var("ROOKPARCEL_DEMO_SLOW_EXIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(std::time::Duration::from_secs)
}

/// The window and `Core` are there (`mac::started`): power-off, the translocation notice,
/// `--minimized` or the startup association check.
pub fn started(mtm: MainThreadMarker) {
    watch_power_off();
    keep_awake();
    let window = ui().map(|u| u.window.clone());
    // macOS runs a quarantined app from a random read-only copy, where a portable marker beside
    // Rookparcel.app is not seen (D42): say how to get it run from where it is
    let translocated = std::env::current_exe().ok().and_then(|e| e.parent().map(profile::translocated));
    if translocated == Some(true) {
        crate::ui::sheet(
            window.as_deref(),
            crate::ui::Style::Info,
            tr!(MacTranslocated),
            "",
            &[],
            |_| {},
            mtm,
        );
    }
    if std::env::args().any(|a| a == MINIMIZED) {
        hide_window();
        return;
    }
    let check = peek(|c| c.gui_options().check_association && !c.is_remote()).unwrap_or(false);
    if check && super::assoc::settable() {
        super::assoc::check(mtm);
    }
}

/// Brings the application forward (`activate` from macOS 14, the older call before).
pub fn activate(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    if available!(macos = 14.0) {
        app.activate();
    } else {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

/// The main window to the front (restored if it was minimized), the application with it. Our
/// windows that were in front of it (Add New Torrent, Preferences, the RSS Downloader; every one
/// that is open while it is hidden) stay in front, in their order, the front one with the
/// keyboard: a click on the status item must not cover an unanswered dialog.
pub fn show_window() {
    let (Some(u), Some(mtm)) = (ui(), MainThreadMarker::new()) else { return };
    let ahead = in_front_of(&u.window, mtm);
    if u.window.isMiniaturized() {
        u.window.deminiaturize(None);
    }
    match ahead.first() {
        None => u.window.makeKeyAndOrderFront(None),
        Some(front) => {
            u.window.orderFront(None);
            for w in ahead.iter().rev() {
                w.orderFront(None);
            }
            front.makeKeyWindow();
        }
    }
    activate(mtm);
}

/// Our ordinary windows in front of `main` on this space, front to back (all of them while
/// `main` is hidden or minimized). Sheets go with their window; floating ones (the countdown,
/// the closing window) and the status item's stay where they are anyway.
fn in_front_of(main: &NSWindow, mtm: MainThreadMarker) -> Vec<Retained<NSWindow>> {
    let app = NSApplication::sharedApplication(mtm);
    let Some(numbers) = NSWindow::windowNumbersWithOptions(NSWindowNumberListOptions::empty(), mtm) else {
        return Vec::new();
    };
    let mut ahead = Vec::new();
    for n in numbers.iter() {
        let n = n.integerValue();
        if n == main.windowNumber() {
            break;
        }
        let Some(w) = app.windowWithWindowNumber(n) else { continue };
        if w.isVisible() && !w.isSheet() && w.level() == NSNormalWindowLevel && w.canBecomeKeyWindow() {
            ahead.push(w);
        }
    }
    ahead
}

/// The main window into the status item (off screen and out of the Dock's window list).
pub fn hide_window() {
    if let Some(u) = ui() {
        u.window.orderOut(None);
    }
}

/// A click on the status item, its Show / Hide and the Dock menu's: the window comes to the
/// front, or goes when it is in front already.
pub fn toggle_window() {
    let (Some(u), Some(mtm)) = (ui(), MainThreadMarker::new()) else { return };
    let active = NSApplication::sharedApplication(mtm).isActive();
    if u.window.isVisible() && !u.window.isMiniaturized() && active {
        hide_window();
    } else {
        show_window();
    }
}

/// `windowShouldClose:`: with "Close to tray" the window hides; without it the application
/// quits. The window itself never closes (it is reused).
pub fn window_should_close() -> bool {
    let to_tray = peek(|c| c.gui_options().close_to_tray).unwrap_or(true);
    if to_tray {
        hide_window();
    } else {
        // after this delegate call has returned
        mainq::on_main(|| {
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        });
    }
    false
}

/// `windowDidMiniaturize:`: with "Minimize to tray" the minimized window leaves the Dock too;
/// the status item or the Dock icon brings it back.
pub fn did_miniaturize() {
    if peek(|c| c.gui_options().minimize_to_tray).unwrap_or(false) {
        // a minimized window ordered out keeps its Dock tile (checked on the VM): it comes back
        // out of the Dock first (after AppKit's own miniaturize has ended) and is hidden when
        // that is done (`did_deminiaturize`)
        TO_TRAY.with(|t| t.set(true));
        mainq::on_main(|| {
            if let Some(u) = ui() {
                u.window.deminiaturize(None);
            }
        });
    }
}

/// `windowDidDeminiaturize:`: the window left the Dock for the status item ([`did_miniaturize`]).
pub fn did_deminiaturize() {
    if TO_TRAY.with(|t| t.replace(false)) {
        // after the notification: ordered out from within it, the Dock tile stayed
        mainq::on_main(hide_window);
    }
}

/// `applicationShouldTerminateAfterLastWindowClosed:`: never; the window hides into the status
/// item, and closing it with "Close to tray" off quits by itself.
pub fn after_last_window_closed() -> bool {
    false
}

/// The application is ending: a window closing now is not the user's answer.
pub fn quitting() -> bool {
    QUITTING.with(Cell::get)
}

/// Quit (the menus, the status item, Auto Shutdown): AppKit does not end an application while
/// a sheet is up (it never asks `applicationShouldTerminate:`), so the sheets go first, as if
/// cancelled (`NSModalResponseAbort`: no button), then the application ends.
pub fn quit() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    for w in app.windows().iter() {
        if let Some(sheet) = w.attachedSheet() {
            w.endSheet_returnCode(&sheet, objc2_app_kit::NSModalResponseAbort);
        }
    }
    app.terminate(None);
}

/// `applicationShouldTerminate:`: Quit, logout, restart and shutdown. Without a session (a
/// start that failed, a second start handing over its files) the application ends at once;
/// otherwise when the closing window has seen the engine done.
pub fn should_terminate(mtm: MainThreadMarker) -> NSApplicationTerminateReply {
    QUITTING.with(|q| q.set(true));
    if super::closing::running() {
        return NSApplicationTerminateReply::TerminateLater;
    }
    let keep = ui();
    super::power::close_countdown();
    // `app::shutdown` puts the layout in the prefs, saves them and starts the engine's stop
    let Some(exit) = crate::app::shutdown() else { return NSApplicationTerminateReply::TerminateNow };
    if let Some(u) = &keep {
        u.window.orderOut(None);
    }
    eprintln!("quit: the engine is stopping");
    super::closing::start(exit, LOGOUT.with(Cell::get), keep, mtm);
    NSApplicationTerminateReply::TerminateLater
}

/// Logout, restart and shutdown announce themselves before `applicationShouldTerminate:`.
fn watch_power_off() {
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let block = RcBlock::new(|_n: NonNull<NSNotification>| {
        LOGOUT.with(|l| l.set(true));
        if super::closing::running() {
            super::closing::show_now();
        }
    });
    // SAFETY: the name is AppKit's constant; with no queue the block runs on the posting thread,
    // the main one for NSWorkspace's notifications; the returned observer is kept.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceWillPowerOffNotification),
            None,
            None,
            &block,
        )
    };
    OBSERVERS.with(|o| o.borrow_mut().push(observer));
}

/// App Nap would slow the engine's threads to a crawl whenever the window is hidden or covered
/// (their timers and sleeps stretch to seconds: transfers stall, a quit takes minutes). A
/// BitTorrent client works in the background by design: one user-initiated activity for the
/// process's life keeps it awake, still letting the Mac sleep when idle.
fn keep_awake() {
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
        &NSString::from_str(&profile::name_build()),
    );
    OBSERVERS.with(|o| o.borrow_mut().push(activity));
}

/// Keeps a notification observer for the process's life.
pub fn keep_observer(o: Retained<ProtocolObject<dyn NSObjectProtocol>>) {
    OBSERVERS.with(|v| v.borrow_mut().push(o));
}
