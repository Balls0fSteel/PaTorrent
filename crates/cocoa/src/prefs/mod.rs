//! Preferences (slice E, docs/MACOS_GUI.md "Preferences: instant apply"): one window, not modal,
//! with the pages of `pref_pages::pages` in a source-list sidebar (an SF Symbol and the title
//! each, System Settings style) and the chosen page beside it, an `NSGridView` built from the
//! page's items (`controls`). The window's title follows the page.
//!
//! There is no OK, Cancel or Apply (D61). A control commits on its own: a check box or a pop-up
//! when clicked, a text field on Return, when it loses the focus and when the window closes, a
//! folder or file field after its panel. A commit goes through `pref_pages::live::Live`, which
//! applies what can be stored (`Core::apply_options`, and the desktop integration through
//! `mac::set_integration`) and keeps a value that cannot be stored in its control, with the
//! message in red under it; closing the window puts such a field's applied text back.
//!
//! Help (D55): each control's tooltip is its help text; the line under the page shows the help
//! of the control under the pointer, else of the focused one (the Advanced table's: the
//! selected row's); a help button opens the wiki page once `help::wiki_url` has one.
//!
//! Every control's action is `Command::Pref(i)`, an index into this window's [`Act`]s, run by
//! [`run`] through `app::run` (convention 2). The window's state is in [`STATE`], apart from the
//! application's: AppKit is only called with no `RefCell` of it borrowed.

pub mod advanced;
pub mod controls;
pub mod sched_grid;
mod tour;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSBackingStoreType, NSBezelStyle, NSBox, NSBoxType, NSButton, NSColor, NSControl, NSGridRow,
    NSOutlineView, NSPopUpButton, NSScrollView, NSSplitViewController, NSSplitViewItem, NSTabView,
    NSTabViewItem, NSTabViewType, NSTextField, NSView, NSViewController, NSWindow, NSWindowStyleMask,
    NSWindowWillCloseNotification, NSWorkspace,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString,
    NSTimer, NSURL,
};
use rookparcel_frontend::help;
use rookparcel_frontend::lang::{self, H};
use rookparcel_frontend::options::{self, Options};
use rookparcel_frontend::pref_pages::live::Live;
use rookparcel_frontend::pref_pages::{
    self, Action, Control, Features, Field, Item, Page, SCHED_ENABLE, TextKind, is_on,
};
use rookparcel_frontend::scheduler::{CELLS, Mode};
use rookparcel_frontend::tr;

use crate::app::{peek, with_core};
use crate::canvas::RPCanvas;
use crate::table::{self, CellKind, CellValue, ColumnSpec, Hooks, RPTableSource, TableKind};
use crate::ui::{self, layout};

