//! Torrent dialogs (slice F, docs/MACOS_GUI.md): Add New Torrent ([`add`]), Add Torrent from URL
//! ([`add_url`]), Create New Torrent ([`create`]), Properties ([`properties`]) and the one-line
//! input ([`input`]).
//!
//! Their controls run through `RPTarget` like every other control (convention 2): an open dialog
//! takes a number ([`new_id`]) and a handler ([`set_handler`]), and its controls carry
//! `Command::Dialog` values ([`cmd`]) made of that number and an action; `app::run` hands them to
//! [`run`], which calls the handler with the action. The handler owns its dialog: [`retire`]
//! (when the dialog closes) lets it go once the current event is over. The windows' close button
//! comes back the same way ([`on_close`], a `NSWindowWillCloseNotification` observer).
//!
//! Test aids: the VM is driven over SSH, where the system's open and save panels cannot be
//! filled and, while its screen is locked, no window's buttons can be reached. All of these are
//! unset in normal use, and nothing else reads them:
//!
//! - `ROOKPARCEL_TEST_PICK`: these dialogs' open panels are not shown; they answer at once with
//!   those of its paths (separated as in `PATH`) the panel would take (`.torrent` files, folders).
//! - `ROOKPARCEL_TEST_SAVE`: their save panels answer with that path.
//! - `ROOKPARCEL_TEST_ACCEPT=N`: each of these dialogs presses its default button N seconds after
//!   it opened (Create New Torrent first takes its source folder from `ROOKPARCEL_TEST_PICK`).

pub mod add;
pub mod add_url;
pub mod create;
pub mod input;
pub mod properties;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSFont, NSStackView, NSView, NSWindow, NSWindowStyleMask,
    NSWindowWillCloseNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize};

use crate::commands::Command;
use crate::ui::{self, layout, ns};

/// Bits of a `Command::Dialog` value that carry the action; the rest is the dialog's number.
const ACT_BITS: u16 = 6;
const ACT_MASK: u16 = (1 << ACT_BITS) - 1;
/// The highest dialog number.
const MAX_ID: u16 = u16::MAX >> ACT_BITS;

/// Action 0: a control that does nothing when clicked (its state is read later).
pub const NOTHING: u8 = 0;

type Handler = Rc<dyn Fn(u8)>;

thread_local! {
    static HANDLERS: RefCell<HashMap<u16, Handler>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u16> = const { Cell::new(1) };
    /// Handlers of closed dialogs, dropped after the event that closed them.
    static RETIRED: RefCell<Vec<Handler>> = const { RefCell::new(Vec::new()) };
}

/// A number for a new dialog (not one in use).
pub fn new_id() -> u16 {
    HANDLERS.with(|h| {
        let h = h.borrow();
        loop {
            let id = NEXT.with(|n| {
                let id = n.get();
                n.set(if id >= MAX_ID { 1 } else { id + 1 });
                id
            });
            if !h.contains_key(&id) {
                return id;
            }
        }
    })
}

/// The handler of dialog `id`: it gets the actions of its controls.
pub fn set_handler(id: u16, f: impl Fn(u8) + 'static) {
    HANDLERS.with(|h| h.borrow_mut().insert(id, Rc::new(f)));
}

/// The command a control of dialog `id` carries for `act`.
pub fn cmd(id: u16, act: u8) -> Command {
    Command::Dialog((id << ACT_BITS) | (u16::from(act) & ACT_MASK))
}

/// `Command::Dialog(v)` from `app::run`.
pub fn run(v: u16) {
    act(v >> ACT_BITS, (v & ACT_MASK) as u8);
}

/// Hands `act` to dialog `id` (nothing when it is gone). No borrow is held during the call.
pub fn act(id: u16, act: u8) {
    let f = HANDLERS.with(|h| h.borrow().get(&id).cloned());
    if let Some(f) = f {
        f(act);
    }
}

/// Dialog `id` closed: its handler (and the dialog it owns) goes once this event is over.
pub fn retire(id: u16) {
    let Some(f) = HANDLERS.with(|h| h.borrow_mut().remove(&id)) else { return };
    RETIRED.with(|r| r.borrow_mut().push(f));
    crate::mainq::on_main(clear_retired);
}

