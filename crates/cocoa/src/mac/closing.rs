//! The window shown while Rookparcel closes (D47), as the Windows and GTK GUIs show it: the
//! engine stops on its own thread (`Session::begin_shutdown`, through `Core::shutdown`); when
//! one step of that takes more than a second (never the trackers' "stopped" announces,
//! `ShowTimer`), or at once when the Mac is logging out, restarting or shutting down, this
//! window says what it is waiting for, lists the moves still to finish with their progress and
//! destination, and for how long. Quit Now lists what quitting early gives up and asks; it is
//! not offered while the torrent list and progress are being saved. The texts and rules are
//! `frontend::closing`'s.
//!
//! The application waits in `NSTerminateLater` meanwhile (a logout waits with it); once the
//! engine is done, Auto Shutdown's Shut Down runs if it was asked for, and
//! `replyToApplicationShouldTerminate:` lets the application end.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::Instant;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{
    NSApplication, NSButton, NSProgressIndicator, NSProgressIndicatorStyle, NSTextField, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSRunLoop, NSRunLoopCommonModes, NSTimer};
use rookparcel_core::{Exit, ExitStatus};
use rookparcel_frontend::closing::{self, ShowTimer};
use rookparcel_frontend::tr;

use crate::commands::Command;
use crate::table::{self, CellValue, ColumnSpec, Hooks, RPTableSource, RPTableView, TableKind};
use crate::ui::{self, layout};
use crate::window::Ui;

/// The window's width; its height follows what it shows.
const WIDTH: f64 = 560.0;

struct Shown {
    window: Retained<NSWindow>,
    phase: Retained<NSTextField>,
    list: Retained<objc2_app_kit::NSScrollView>,
    table: Retained<RPTableView>,
    _source: Retained<RPTableSource>,
    bar: Retained<NSProgressIndicator>,
    elapsed: Retained<NSTextField>,
    quit: Retained<NSButton>,
}

struct State {
    exit: Option<Exit>,
    start: Instant,
    timer: ShowTimer,
    /// The Mac is logging out, restarting or shutting down: show at once.
    logout: bool,
    shown: Option<Shown>,
    /// What the window shows, to set only what changed.
    last: Option<(ExitStatus, u64)>,
    poll: Option<Retained<NSTimer>>,
    /// The main window's parts, kept alive (hidden) until the end.
    _keep: Option<Rc<Ui>>,
    /// Keeps App Nap from slowing the engine's threads while it saves (a hidden window's
    /// application naps: its timers and sleeps stretch to seconds).
    activity: Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// The moves the list shows (its data source reads them while it reloads).
    static ROWS: RefCell<Vec<[String; 3]>> = const { RefCell::new(Vec::new()) };
}

/// Whether the application is closing (the engine is stopping).
pub fn running() -> bool {
    STATE.with(|s| s.borrow().is_some())
}

/// Follows `exit` until the engine is done (or Quit Now got it past saving), behind the window
/// when it takes long, then lets the application end. `logout`: the Mac's session is ending.
pub fn start(exit: Exit, logout: bool, keep: Option<Rc<Ui>>, mtm: MainThreadMarker) {
    let now = Instant::now();
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            exit: Some(exit),
            start: now,
            timer: ShowTimer::new(now),
            logout,
            shown: None,
            last: None,
            poll: None,
            _keep: keep,
            activity: Some(NSProcessInfo::processInfo().beginActivityWithOptions_reason(
                NSActivityOptions::UserInitiated,
                &ui::nstext(tr!(EndSessionSaving)),
            )),
        })
    });
    let block = RcBlock::new(|_t: NonNull<NSTimer>| tick());
    // SAFETY: the block lives as long as the timer; the common modes include the modal panel
    // mode AppKit waits in after `NSTerminateLater`.
    let timer =
        unsafe { NSTimer::timerWithTimeInterval_repeats_block(closing::REFRESH.as_secs_f64(), true, &block) };
    // SAFETY: the main run loop and the common modes constant.
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.poll = Some(timer);
        }
    });
    // the first look comes with the timer: `applicationShouldTerminate:` must have answered
    // `NSTerminateLater` before `finish` may reply
    let _ = mtm;
}

/// The Mac is logging out or shutting down while Rookparcel closes: the window shows now.
pub fn show_now() {
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.logout = true;
        }
    });
    tick();
}

fn tick() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let status = STATE.with(|st| {
        let st = st.borrow();
        let exit = st.as_ref()?.exit.as_ref()?;
        let s = exit.status();
        Some((!closing::done(exit, &s)).then_some(s))
    });
    let s = match status {
        None => return,
        Some(None) => return finish(mtm),
        Some(Some(s)) => s,
    };
    let show = STATE.with(|st| {
        let mut st = st.borrow_mut();
        let Some(state) = st.as_mut() else { return false };
        let due = state.timer.due(&s, Instant::now());
        state.shown.is_none() && (state.logout || due)
    });
    if show {
        eprintln!("quit: the closing window shows");
        let shown = build(mtm);
        STATE.with(|st| {
            if let Some(state) = st.borrow_mut().as_mut() {
                state.shown = Some(shown);
            }
        });
    }
    refresh(s);
}