/// What a control of the window does (`Command::Pref` carries its index).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Act {
    /// A field's control committed.
    Field(&'static str),
    /// A folder or file field's button: its panel.
    Browse(&'static str),
    /// A page's button.
    Button(Action),
    /// The Advanced table's filter changed.
    AdvFilter,
    /// The Advanced table's value editor committed.
    AdvEditor,
    /// The Advanced table's Reset: the selected setting back to its default.
    AdvReset,
    /// The help button: the help line's wiki page.
    Help,
}

/// A field's control.
pub(crate) enum Ctl {
    Check(Retained<NSButton>),
    /// Numbers, texts, passwords, folders and files.
    Text(Retained<NSTextField>),
    /// A pop-up and its choices' values.
    Choice(Retained<NSPopUpButton>, Vec<String>),
}

impl Ctl {
    /// The value as `Field::shown` gives it.
    pub(crate) fn value(&self) -> String {
        match self {
            Ctl::Check(b) => ui::checked(b).to_string(),
            Ctl::Text(t) => ui::value(t),
            Ctl::Choice(p, values) => ui::chosen(p).and_then(|i| values.get(i)).cloned().unwrap_or_default(),
        }
    }

    pub(crate) fn set(&self, value: &str) {
        match self {
            Ctl::Check(b) => ui::set_checked(b, value == "true"),
            Ctl::Text(t) => t.setStringValue(&NSString::from_str(value)),
            Ctl::Choice(p, values) => {
                p.selectItemAtIndex(values.iter().position(|v| v == value).unwrap_or(0) as isize)
            }
        }
    }

    pub(crate) fn view(&self) -> &NSView {
        match self {
            Ctl::Check(b) => b,
            Ctl::Text(t) => t,
            Ctl::Choice(p, _) => p,
        }
    }
}

/// A field's controls on its page.
pub(crate) struct FieldUi {
    pub field: Field,
    pub ctl: Ctl,
    /// Everything greyed with it: the control, its caption, its Browse button.
    pub grey: Vec<Retained<NSControl>>,
    /// The red message under it and the grid row holding it (text fields only).
    pub error: Option<(Retained<NSTextField>, Retained<NSGridRow>)>,
}

/// The open window.
pub(crate) struct State {
    window: Retained<NSWindow>,
    /// Kept: the window's content is its view.
    _split: Retained<NSSplitViewController>,
    pages: Vec<Page>,
    tabs: Retained<NSTabView>,
    page_list: Retained<NSOutlineView>,
    page_source: Retained<RPTableSource>,
    /// The page indices in the sidebar's row order.
    order: Vec<usize>,
    page: Cell<usize>,
    pub(crate) fields: HashMap<&'static str, FieldUi>,
    acts: Vec<Act>,
    /// Every view with a help text (controls and their captions).
    helps: Vec<(Retained<NSView>, H)>,
    help_line: Retained<NSTextField>,
    help_button: Retained<NSButton>,
    help_shown: Cell<Option<H>>,
    ip_status: Option<Retained<NSTextField>>,
    pub(crate) advanced: Option<advanced::Advanced>,
    sched: Option<Retained<RPCanvas>>,
    live: RefCell<Live>,
    /// The options as applied, with the desktop integration as the system has it.
    pub(crate) applied: RefCell<Options>,
    /// The help line's poll while the window is open.
    timer: RefCell<Option<Retained<NSTimer>>>,
    /// The window's close notification.
    _observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

thread_local! {
    /// The window, once built; it lives as long as the application.
    static STATE: RefCell<Option<Rc<State>>> = const { RefCell::new(None) };
}

/// The window's state (a clone: no borrow is kept).
pub(crate) fn state() -> Option<Rc<State>> {
    STATE.with(|s| s.borrow().clone())
}

/// The window at the page shown last (General the first time).
pub fn open() {
    open_page(None);
}

/// The window at the page with key `key` (a page's, `connection`, or a field's, `port`; the
/// status bar's network part opens Connection while the port is taken).
pub fn open_at(key: &str) {
    open_page(Some(key));
}

fn open_page(key: Option<&str>) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    // the list in "Use only this network" refreshes for the next time it is built (as GTK)
    with_core(|c| c.handle().post_network_adapters());
    let st = match state() {
        Some(st) => {
            if !st.window.isVisible() {
                refresh(&st);
            }
            st
        }
        None => {
            let st = Rc::new(build(mtm));
            STATE.with(|s| *s.borrow_mut() = Some(st.clone()));
            refresh(&st);
            st.page_list.reloadData();
            // SAFETY: nil expands every page with pages under it.
            unsafe { st.page_list.expandItem_expandChildren(None, true) };
            st.window.center();
            st
        }
    };
    let page = key
        .and_then(|k| st.pages.iter().position(|p| p.key == k).or_else(|| page_index(&st.pages, k)))
        .unwrap_or_else(|| st.page.get());
    show_page_selected(&st, page);
    st.window.makeKeyAndOrderFront(None);
    start_timer(&st);
    tour::start();
    // a field's key: that field takes the focus
    if let Some(f) = key.and_then(|k| st.fields.get(k)) {
        st.window.makeFirstResponder(Some(f.ctl.view()));
    }
}

/// The page holding the field `key`.
fn page_index(pages: &[Page], key: &str) -> Option<usize> {
    let page = pref_pages::page_of(pages, key)?;
    pages.iter().position(|p| p.key == page.key)
}

/// Builds the window.
fn build(mtm: MainThreadMarker) -> State {
    let updates = peek(|c| c.updates_here()).unwrap_or(false);
    let mut pages = pref_pages::pages(Features {
        tray: true,
        desktop: true,
        packaged: false,
        updates,
        alternate_rows: true,
    });
    if !crate::mac::associations_settable() {
        // outside the bundle or in another profile: no bundle id to register (autostart works)
        for page in &mut pages {
            for item in &mut page.items {
                if let Item::Field(f) = item {
                    if matches!(f.key, "assoc.torrent" | "assoc.magnet" | "ui.check_assoc") {
                        f.fixed = true;
                    }
                }
            }
        }
    }
    let mut applied = peek(|c| c.options()).unwrap_or_else(|| Options::load(&Default::default(), ""));
    applied.integration = crate::mac::integration();

    let mut b = controls::Builder::new(mtm);
    let tabs = NSTabView::new(mtm);
    tabs.setTabViewType(NSTabViewType::NoTabsNoBorder);
    ui::controls::auto(&tabs);
    for page in &pages {
        let item = NSTabViewItem::new();
        item.setView(Some(&b.page(page, &applied)));
        tabs.addTabViewItem(&item);
    }

    // the sidebar: the top pages, each followed by the pages under it (Disk Cache under Advanced)
    let mut order = Vec::new();
    for (i, p) in pages.iter().enumerate().filter(|(_, p)| p.parent.is_none()) {
        order.push(i);
        order.extend(pages.iter().enumerate().filter(|(_, c)| c.parent == Some(p.key)).map(|(j, _)| j));
    }
    let (list_scroll, page_list, page_source) = page_list(&pages, &order, mtm);

    // the help line under the page, and the help button when there is a wiki
    let help_line = ui::note("", mtm);
    help_line.setPreferredMaxLayoutWidth(480.0);
    help_line.setSelectable(true);
    let help_button = ui::button("", b.act(Act::Help), mtm);
    help_button.setBezelStyle(NSBezelStyle::HelpButton);
    help_button.setHidden(true);
    let spacer = layout::spacer(mtm);
    let help_row = layout::hstack(&[&help_line, &spacer, &help_button], mtm);
    help_row.setAlignment(objc2_app_kit::NSLayoutAttribute::Top);
    let separator = NSBox::new(mtm);
    separator.setBoxType(NSBoxType::Separator);
    ui::controls::auto(&separator);
    let right = NSView::new(mtm);
    for v in [&*tabs as &NSView, &separator, &help_row] {
        right.addSubview(v);
    }
    layout::activate(&[
        tabs.topAnchor().constraintEqualToAnchor(&right.topAnchor()),
        tabs.leadingAnchor().constraintEqualToAnchor(&right.leadingAnchor()),
        tabs.trailingAnchor().constraintEqualToAnchor(&right.trailingAnchor()),
        tabs.bottomAnchor().constraintEqualToAnchor(&separator.topAnchor()),
        separator.leadingAnchor().constraintEqualToAnchor(&right.leadingAnchor()),
        separator.trailingAnchor().constraintEqualToAnchor(&right.trailingAnchor()),
        help_row.topAnchor().constraintEqualToAnchor_constant(&separator.bottomAnchor(), 8.0),
        help_row.leadingAnchor().constraintEqualToAnchor_constant(&right.leadingAnchor(), layout::MARGIN),
        help_row.trailingAnchor().constraintEqualToAnchor_constant(&right.trailingAnchor(), -layout::MARGIN),
        help_row.bottomAnchor().constraintEqualToAnchor_constant(&right.bottomAnchor(), -10.0),
        // about three lines, so the page does not jump as the help changes
        help_row.heightAnchor().constraintGreaterThanOrEqualToConstant(44.0),
    ]);

    let split = NSSplitViewController::new(mtm);
    split.splitView().setVertical(true);
    let sidebar_item = NSSplitViewItem::sidebarWithViewController(&controller(&list_scroll, mtm));
    let sidebar = sidebar_width(&pages, mtm);
    sidebar_item.setMinimumThickness(sidebar);
    sidebar_item.setMaximumThickness(320.0);
    sidebar_item.setCanCollapse(false);
    split.addSplitViewItem(&sidebar_item);
    split.addSplitViewItem(&NSSplitViewItem::splitViewItemWithViewController(&controller(&right, mtm)));

    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let frame = NSRect::new(NSPoint::new(240.0, 180.0), NSSize::new(860.0, 620.0));
    // SAFETY: a plain titled window; it is released only when we drop it (`setReleasedWhenClosed`).
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the state keeps the window; closing only hides it.
    unsafe { window.setReleasedWhenClosed(false) };
    // wide enough for the widest page (German captions are long), within the screen
    let screen = objc2_app_kit::NSScreen::mainScreen(mtm).map_or(1600.0, |s| s.visibleFrame().size.width);
    let width = (sidebar + b.widest + 2.0 * layout::MARGIN + 24.0).clamp(700.0, (screen - 40.0).max(700.0));
    window.setContentMinSize(NSSize::new(width, 460.0));
    window.setContentViewController(Some(&split));
    // the controller sized the window to its views' least: back to a comfortable size
    window.setContentSize(NSSize::new(width.max(880.0), 640.0));
    window.setTitle(&ui::nstext(tr!(PrefTitle)));

    // the window closing commits the field being edited and gives up the bad values
    let on_close = RcBlock::new(|_n: NonNull<NSNotification>| closing());
    // SAFETY: the notification's name is AppKit's constant; the object is our window; the block
    // runs on the posting (main) thread and captures nothing.
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSWindowWillCloseNotification),
            Some(&window),
            None,
            &on_close,
        )
    };

    let controls::Builder { acts, fields, helps, ip_status, advanced, sched, .. } = b;
    State {
        window,
        _split: split,
        pages,
        tabs,
        page_list,
        page_source,
        order,
        page: Cell::new(0),
        fields,
        acts,
        helps,
        help_line,
        help_button,
        help_shown: Cell::new(None),
        ip_status,
        advanced,
        sched,
        live: RefCell::new(Live::default()),
        applied: RefCell::new(applied),
        timer: RefCell::new(None),
        _observer: observer,
    }
}