fn clear_retired() {
    let gone = RETIRED.with(|r| std::mem::take(&mut *r.borrow_mut()));
    drop(gone);
}

/// A `NSWindowWillCloseNotification` observer; [`forget`] removes it.
pub type Observer = Retained<ProtocolObject<dyn NSObjectProtocol>>;

/// Runs `act` of dialog `id` when `window` closes (its close button, ⌘W, `close`).
pub fn on_close(window: &NSWindow, id: u16, act: u8) -> Observer {
    let block = RcBlock::new(move |_n: NonNull<NSNotification>| self::act(id, act));
    let object: &AnyObject = window.as_ref();
    // SAFETY: the notification's name constant; the window is the object. A window closes on
    // the main thread and, with no queue, the block runs there at once, in the poster's thread.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSWindowWillCloseNotification),
            Some(object),
            None,
            &block,
        )
    }
}

/// Removes an [`on_close`] observer.
pub fn forget(observer: &Observer) {
    let o: &AnyObject = observer.as_ref();
    // SAFETY: the observer token `addObserverForName:object:queue:usingBlock:` returned.
    unsafe { NSNotificationCenter::defaultCenter().removeObserver(o) };
}

/// A dialog window of `content` (with the window's margins around it): titled, closable, and
/// resizable when `resizable`. Its content size is `size`, or what the content needs.
pub fn window(
    title: &str,
    content: &NSView,
    size: Option<NSSize>,
    resizable: bool,
    mtm: MainThreadMarker,
) -> Retained<NSWindow> {
    let mut style =
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
    if resizable {
        style |= NSWindowStyleMask::Resizable;
    }
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), size.unwrap_or(NSSize::new(400.0, 200.0)));
    // SAFETY: NSWindow's designated initializer with a buffered backing store.
    let w = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the window is held by its dialog (a `Retained`), so AppKit must not release it on
    // close as well.
    unsafe { w.setReleasedWhenClosed(false) };
    w.setTitle(&ns(title));
    let host = NSView::new(mtm);
    host.addSubview(content);
    layout::pin(content, &host, layout::MARGIN);
    w.setContentView(Some(&host));
    let fitting = host.fittingSize();
    w.setContentMinSize(fitting);
    match size {
        Some(s) => w.setContentSize(NSSize::new(s.width.max(fitting.width), s.height.max(fitting.height))),
        None => w.setContentSize(fitting),
    }
    w
}

/// A vertical stack whose views take its whole width.
pub fn column(views: &[&NSView], mtm: MainThreadMarker) -> Retained<NSStackView> {
    let s = layout::vstack(views, mtm);
    let edges: Vec<_> = views
        .iter()
        .flat_map(|v| {
            [
                v.leadingAnchor().constraintEqualToAnchor(&s.leadingAnchor()),
                v.trailingAnchor().constraintEqualToAnchor(&s.trailingAnchor()),
            ]
        })
        .collect();
    layout::activate(&edges);
    s
}

/// A titled part of a dialog (µTorrent's group boxes, as a bold heading over its controls).
pub fn section(title: &str, content: &NSView, mtm: MainThreadMarker) -> Retained<NSStackView> {
    let heading = ui::label_tr(title, mtm);
    heading.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize())));
    let s = column(&[&heading, content], mtm);
    s.setSpacing(6.0);
    s
}

/// The dialog's bottom row: the `leading` views, then at the trailing edge Cancel and the default
/// button `ok` (Return; Cancel gets Escape).
pub fn buttons(
    leading: &[&NSView],
    cancel: &NSButton,
    ok: &NSButton,
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    ok.setKeyEquivalent(&ns("\r"));
    cancel.setKeyEquivalent(&ns("\u{1b}"));
    let spacer = layout::spacer(mtm);
    let mut views: Vec<&NSView> = leading.to_vec();
    views.push(&spacer);
    views.push(cancel);
    views.push(ok);
    layout::hstack(&views, mtm)
}

/// The main window while it is on screen (sheets go there).
pub fn main_window() -> Option<Retained<NSWindow>> {
    crate::app::ui().map(|u| u.window.clone()).filter(|w| w.isVisible())
}