/// The engine is done: the window goes, Auto Shutdown's Shut Down runs if asked for, and the
/// application ends.
fn finish(mtm: MainThreadMarker) {
    let Some(mut state) = STATE.with(|s| s.borrow_mut().take()) else { return };
    if let Some(t) = state.poll.take() {
        t.invalidate();
    }
    if let Some(a) = state.activity.take() {
        // SAFETY: the activity `beginActivityWithOptions:reason:` gave, ended once.
        unsafe { NSProcessInfo::processInfo().endActivity(&a) };
    }
    if let Some(exit) = state.exit.take() {
        if exit.is_finished() { exit.wait() } else { exit.abandon() }
    }
    if let Some(shown) = state.shown.take() {
        shown.window.orderOut(None);
    }
    eprintln!("quit: the engine stopped after {} ms", state.start.elapsed().as_millis());
    super::status_item::remove();
    super::power::after_exit();
    drop(state);
    NSApplication::sharedApplication(mtm).replyToApplicationShouldTerminate(true);
}

fn build(mtm: MainThreadMarker) -> Shown {
    let phase = ui::wrapping_label("", mtm);
    let mut specs: Vec<ColumnSpec> = closing::columns()
        .iter()
        .enumerate()
        .map(|(i, t)| ColumnSpec::text(&format!("c{i}"), &ui::text(t), if i == 1 { 130.0 } else { 175.0 }))
        .collect();
    for s in &mut specs {
        s.sortable = false;
        s.hideable = false;
    }
    let hooks = Hooks::flat(
        || ROWS.with(|r| r.borrow().len()),
        |row, col, _dark| {
            ROWS.with(|r| {
                r.borrow()
                    .get(row as usize)
                    .map_or(CellValue::Empty, |cells| CellValue::Text(cells[col].clone()))
            })
        },
    );
    let (list, table, source) = table::table(TableKind("closing"), specs, hooks, None, mtm);
    layout::height(&list, 140.0);
    list.setHidden(true);
    let bar = NSProgressIndicator::new(mtm);
    bar.setStyle(NSProgressIndicatorStyle::Bar);
    bar.setMinValue(0.0);
    bar.setMaxValue(1.0);
    ui::auto(&bar);
    let elapsed = ui::label("", mtm);
    let quit = ui::button_tr(tr!(ExitQuitNow), Command::QuitNow, mtm);
    let bottom = layout::hstack(&[&elapsed, &layout::spacer(mtm), &quit], mtm);
    let body = layout::vstack(&[&phase, &list, &bar, &bottom], mtm);
    layout::insets(&body, layout::MARGIN, layout::MARGIN, layout::MARGIN, layout::MARGIN);
    let views: [&objc2_app_kit::NSView; 4] = [&phase, &list, &bar, &bottom];
    for v in views {
        layout::activate(&[v
            .widthAnchor()
            .constraintEqualToAnchor_constant(&body.widthAnchor(), -2.0 * layout::MARGIN)]);
    }
    layout::width(&body, WIDTH);
    // no close button: Quit Now is the way to cut it short, with its question
    let window = super::window(tr!(ExitTitle), &body, NSWindowStyleMask::Titled, mtm);
    window.center();
    window.makeKeyAndOrderFront(None);
    window.orderFrontRegardless();
    Shown { window, phase, list, table, _source: source, bar, elapsed, quit }
}

/// Shows what the engine is doing now.
fn refresh(s: ExitStatus) {
    let Some((w, secs)) = STATE.with(|st| {
        let st = st.borrow();
        let state = st.as_ref()?;
        let secs = state.start.elapsed().as_secs();
        if state.last.as_ref().is_some_and(|(l, t)| *l == s && *t == secs) {
            return None;
        }
        let w = state.shown.as_ref()?;
        Some((
            (
                w.phase.clone(),
                w.elapsed.clone(),
                w.bar.clone(),
                w.list.clone(),
                w.table.clone(),
                w.quit.clone(),
            ),
            secs,
        ))
    }) else {
        return;
    };
    let (phase, elapsed, bar, list, table, quit) = w;
    phase.setStringValue(&ui::ns(&closing::phase_text(&s)));
    elapsed.setStringValue(&ui::ns(&closing::elapsed_text(secs)));
    match closing::fraction(&s) {
        Some(f) => {
            // SAFETY: a nil sender is allowed.
            unsafe { bar.stopAnimation(None) };
            bar.setIndeterminate(false);
            bar.setDoubleValue(f);
        }
        // runs back and forth for steps it cannot measure
        None => {
            bar.setIndeterminate(true);
            // SAFETY: a nil sender is allowed.
            unsafe { bar.startAnimation(None) };
        }
    }
    let rows = closing::rows(&s);
    let changed = ROWS.with(|r| *r.borrow() != rows);
    if changed {
        let any = !rows.is_empty();
        ROWS.with(|r| *r.borrow_mut() = rows);
        table.reloadData();
        list.setHidden(!any);
    }
    quit.setEnabled(closing::quit_offered(&s));
    STATE.with(|st| {
        if let Some(state) = st.borrow_mut().as_mut() {
            state.last = Some((s, secs));
        }
    });
}

/// Quit Now: says what it gives up and asks, then cuts the waits short (the window ends once
/// the engine is past saving).
pub fn quit_now() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some((s, window)) = STATE.with(|st| {
        let st = st.borrow();
        let state = st.as_ref()?;
        Some((state.exit.as_ref()?.status(), state.shown.as_ref().map(|w| w.window.clone())))
    }) else {
        return;
    };
    if !closing::quit_offered(&s) {
        return;
    }
    let Some(q) = closing::question(&closing::consequences(&s)) else {
        hurry();
        return;
    };
    ui::sheet(
        window.as_deref(),
        ui::Style::Warning,
        tr!(ExitTitle),
        &q,
        &[tr!(BtnCancel), tr!(ExitQuitNow)],
        |b| {
            if b == 1 {
                hurry();
            }
        },
        mtm,
    );
}

fn hurry() {
    STATE.with(|st| {
        if let Some(exit) = st.borrow().as_ref().and_then(|s| s.exit.as_ref()) {
            exit.hurry();
        }
    });
}