fn controller(view: &NSView, mtm: MainThreadMarker) -> Retained<NSViewController> {
    let vc = NSViewController::new(mtm);
    vc.setView(view);
    vc
}

/// The sidebar's width: the longest page title with its symbol (translations differ a lot).
fn sidebar_width(pages: &[Page], mtm: MainThreadMarker) -> f64 {
    let widest = pages
        .iter()
        .map(|p| {
            let l = ui::label(&ui::text(lang::text(p.title)), mtm);
            let w = l.fittingSize().width;
            if p.parent.is_some() { w + 16.0 } else { w }
        })
        .fold(0.0, f64::max);
    (widest + 64.0).clamp(170.0, 320.0)
}

/// The sidebar: a source list of the pages, an item's id its page's index + 1.
fn page_list(
    pages: &[Page],
    order: &[usize],
    mtm: MainThreadMarker,
) -> (Retained<NSScrollView>, Retained<NSOutlineView>, Retained<RPTableSource>) {
    let mut spec = ColumnSpec::text("page", "", 160.0);
    spec.kind = CellKind::IconText;
    spec.hideable = false;
    let titles: Vec<(lang::S, Option<&'static str>, &'static str)> =
        pages.iter().map(|p| (p.title, p.parent, p.key)).collect();
    let cells = titles.clone();
    let mut hooks = Hooks::flat(
        || 0,
        move |id, _, _dark| {
            let Some(&(title, ..)) = (id as usize).checked_sub(1).and_then(|i| cells.get(i)) else {
                return CellValue::Empty;
            };
            let text = ui::text(lang::text(title));
            CellValue::IconText(crate::images::symbol(crate::symbols::page(title), &text), text)
        },
    );
    let tree = titles.clone();
    hooks.children = Some(Box::new(move |parent| {
        let parent_key = parent.and_then(|id| tree.get(id as usize - 1)).map(|t| t.2);
        tree.iter().enumerate().filter(|(_, (_, p, _))| *p == parent_key).map(|(i, _)| i as u64 + 1).collect()
    }));
    let rows: Vec<usize> = order.to_vec();
    // every page with pages under it is expanded: a row is the place in `order`
    hooks.rows_of = Box::new(move |ids| {
        ids.iter().filter_map(|&id| rows.iter().position(|&i| i as u64 + 1 == id)).collect()
    });
    hooks.selection = Some(Box::new(|ids| {
        if let (Some(&id), Some(st)) = (ids.first(), state()) {
            show_page(&st, id as usize - 1);
        }
    }));
    table::outline(TableKind("preferences"), vec![spec], hooks, true, mtm)
}