/// A message: a sheet on `window`, else on the main window, else an application-modal alert.
pub fn tell(window: Option<&NSWindow>, style: ui::Style, message: &str, detail: &str, mtm: MainThreadMarker) {
    let main = main_window();
    let on = window.filter(|w| w.isVisible()).or(main.as_deref());
    ui::sheet(on, style, message, detail, &[], |_| {}, mtm);
}

fn test_answer(var: &str) -> Option<Vec<PathBuf>> {
    let v = std::env::var_os(var).filter(|v| !v.is_empty())?;
    Some(std::env::split_paths(&v).collect())
}

/// Of `ROOKPARCEL_TEST_PICK`'s paths, those `pick` would let the user choose.
fn picked(paths: Vec<PathBuf>, pick: &ui::Pick) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| if p.is_dir() { pick.folders } else { pick.files })
        .filter(|p| {
            p.is_dir()
                || pick.extensions.is_empty()
                || p.extension().is_some_and(|e| pick.extensions.iter().any(|x| e.eq_ignore_ascii_case(x)))
        })
        .collect();
    if !pick.multiple {
        out.truncate(1);
    }
    eprintln!("ROOKPARCEL_TEST_PICK: answering an open panel with {out:?}");
    out
}

/// `ROOKPARCEL_TEST_ACCEPT` (see the module's notes): runs `act` of dialog `id` that many seconds
/// from now.
pub fn test_accept(id: u16, act: u8) {
    let Some(secs) = std::env::var("ROOKPARCEL_TEST_ACCEPT").ok().and_then(|s| s.trim().parse::<f64>().ok())
    else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs_f64(secs.max(0.0)));
        crate::mainq::on_main(move || {
            eprintln!("ROOKPARCEL_TEST_ACCEPT: dialog {id}, action {act}");
            self::act(id, act);
        });
    });
}

/// The system's open panel, or `ROOKPARCEL_TEST_PICK`'s paths (see the module's notes).
pub fn open_panel(
    window: Option<&NSWindow>,
    pick: ui::Pick,
    done: impl FnOnce(Vec<PathBuf>) + 'static,
    mtm: MainThreadMarker,
) {
    match test_answer("ROOKPARCEL_TEST_PICK") {
        Some(paths) => done(picked(paths, &pick)),
        None => ui::open_panel(window, pick, done, mtm),
    }
}

/// The system's save panel, or `ROOKPARCEL_TEST_SAVE`'s path (see the module's notes).
pub fn save_panel(
    window: Option<&NSWindow>,
    name: &str,
    start: Option<PathBuf>,
    done: impl FnOnce(Option<PathBuf>) + 'static,
    mtm: MainThreadMarker,
) {
    match test_answer("ROOKPARCEL_TEST_SAVE") {
        Some(paths) => {
            eprintln!("ROOKPARCEL_TEST_SAVE: answering a save panel with {paths:?}");
            done(paths.into_iter().next())
        }
        None => ui::save_panel(window, name, start, done, mtm),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_carry_the_dialog_and_the_action() {
        for (id, a) in [(1u16, 0u8), (7, 5), (MAX_ID, 63)] {
            let Command::Dialog(v) = cmd(id, a) else { panic!("not a dialog command") };
            assert_eq!((v >> ACT_BITS, (v & ACT_MASK) as u8), (id, a));
            assert_eq!(Command::from_tag(cmd(id, a).tag()), Some(cmd(id, a)));
        }
    }

    #[test]
    fn handlers_get_their_actions_and_numbers_are_not_reused_while_open() {
        let got = Rc::new(Cell::new(0u8));
        let a = new_id();
        let g = got.clone();
        set_handler(a, move |act| g.set(act));
        let Command::Dialog(v) = cmd(a, 9) else { unreachable!() };
        run(v);
        assert_eq!(got.get(), 9);
        for _ in 0..MAX_ID {
            assert_ne!(new_id(), a, "an open dialog's number is skipped");
        }
        HANDLERS.with(|h| h.borrow_mut().remove(&a));
        act(a, 3);
        assert_eq!(got.get(), 9, "a gone dialog gets nothing");
    }
}
