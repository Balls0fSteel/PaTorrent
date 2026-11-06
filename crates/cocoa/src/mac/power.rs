//! Options > Auto Shutdown (GTK `shutdown.rs`, `auto_shutdown_step`): the menu's modes
//! (`auto_shutdown::Mode::MENU`, the checked one `Core::auto_shutdown_mode`; Hibernate greyed:
//! macOS has no hibernation of its own to ask for), the countdown window `Core`'s timer drives
//! (open, the seconds, close; its Cancel turns Auto Shutdown off), and the power actions.
//!
//! Sleep and Shut Down are Apple events to loginwindow, as Apple's Technical Q&A QA1134 sends
//! them: `kAESleep` and `kAEShutDown` of `kCoreEventClass` to the system process
//! (`kSystemProcess`). Shut Down asks the user's apps to quit as the Apple menu's Shut Down
//! does. With `ROOKPARCEL_POWER_DRY_RUN` set, the event is built but not sent, and the action
//! says what it would have done. A failure goes to `Core::power_failed` (the Logger and a
//! message).

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSTextField, NSWindow, NSWindowStyleMask};
use rookparcel_frontend::app::Power;
use rookparcel_frontend::auto_shutdown::{self, Mode, Step};
use rookparcel_frontend::tr;

use crate::commands::Command;
use crate::ui::{self, layout};

const DRY_RUN: &str = "ROOKPARCEL_POWER_DRY_RUN";

/// The countdown window, its text and the mode it counts down to.
type Countdown = (Retained<NSWindow>, Retained<NSTextField>, Mode);

thread_local! {
    /// The countdown while it is open.
    static OPEN: RefCell<Option<Countdown>> = const { RefCell::new(None) };
    /// Shut down once the application's session has stopped.
    static AFTER_EXIT: Cell<Option<Power>> = const { Cell::new(None) };
}

/// Picks an Auto Shutdown mode from the menu (`Command::AutoShutdown(index)`).
pub fn auto_shutdown(index: usize) {
    let Some(&mode) = Mode::MENU.get(index) else { return };
    crate::app::with_core(|c| c.set_auto_shutdown(mode));
}

/// The menu item's state (`app::state`): Hibernate greyed, the current mode checked.
pub fn state(index: u16) -> (bool, Option<bool>) {
    let mode = Mode::MENU.get(usize::from(index)).copied().flatten();
    let current = crate::app::peek(|c| c.auto_shutdown_mode()).flatten();
    let enabled = mode.is_none_or(|m| m.action != auto_shutdown::Action::Hibernate);
    (enabled, Some(mode == current))
}

/// Auto Shutdown's `Events`: the countdown (`auto_shutdown`), a standby now (`power`), a
/// shutdown after the exit (`power_after_exit`), from `app::apply`.
pub fn on_events(auto_shutdown: Option<Step>, power: Option<Power>, power_after_exit: Option<Power>) {
    match auto_shutdown {
        Some(Step::Start(secs)) => {
            if let Some(mode) = crate::app::peek(|c| c.auto_shutdown_mode()).flatten() {
                open(mode, secs);
            }
        }
        Some(Step::Tick(secs)) => tick(secs),
        Some(Step::Abort | Step::Act(_)) => close_countdown(),
        Some(Step::Wait) | None => {}
    }
    if let Some(p) = power_after_exit {
        AFTER_EXIT.with(|a| a.set(Some(p)));
    }
    if let Some(p) = power {
        if let Err(e) = perform(p) {
            crate::app::apply(crate::app::with_core(|c| c.power_failed(&e)));
        }
    }
}

/// The countdown's Cancel (`Command::CancelShutdown`): Auto Shutdown is off again.
pub fn cancel() {
    if OPEN.with(|o| o.borrow().is_none()) {
        return;
    }
    close_countdown();
    crate::app::apply(crate::app::with_core(|c| {
        c.cancel_auto_shutdown();
        c.events()
    }));
}

fn open(mode: Mode, secs: u32) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if OPEN.with(|o| o.borrow().is_some()) {
        return;
    }
    let label = ui::wrapping_label(&auto_shutdown::countdown_text(mode, secs), mtm);
    layout::min_width(&label, 300.0);
    let cancel = ui::button_tr(tr!(BtnCancel), Command::CancelShutdown, mtm);
    // Escape cancels, as in the other GUIs' countdown
    cancel.setKeyEquivalent(&ui::ns("\u{1b}"));
    let row = layout::hstack(&[&layout::spacer(mtm), &cancel], mtm);
    let body = layout::vstack(&[&label, &row], mtm);
    layout::insets(&body, layout::MARGIN, layout::MARGIN, layout::MARGIN, layout::MARGIN);
    layout::activate(&[row.widthAnchor().constraintEqualToAnchor(&label.widthAnchor())]);
    let window = super::window(tr!(ShutdownTitle), &body, NSWindowStyleMask::Titled, mtm);
    window.setLevel(objc2_app_kit::NSFloatingWindowLevel);
    window.center();
    // over the other windows, even with the main one hidden
    window.makeKeyAndOrderFront(None);
    window.orderFrontRegardless();
    OPEN.with(|o| *o.borrow_mut() = Some((window, label, mode)));
}

fn tick(secs: u32) {
    let Some((label, mode)) = OPEN.with(|o| o.borrow().as_ref().map(|(_, l, m)| (l.clone(), *m))) else {
        return;
    };
    label.setStringValue(&ui::ns(&auto_shutdown::countdown_text(mode, secs)));
}

