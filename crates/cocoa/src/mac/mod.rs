//! macOS integration (slice G, docs/MACOS_GUI.md "macOS integration"):
//!
//! - [`lifecycle`]: Quit and logout through `applicationShouldTerminate:` and the closing
//!   window ([`closing`], D47); close and minimize into the status item; `--minimized`;
//!   power-off.
//! - [`status_item`]: the menu-bar item with its menu and tooltip.
//! - [`open`]: files and links the system opens with Rookparcel, and a second start handing its
//!   arguments to the running copy.
//! - [`notify`]: completed-download notifications.
//! - [`assoc`] and [`login`]: Preferences > General's associations and LaunchAgent
//!   ([`integration`], [`set_integration`]).
//! - [`power`]: Options > Auto Shutdown's menu, countdown and power actions.
//! - [`about`] and [`update`]: About, Third-Party Notices, the website, Check for Updates.
//! - [`dock`]: the Dock menu's and Dock icon's part.
//! - [`defaults`]: a portable copy leaves `~/Library/Preferences` as it found it.

pub mod about;
pub mod assoc;
pub mod closing;
pub mod defaults;
pub mod dock;
pub mod lifecycle;
pub mod login;
pub mod notify;
pub mod open;
pub mod power;
pub mod status_item;
pub mod update;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use rookparcel_frontend::integration::Integration;
use rookparcel_frontend::tr;

use crate::commands::Command;

/// Preferences > General's desktop integration as the system has it now (associations from
/// LaunchServices, autostart from the LaunchAgent), for Preferences (slice E) to show.
pub fn integration() -> Integration {
    let (torrent, magnet) = assoc::read();
    let (autostart, minimized) = login::read();
    Integration { torrent, magnet, autostart, minimized }
}

/// Writes what differs between `old` (as `integration` read it) and `new`; `Err` is the message
/// for the user (`ErrDesktopIntegration`).
pub fn set_integration(old: &Integration, new: &Integration) -> Result<(), String> {
    let (old, new) = (old.normalized(), new.normalized());
    let fail = |e: String| tr!(ErrDesktopIntegration, error = e);
    assoc::set(new.torrent && !old.torrent, new.magnet && !old.magnet).map_err(fail)?;
    if (new.autostart, new.minimized) != (old.autostart, old.minimized) {
        login::write(new.autostart, new.minimized).map_err(fail)?;
    }
    // LaunchServices keeps a default until another program takes it; there is no reset
    if (old.torrent && !new.torrent) || (old.magnet && !new.magnet) {
        return Err(fail(tr!(MacAssocKept).to_owned()));
    }
    Ok(())
}

/// Whether the associations can be changed here: from the bundle and in the default profile.
pub fn associations_settable() -> bool {
    assoc::settable()
}

/// A window of slice G's own (closing, countdown, notices) around `content`, a stack with its
/// own margins that sizes it (Auto Layout); released only when dropped.
pub(crate) fn window(
    title: &str,
    content: &NSView,
    style: NSWindowStyleMask,
    mtm: MainThreadMarker,
) -> Retained<NSWindow> {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 200.0));
    // SAFETY: a plain window; it is released only when we drop it (`setReleasedWhenClosed`).
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the caller keeps the window; closing only hides it.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&crate::ui::nstext(title));
    let host = NSView::new(mtm);
    host.addSubview(content);
    crate::ui::layout::pin(content, &host, 0.0);
    window.setContentView(Some(&host));
    // as large as the content needs, no more (the frame above is only a start)
    window.setContentSize(host.fittingSize());
    window
}

/// Runs slice G's own commands (`app::run`).
pub fn run(cmd: Command) {
    match cmd {
        Command::StatusItem => status_item::clicked(),
        Command::QuitNow => closing::quit_now(),
        Command::CancelShutdown => power::cancel(),
        _ => {}
    }
}

/// The end of `app::start`, once the window and `Core` are there: the status item, the
/// translocation notice, `--minimized`, the startup association check, what was opened before
/// and what later starts hand over, and power-off.
pub fn started(mtm: MainThreadMarker) {
    // for the logs of test runs (stderr only)
    eprintln!("mac: {:?}, associations settable: {}", integration(), associations_settable());
    status_item::start(mtm);
    lifecycle::started(mtm);
    open::started(mtm);
}