/// Selects page `i` in the sidebar (which shows it).
fn show_page_selected(st: &State, i: usize) {
    st.page_source.select(&[i as u64 + 1]);
    show_page(st, i);
}

/// Shows page `i`: its view, the title.
fn show_page(st: &State, i: usize) {
    let Some(page) = st.pages.get(i) else { return };
    st.page.set(i);
    if st.tabs.selectedTabViewItem().is_none_or(|t| st.tabs.indexOfTabViewItem(&t) != i as isize) {
        st.tabs.selectTabViewItemAtIndex(i as isize);
        if let Some(view) = st.tabs.tabViewItemAtIndex(i as isize).view(MainThreadMarker::from(&*st.window)) {
            controls::scroll_to_top(&view);
        }
    }
    st.window.setTitle(&ui::nstext(lang::text(page.title)));
}

/// The controls from the options as they are now (the window opens or reopens).
fn refresh(st: &State) {
    let mut now = peek(|c| c.options()).unwrap_or_else(|| st.applied.borrow().clone());
    now.integration = crate::mac::integration();
    let _ = st.live.borrow_mut().revert();
    let values = st.live.borrow_mut().write(&st.pages, &now);
    *st.applied.borrow_mut() = now;
    write_values(st, &values);
    show_errors(st, &[]);
    update_ip_status(st, None);
}