/// Ends the countdown from outside (time up, called off by new work, the application quits).
pub fn close_countdown() {
    if let Some((window, _, _)) = OPEN.with(|o| o.borrow_mut().take()) {
        window.orderOut(None);
    }
}

/// Auto Shutdown's Shut Down, once the session has saved its state and stopped (the closing
/// window's end). Nothing can show a message any more: a failure goes to stderr.
pub fn after_exit() {
    if let Some(Err(e)) = AFTER_EXIT.with(Cell::take).map(perform) {
        eprintln!("{e}");
    }
}

/// What the dry run's message names.
fn what(p: Power) -> &'static str {
    match p {
        Power::Standby => tr!(PowerStandby),
        Power::Hibernate => tr!(PowerHibernate),
        Power::ShutDown => tr!(PowerShutDown),
    }
}

/// Puts the Mac to sleep or shuts it down (the caller saved everything first).
pub fn perform(p: Power) -> Result<(), String> {
    let id = match p {
        Power::Standby => ae::SLEEP,
        Power::ShutDown => ae::SHUT_DOWN,
        // greyed in the menu; Core never asks for it here
        Power::Hibernate => return Err(tr!(ShutdownErrHibernate, error = "macOS")),
    };
    let dry = std::env::var_os(DRY_RUN).is_some();
    let sent = ae::send_to_loginwindow(id, !dry);
    let fail = |code: i16| match p {
        Power::Standby => tr!(ShutdownErrStandby, error = format!("Apple event error {code}")),
        _ => tr!(ShutdownErrShutDown, error = format!("Apple event error {code}")),
    };
    match sent {
        Err(code) => Err(fail(code)),
        Ok(()) if dry => Err(tr!(ShutdownDryRun, name = DRY_RUN, what = what(p))),
        Ok(()) => Ok(()),
    }
}

/// The Apple Event Manager (CoreServices' AE framework), declared by hand.
mod ae {
    use super::c_void;

    type OSErr = i16;
    type FourCc = u32;

    /// `AEDesc` (and `AppleEvent`, the same type).
    #[repr(C)]
    struct AEDesc {
        descriptor_type: FourCc,
        data_handle: *mut c_void,
    }

    /// `ProcessSerialNumber`.
    #[repr(C)]
    struct Psn {
        high: u32,
        low: u32,
    }

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn AECreateDesc(type_code: FourCc, data: *const c_void, size: isize, result: *mut AEDesc) -> OSErr;
        fn AECreateAppleEvent(
            class: FourCc,
            id: FourCc,
            target: *const AEDesc,
            return_id: i16,
            transaction_id: i32,
            result: *mut AEDesc,
        ) -> OSErr;
        /// (AEMach.h: in CoreServices, unlike `AESend`, which is in ApplicationServices.)
        fn AESendMessage(event: *const AEDesc, reply: *mut AEDesc, mode: i32, timeout_ticks: isize) -> i32;
        fn AEDisposeDesc(desc: *mut AEDesc) -> OSErr;
    }

    const fn fourcc(code: &[u8; 4]) -> FourCc {
        u32::from_be_bytes(*code)
    }
    /// `kCoreEventClass`.
    const CORE: FourCc = fourcc(b"aevt");
    /// `kAESleep`.
    pub const SLEEP: FourCc = fourcc(b"slep");
    /// `kAEShutDown`.
    pub const SHUT_DOWN: FourCc = fourcc(b"shut");
    /// `typeProcessSerialNumber`.
    const PSN: FourCc = fourcc(b"psn ");
    /// `kSystemProcess`: loginwindow.
    const SYSTEM_PROCESS: u32 = 1;
    /// `kAutoGenerateReturnID`, `kAnyTransactionID`.
    const AUTO_RETURN_ID: i16 = -1;
    const ANY_TRANSACTION: i32 = 0;
    /// `kAENoReply`, `kAEDefaultTimeout`.
    const NO_REPLY: i32 = 1;
    const DEFAULT_TIMEOUT: isize = -1;

    fn empty() -> AEDesc {
        AEDesc { descriptor_type: fourcc(b"null"), data_handle: std::ptr::null_mut() }
    }

    /// Builds the event `id` for loginwindow and sends it when `send` (QA1134).
    pub fn send_to_loginwindow(id: FourCc, send: bool) -> Result<(), OSErr> {
        let psn = Psn { high: 0, low: SYSTEM_PROCESS };
        let (mut target, mut event, mut reply) = (empty(), empty(), empty());
        // SAFETY: the descriptors are ours, made and disposed here; the PSN is copied in.
        unsafe {
            let mut err = AECreateDesc(
                PSN,
                (&psn as *const Psn).cast(),
                std::mem::size_of::<Psn>() as isize,
                &mut target,
            );
            if err == 0 {
                err = AECreateAppleEvent(CORE, id, &target, AUTO_RETURN_ID, ANY_TRANSACTION, &mut event);
                AEDisposeDesc(&mut target);
            }
            if err == 0 && send {
                let status = AESendMessage(&event, &mut reply, NO_REPLY, DEFAULT_TIMEOUT);
                err = i16::try_from(status).unwrap_or(i16::MIN);
                AEDisposeDesc(&mut reply);
            }
            if !event.data_handle.is_null() {
                AEDisposeDesc(&mut event);
            }
            if err == 0 { Ok(()) } else { Err(err) }
        }
    }
}
