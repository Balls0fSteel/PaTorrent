//! The detail tabs (General, Trackers, Peers, Pieces, Files, Speed, Logger) in the main
//! window's `NSTabView` (docs/MACOS_GUI.md, slice D), drawn from `Core::detail` for the first
//! selected torrent.
//!
//! - [`install`] puts each tab's view into the window's tab view and selects the tab kept in
//!   the preferences (`ui.tab`).
//! - [`show_tab`] (the tab view's delegate) tells `Core`, which then asks the engine for that
//!   tab's data only, and keeps the choice.
//! - [`refresh`] redraws the tab on screen (`app::apply` after `Events::detail`, and after a
//!   selection change).
//! - [`find`]: Edit > Find in the Files tab goes to its filter.
//! - [`run`] and [`state`]: the Files tab's commands (its filter and menu).
//!
//! Every read of `Core` is a shared borrow ([`crate::app::peek`]) dropped before AppKit is
//! called; changes go through `with_core` and `app::apply`.

pub mod files;
pub mod general;
pub mod pieces;
pub mod speed;
pub mod tables;

use std::cell::RefCell;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow};
use rookparcel_frontend::detail::Tab;

use crate::app::{apply, peek, with_core};
use crate::canvas::RPCanvas;
use crate::commands::Command;
use crate::ui::layout;
use crate::window::Ui;

/// The tabs' views.
struct Tabs {
    general: general::General,
    trackers: tables::Table,
    peers: tables::Table,
    pieces: pieces::Pieces,
    files: files::Files,
    speed: Retained<RPCanvas>,
    logger: tables::Table,
}

thread_local! {
    static TABS: RefCell<Option<Rc<Tabs>>> = const { RefCell::new(None) };
}

fn tabs() -> Option<Rc<Tabs>> {
    TABS.with(|t| t.borrow().clone())
}

/// A tab item's view: a plain view the tab view sizes (by its frame), holding `content` pinned
/// to its edges.
fn page(content: &NSView, mtm: MainThreadMarker) -> Retained<NSView> {
    let page = NSView::new(mtm);
    content.setTranslatesAutoresizingMaskIntoConstraints(false);
    page.addSubview(content);
    layout::pin(content, &page, 0.0);
    page
}

/// Builds the tabs into the window's tab view (after the window is built, before it shows
/// data) and selects the kept tab.
pub fn install(ui: &Ui) {
    let mtm = MainThreadMarker::from(&*ui.window);
    general::register();
    pieces::register();
    speed::register();
    let t = Tabs {
        general: general::General::new(mtm),
        trackers: tables::Table::trackers(mtm),
        peers: tables::Table::peers(mtm),
        pieces: pieces::Pieces::new(mtm),
        files: files::Files::new(mtm),
        speed: speed::new(mtm),
        logger: tables::Table::logger(mtm),
    };
    for tab in Tab::ALL {
        let content: Retained<NSView> = match tab {
            Tab::General => t.general.view.clone(),
            Tab::Trackers => Retained::into_super(t.trackers.scroll.clone()),
            Tab::Peers => Retained::into_super(t.peers.scroll.clone()),
            Tab::Pieces => t.pieces.view.clone(),
            Tab::Files => t.files.view.clone(),
            Tab::Speed => Retained::into_super(t.speed.clone()),
            Tab::Logger => Retained::into_super(t.logger.scroll.clone()),
        };
        let item = ui.tabs.tabViewItemAtIndex(tab.index() as isize);
        item.setView(Some(&page(&content, mtm)));
    }
    TABS.with(|s| *s.borrow_mut() = Some(Rc::new(t)));
    let tab = peek(|c| c.tab()).unwrap_or(Tab::General);
    let current = ui.tabs.selectedTabViewItem().map(|i| ui.tabs.indexOfTabViewItem(&i));
    if current != Some(tab.index() as isize) {
        // the delegate's `show_tab` follows
        ui.tabs.selectTabViewItemAtIndex(tab.index() as isize);
    }
    refresh();
}

/// The tab view shows `tab`: `Core` asks for its data (only the shown tab's), the choice is kept
/// (`ui.tab`), and the tab is drawn.
pub fn show_tab(tab: Tab) {
    apply(with_core(|c| {
        c.show_tab(tab);
        c.prefs_mut().set("ui.tab", tab.key());
        c.events()
    }));
}

/// Redraws the tab on screen from `Core::detail`.
pub fn refresh() {
    let (Some(t), Some(tab)) = (tabs(), peek(|c| c.tab())) else { return };
    match tab {
        Tab::General => t.general.refresh(),
        Tab::Trackers => t.trackers.refresh(),
        Tab::Peers => t.peers.refresh(),
        Tab::Pieces => t.pieces.refresh(),
        Tab::Files => t.files.refresh(),
        Tab::Speed => speed::refresh(&t.speed),
        Tab::Logger => t.logger.refresh(),
    }
}

/// Edit > Find while the keyboard is in the Files tab (its list or its filter): the filter
/// takes it (`true`); otherwise the toolbar search does (`false`).
pub fn find() -> bool {
    let (Some(t), Some(ui)) = (tabs(), crate::app::ui()) else { return false };
    if peek(|c| c.tab()) != Some(Tab::Files) || !focus_in(&ui.window, &t.files.view) {
        return false;
    }
    ui.window.makeFirstResponder(Some(&t.files.filter));
    true
}

/// Whether the window's first responder is `view` or inside it (a field being edited answers
/// through its field editor, a subview of the field).
fn focus_in(window: &NSWindow, view: &NSView) -> bool {
    window.firstResponder().and_then(|r| r.downcast::<NSView>().ok()).is_some_and(|v| v.isDescendantOf(view))
}

/// The Files tab's commands (slice D).
pub fn run(cmd: Command) {
    if let Some(t) = tabs() {
        t.files.run(cmd);
    }
}

/// Whether one of the Files tab's commands is enabled now; `None` for the other commands.
pub fn state(cmd: Command) -> Option<(bool, Option<bool>)> {
    if !crate::commands::FILES.iter().any(|c| std::mem::discriminant(c) == std::mem::discriminant(&cmd)) {
        return None;
    }
    Some((tabs().is_some_and(|t| t.files.state(cmd)), None))
}