/// Writes values into their controls and redoes what follows from them.
fn write_values(st: &State, values: &[(&'static str, String)]) {
    for (key, value) in values {
        let Some(f) = st.fields.get(key) else { continue };
        // a field being typed in keeps its text: it commits when its editing ends
        if matches!(&f.ctl, Ctl::Text(t) if t.currentEditor().is_some()) {
            continue;
        }
        f.ctl.set(value);
    }
    let table = st.applied.borrow().gui.schedule.table;
    if let Some(c) = &st.sched {
        sched_grid::set_table(c, table);
    }
    if let Some(a) = &st.advanced {
        a.reload();
    }
    update_greying(st);
}

/// The value a field's control holds now.
fn value_of(st: &State, key: &str) -> String {
    st.fields.get(key).map(|f| f.ctl.value()).unwrap_or_default()
}

/// Greys what needs a box that is off (`Field::enabled`), and what is fixed.
fn update_greying(st: &State) {
    let on = |k: &str| st.fields.get(k).is_some_and(|f| is_on(&f.field.control, &f.ctl.value()));
    for f in st.fields.values() {
        let enabled = !f.field.fixed && f.field.enabled(on);
        for c in &f.grey {
            c.setEnabled(enabled);
        }
    }
    if let Some(c) = &st.sched {
        sched_grid::set_enabled(c, on(SCHED_ENABLE));
    }
}

/// Shows these messages under their fields and hides the others'.
fn show_errors(st: &State, errors: &[(&'static str, String)]) {
    for (key, f) in &st.fields {
        let Some((label, row)) = &f.error else { continue };
        match errors.iter().find(|(k, _)| k == key) {
            Some((_, message)) => {
                label.setStringValue(&NSString::from_str(message));
                row.setHidden(false);
            }
            None => row.setHidden(true),
        }
    }
    // a message for a field without a place for it (none today): a sheet
    if let Some((_, message)) =
        errors.iter().find(|(k, _)| st.fields.get(k).is_none_or(|f| f.error.is_none()))
    {
        sheet(st, ui::Style::Warning, message);
    }
}

fn sheet(st: &State, style: ui::Style, message: &str) {
    if let Some(mtm) = MainThreadMarker::new() {
        ui::sheet(Some(&st.window), style, message, "", &[], |_| {}, mtm);
    }
}

/// A control's action (`Command::Pref(i)`), from `app::run`.
pub fn run(i: u16) {
    let Some(st) = state() else { return };
    let Some(&act) = st.acts.get(i as usize) else { return };
    match act {
        Act::Field(key) => commit(&st, key),
        Act::Browse(key) => browse(&st, key),
        Act::Button(Action::RandomPort) => {
            if let Some(f) = st.fields.get("port") {
                if let Ctl::Text(t) = &f.ctl {
                    // the new port replaces what is being typed there
                    t.abortEditing();
                }
                f.ctl.set(&options::random_port().to_string());
                commit(&st, "port");
            }
        }
        Act::Button(Action::ReloadIpFilter) => reload_ip_filter(&st),
        Act::AdvFilter => advanced::filter_changed(&st),
        Act::AdvEditor => advanced::end_edit(&st),
        Act::AdvReset => advanced::reset(&st),
        Act::Help => {
            if let Some(url) = st.help_shown.get().and_then(help::wiki_url) {
                if let Some(u) = NSURL::URLWithString(&NSString::from_str(&url)) {
                    NSWorkspace::sharedWorkspace().openURL(&u);
                }
            }
        }
    }
}

/// The field `key` committed: applies what can be stored, shows what cannot.
fn commit(st: &State, key: &'static str) {
    let applied = st.applied.borrow().clone();
    // the controls' values first: reading them calls AppKit, which calls nothing of ours back
    let now: HashMap<&'static str, String> = st.fields.iter().map(|(k, f)| (*k, f.ctl.value())).collect();
    let c =
        st.live.borrow_mut().commit(&st.pages, &applied, key, |k| now.get(k).cloned().unwrap_or_default());
    show_errors(st, &c.errors);
    match c.options {
        Some(o) => apply(st, o),
        None => update_greying(st),
    }
}

/// Applies `o` (the desktop integration first, then `Core::apply_options`) and shows what is
/// stored now.
pub(crate) fn apply(st: &State, o: Options) {
    let old = st.applied.borrow().clone();
    if o.integration != old.integration {
        if let Err(e) = crate::mac::set_integration(&old.integration, &o.integration) {
            sheet(st, ui::Style::Error, &tr!(ErrDesktopIntegration, error = e));
        }
    }
    let ev = with_core(|c| c.apply_options(&o));
    let mut now = peek(|c| c.options()).unwrap_or_else(|| o.clone());
    now.integration = crate::mac::integration();
    let values = st.live.borrow_mut().write(&st.pages, &now);
    let language = now.gui.language != old.gui.language;
    let ip_changed = now.gui.ip_filter_path != old.gui.ip_filter_path;
    *st.applied.borrow_mut() = now;
    write_values(st, &values);
    if ip_changed {
        update_ip_status(st, None);
    }
    crate::app::apply(ev);
    if language {
        // the windows are built in the language of the start
        sheet(st, ui::Style::Info, tr!(LangRestart));
    }
}

/// The Scheduler's table was painted.
pub(crate) fn schedule_changed(table: [Mode; CELLS]) {
    let Some(st) = state() else { return };
    let mut o = st.applied.borrow().clone();
    if o.gui.schedule.table != table {
        o.gui.schedule.table = table;
        apply(&st, o);
    }
}

/// A folder or file field's panel; the choice commits.
fn browse(st: &State, key: &'static str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(f) = st.fields.get(key) else { return };
    let Control::Text { kind, .. } = f.field.control else { return };
    let (title, folders) = match kind {
        TextKind::Folder(t) => (t, true),
        TextKind::File(t) => (t, false),
        _ => return,
    };
    let now = value_of(st, key);
    let now = Path::new(now.trim());
    let start = if now.is_dir() { Some(now.to_path_buf()) } else { now.parent().map(Path::to_path_buf) };
    let pick = ui::Pick {
        files: !folders,
        folders,
        start: start.filter(|p| p.is_dir()),
        message: ui::text(lang::text(title)),
        ..Default::default()
    };
    ui::open_panel(
        Some(&st.window),
        pick,
        move |paths| {
            let (Some(path), Some(st)) = (paths.first(), state()) else { return };
            if let Some(f) = st.fields.get(key) {
                f.ctl.set(&path.to_string_lossy());
                commit(&st, key);
            }
        },
        mtm,
    );
}

/// BitTorrent > Reload: loads the block list in the path field now (as GTK).
fn reload_ip_filter(st: &State) {
    if value_of(st, "ipfilter.enable") != "true" {
        update_ip_status(st, Some(Err(tr!(PrefIpFilterOff).to_owned())));
        return;
    }
    let Some(settings) = peek(|c| c.dirs().settings.clone()) else { return };
    let mut gui = st.applied.borrow().gui.clone();
    gui.ip_filter_path = value_of(st, "ipfilter.path").trim().to_owned();
    let file = gui.ip_filter_file(&settings);
    let result = with_core(|c| c.reload_ip_filter(&file))
        .unwrap_or_else(|| Err(tr!(ErrNotRunning).to_owned()))
        .map_err(|e| format!("{}: {e}", file.display()));
    update_ip_status(st, Some(result));
    // the Logger has the line too
    crate::app::apply(with_core(|c| c.events()));
}

/// The IP filter's status line: the reload's result, or where an empty path looks.
fn update_ip_status(st: &State, result: Option<Result<usize, String>>) {
    let Some(label) = &st.ip_status else { return };
    let text = match result {
        Some(Ok(n)) => tr!(PrefIpFilterLoaded, count = n),
        Some(Err(e)) => e,
        None if value_of(st, "ipfilter.path").trim().is_empty() => {
            let dir = peek(|c| c.dirs().settings.clone()).unwrap_or_default();
            tr!(PrefIpFilterDefault, path = dir.join("ipfilter.dat").display())
        }
        None => String::new(),
    };
    label.setStringValue(&NSString::from_str(&text));
}

/// The window closes: the field being edited commits (its end-editing action), then a field
/// whose value could not be stored gets its applied text back.
fn closing() {
    let Some(st) = state() else { return };
    st.window.makeFirstResponder(None);
    let back = st.live.borrow_mut().revert();
    for (key, value) in &back {
        if let Some(f) = st.fields.get(key) {
            f.ctl.set(value);
        }
    }
    show_errors(&st, &[]);
    update_greying(&st);
    if let Some(a) = &st.advanced {
        a.cancel_edit();
    }
    if let Some(t) = st.timer.borrow_mut().take() {
        t.invalidate();
    }
}

/// Polls what is under the pointer and what has the focus for the help line, while open.
fn start_timer(st: &State) {
    if st.timer.borrow().is_some() {
        return;
    }
    let block = RcBlock::new(|_t: NonNull<NSTimer>| update_help());
    // SAFETY: the block runs on the main run loop (the timer is added to it below) and
    // captures nothing.
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(0.2, true, &block) };
    // SAFETY: the common modes constant; the main run loop keeps the timer until it is
    // invalidated when the window closes.
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    *st.timer.borrow_mut() = Some(timer);
}

/// The help line: the help of the control under the pointer, else of the focused one.
fn update_help() {
    let Some(st) = state() else { return };
    if !st.window.isVisible() {
        return;
    }
    let hovered = st.window.contentView().and_then(|content| {
        let p = st.window.mouseLocationOutsideOfEventStream();
        content.hitTest(p).and_then(|v| help_of(&st, &v))
    });
    let h = hovered.or_else(|| {
        let r = st.window.firstResponder()?;
        let any: &AnyObject = r.as_ref();
        any.downcast_ref::<NSView>().and_then(|v| help_of(&st, v))
    });
    if h == st.help_shown.get() {
        return;
    }
    st.help_shown.set(h);
    st.help_line.setStringValue(&NSString::from_str(h.map_or("", lang::help)));
    st.help_button.setHidden(h.and_then(help::wiki_url).is_none());
    if let Some(a) = &st.advanced {
        // the table's tooltip is the selected row's help
        ui::tooltip(&a.table, a.row_help().map_or("", lang::help));
    }
}

/// The help of `view` or the control holding it (a text field's editor, a cell).
fn help_of(st: &State, view: &NSView) -> Option<H> {
    let mut at = Some(view.retain());
    while let Some(v) = at {
        if let Some(a) = &st.advanced {
            let table: &NSView = &a.table;
            if std::ptr::eq(table, &*v) {
                return a.row_help();
            }
        }
        if let Some((_, h)) = st.helps.iter().find(|(w, _)| std::ptr::eq::<NSView>(&**w, &*v)) {
            return Some(*h);
        }
        // SAFETY: the views are in the window's hierarchy, which we hold.
        at = unsafe { v.superview() };
    }
    None
}

/// The red of a message under a field.
pub(crate) fn error_color() -> Retained<NSColor> {
    NSColor::systemRedColor()
}
