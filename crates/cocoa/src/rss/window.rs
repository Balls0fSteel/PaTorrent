//! The RSS Downloader window (GTK `rss::show`, µTorrent 2.2.1's Options > RSS Downloader): one,
//! not modal.
//!
//! Left, the Feeds list (All Feeds, then each feed; a double click edits, a right click has the
//! Feeds menu) with Add, Edit, Remove and Update and the picked feed's state under it. Right,
//! two tabs: Filters (the filter list with a check box per filter for on and off, Add, Remove,
//! Up and Down; the picked filter's fields; the items of the feeds' last fetches it matches)
//! and History (what the downloader added, Clear History). Edits go to the live downloader
//! (`Core::edit_rss`): check boxes, pop-ups and the Quality menu at once, text fields on Return
//! or when they lose the focus (and before anything else happens in the window). The window has
//! Close rather than OK and Cancel; closing it runs the filters over the feeds' current items
//! (`Core::rss_closed`).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSComboBox, NSControlStateValueOff, NSControlStateValueOn, NSLayoutAttribute,
    NSLayoutConstraintOrientation, NSMenuItem, NSPopUpButton, NSScrollView, NSStackView,
    NSStackViewDistribution, NSTabView, NSTabViewItem, NSTableViewColumnAutoresizingStyle, NSTextField,
    NSView, NSWindow,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use rookparcel_frontend::lang::{self, S};
use rookparcel_frontend::rss::view::{self, quality_text};
use rookparcel_frontend::rss::{FeedId, FilterId, MIN_INTERVALS, Quality, RssFilter};
use rookparcel_frontend::tr;

use crate::app::{apply, peek, with_core};
use crate::dialogs::{self, Observer, cmd};
use crate::table::{self, CellKind, CellValue, ColumnSpec, Hooks, RPTableSource, RPTableView, TableKind};
use crate::ui::{self, layout, ns};

pub(super) const CLOSE: u8 = 1;
const CLOSED: u8 = 2;
pub(super) const FEED_ADD: u8 = 3;
const FEED_EDIT: u8 = 4;
const FEED_REMOVE: u8 = 5;
const FEED_UPDATE: u8 = 6;
pub(super) const FILTER_ADD: u8 = 7;
const FILTER_REMOVE: u8 = 8;
const FILTER_UP: u8 = 9;
const FILTER_DOWN: u8 = 10;
const BROWSE: u8 = 11;
/// A field, pop-up or check box of the filter changed.
pub(super) const STORE: u8 = 12;
const CLEAR_HISTORY: u8 = 13;
/// The Quality menu: `QUALITY` is All, `QUALITY + 1 + i` is `Quality::LIST[i]`.
const QUALITY: u8 = 32;

/// Most rows the preview shows.
const PREVIEW_ROWS: usize = 500;

/// The Feeds list's id of All Feeds (a feed's id is its `FeedId`, never 0).
const ALL_FEEDS: u64 = 0;

type Rows<T> = Rc<RefCell<Vec<T>>>;

pub(super) struct Dialog {
    id: u16,
    pub(super) window: Retained<NSWindow>,
    observer: RefCell<Option<Observer>>,
    feeds_view: Retained<RPTableView>,
    feeds: Retained<RPTableSource>,
    /// The Feeds list's rows: (feed, name); `None` is All Feeds.
    feed_rows: Rows<(Option<FeedId>, String)>,
    feed_status: Retained<NSTextField>,
    feed_edit: Retained<NSButton>,
    feed_remove: Retained<NSButton>,
    filters_view: Retained<RPTableView>,
    filters: Retained<RPTableSource>,
    /// The filter list's rows: (id, name, on).
    filter_rows: Rows<(FilterId, String, bool)>,
    /// The filter whose fields show.
    filter: Cell<Option<FilterId>>,
    pub(super) name: Retained<NSTextField>,
    pub(super) pattern: Retained<NSTextField>,
    not: Retained<NSTextField>,
    save: Retained<NSTextField>,
    save_browse: Retained<NSButton>,
    /// The Feed pop-up and its choices (`None`: All Feeds).
    feed: Retained<NSPopUpButton>,
    feed_choices: RefCell<Vec<Option<FeedId>>>,
    /// A pull-down: its first item is the button's text, then All and each quality.
    quality: Retained<NSPopUpButton>,
    episodes: Retained<NSTextField>,
    interval: Retained<NSPopUpButton>,
    pub(super) label: Retained<NSComboBox>,
    smart: Retained<NSButton>,
    priority: Retained<NSButton>,
    paused: Retained<NSButton>,
    filter_remove: Retained<NSButton>,
    filter_up: Retained<NSButton>,
    filter_down: Retained<NSButton>,
    preview: Retained<RPTableSource>,
    preview_rows: Rows<[String; 2]>,
    history: Retained<RPTableSource>,
    history_rows: Rows<[String; 3]>,
    pub(super) tabs: Retained<NSTabView>,
    /// Controls are being filled: their changes are not edits.
    loading: Cell<bool>,
}

thread_local! {
    static OPEN: RefCell<Weak<Dialog>> = const { RefCell::new(Weak::new()) };
}

/// The open window.
pub(super) fn current() -> Option<Rc<Dialog>> {
    OPEN.with(|o| o.borrow().upgrade())
}

/// The open window's `NSWindow`.
pub(super) fn ns_window() -> Option<Retained<NSWindow>> {
    current().map(|d| d.window.clone())
}

/// The downloader changed underneath (a fetch finished, a feed command): the open window
/// shows it again.
pub(super) fn refresh() {
    if let Some(d) = current() {
        d.refresh();
    }
}

/// A feed was added: the open window picks it.
pub(super) fn feed_added(id: FeedId) {
    if let Some(d) = current() {
        d.fill_feeds(Some(id));
    }
}

/// At least `h` points high, and the first to grow in its stack.
fn activate_height(v: &NSView, h: f64) {
    layout::activate(&[v.heightAnchor().constraintGreaterThanOrEqualToConstant(h)]);
    v.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
}

/// A column whose stretching view takes the height left.
fn filled(views: &[&NSView], mtm: MainThreadMarker) -> Retained<NSStackView> {
    let s = dialogs::column(views, mtm);
    s.setDistribution(NSStackViewDistribution::Fill);
    s
}

/// Column specs of `columns` (title, width, right-aligned).
fn specs(columns: &[(S, i32, bool)]) -> Vec<ColumnSpec> {
    columns
        .iter()
        .enumerate()
        .map(|(i, &(title, width, right))| {
            let mut s = ColumnSpec::text(&format!("c{i}"), &ui::text(lang::text(title)), f64::from(width));
            s.hideable = false;
            if right {
                s.align = table::Align::Right;
            }
            s
        })
        .collect()
}

/// A table of `specs` with `hooks`, in a scroll view at least `height` points high.
fn table_of(
    kind: &'static str,
    specs: Vec<ColumnSpec>,
    hooks: Hooks,
    height: f64,
    header: bool,
    mtm: MainThreadMarker,
) -> (Retained<NSScrollView>, Retained<RPTableView>, Retained<RPTableSource>) {
    let (scroll, view, source) = table::table(TableKind(kind), specs, hooks, None, mtm);
    view.setAllowsMultipleSelection(false);
    view.setAllowsColumnReordering(false);
    if !header {
        view.setHeaderView(None);
        view.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle);
        scroll.setHasHorizontalScroller(false);
    }
    activate_height(&scroll, height);
    (scroll, view, source)
}

/// Runs `f` on the open window (hooks of its tables).
fn with_open(f: impl FnOnce(&Rc<Dialog>)) {
    if let Some(d) = current() {
        f(&d);
    }
}

/// Options > RSS Downloader (and a Feeds menu's RSS Downloader, with `feed` picked): the
/// window, or the open one brought forward.
pub(super) fn show(feed: Option<FeedId>) {
    if let Some(d) = current() {
        d.window.makeKeyAndOrderFront(None);
        if feed.is_some() {
            d.store_fields();
            d.fill_feeds(feed);
        }
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else { return };
    let id = dialogs::new_id();
    let button = |t: &str, act: u8| ui::button_tr(t, cmd(id, act), mtm);
    let field = || {
        let f = ui::field("", "", mtm);
        if let Some(c) = f.cell() {
            c.setSendsActionOnEndEditing(true);
        }
        ui::target::wire(&f, cmd(id, STORE), mtm);
        layout::stretch(&f);
        f
    };

    // the Feeds list
    let feed_rows: Rows<(Option<FeedId>, String)> = Rc::default();
    let mut hooks = {
        let (n, c) = (feed_rows.clone(), feed_rows.clone());
        Hooks::flat(
            move || n.borrow().len(),
            move |row, _, _| {
                c.borrow().get(row as usize).map_or(CellValue::Empty, |r| CellValue::Text(r.1.clone()))
            },
        )
    };
    let ids = feed_rows.clone();
    hooks.id = Box::new(move |row| ids.borrow().get(row).map(|r| r.0.map_or(ALL_FEEDS, u64::from)));
    let ids = feed_rows.clone();
    hooks.rows_of = Box::new(move |want| {
        ids.borrow()
            .iter()
            .enumerate()
            .filter(|(_, r)| want.contains(&r.0.map_or(ALL_FEEDS, u64::from)))
            .map(|(i, _)| i)
            .collect()
    });
    hooks.selection = Some(Box::new(|_| with_open(|d| d.feed_selected())));
    hooks.activate = Some(Box::new(|_| with_open(|d| d.act(FEED_EDIT))));
    hooks.menu = Some(Box::new(|ids| {
        let mtm = MainThreadMarker::new()?;
        let feed = ids.first().and_then(|&i| FeedId::try_from(i).ok()).filter(|&i| u64::from(i) != ALL_FEEDS);
        with_open(|d| d.store_fields());
        Some(super::window_menu(feed, mtm))
    }));
    let (feeds_scroll, feeds_view, feeds) =
        table_of("rss feeds", vec![ColumnSpec::text("name", "", 200.0)], hooks, 160.0, false, mtm);
    let feeds_title = ui::label_tr(tr!(RssFeeds), mtm);
    let feed_status = ui::note("", mtm);
    let feed_add = button(tr!(RssFeedsAdd), FEED_ADD);
    let feed_edit = button(tr!(RssFeedsEdit), FEED_EDIT);
    let feed_remove = button(tr!(RssFeedsRemove), FEED_REMOVE);
    let feed_update = button(tr!(RssFeedsUpdate), FEED_UPDATE);
    let feed_buttons = layout::grid(
        &[vec![Some(&*feed_add), Some(&*feed_edit)], vec![Some(&*feed_remove), Some(&*feed_update)]],
        mtm,
    );
    for b in [&feed_add, &feed_edit, &feed_remove, &feed_update] {
        layout::stretch(b);
    }
    let left = filled(&[&feeds_title, &feeds_scroll, &feed_status, &feed_buttons], mtm);
    layout::width(&left, 230.0);

    // the filter list
    let filter_rows: Rows<(FilterId, String, bool)> = Rc::default();
    let mut hooks = {
        let (n, c) = (filter_rows.clone(), filter_rows.clone());
        Hooks::flat(
            move || n.borrow().len(),
            move |row, _, _| {
                c.borrow().get(row as usize).map_or(CellValue::Empty, |r| CellValue::Check(r.2, r.1.clone()))
            },
        )
    };
    let ids = filter_rows.clone();
    hooks.id = Box::new(move |row| ids.borrow().get(row).map(|r| u64::from(r.0)));
    let ids = filter_rows.clone();
    hooks.rows_of = Box::new(move |want| {
        ids.borrow()
            .iter()
            .enumerate()
            .filter(|(_, r)| want.contains(&u64::from(r.0)))
            .map(|(i, _)| i)
            .collect()
    });
    hooks.selection = Some(Box::new(|ids| {
        let id = ids.first().and_then(|&i| FilterId::try_from(i).ok());
        with_open(|d| d.filter_picked(id));
    }));
    hooks.check = Some(Box::new(|(id, on)| {
        let Ok(id) = FilterId::try_from(id) else { return };
        with_core(|c| {
            c.edit_rss(|d| {
                if let Some(f) = d.filter_mut(id) {
                    f.enabled = on;
                }
            })
        });
        with_open(|d| {
            if let Some(r) = d.filter_rows.borrow_mut().iter_mut().find(|r| r.0 == id) {
                r.2 = on;
            }
        });
    }));
    let mut check = ColumnSpec::text("name", "", 180.0);
    check.kind = CellKind::Check;
    let (filters_scroll, filters_view, filters) =
        table_of("rss filters", vec![check], hooks, 160.0, false, mtm);
    let filter_add = button(tr!(RssFilterAdd), FILTER_ADD);
    let filter_remove = button(tr!(RssFilterRemove), FILTER_REMOVE);
    let filter_up = button(tr!(RssFilterUp), FILTER_UP);
    let filter_down = button(tr!(RssFilterDown), FILTER_DOWN);
    for b in [&filter_add, &filter_remove, &filter_up, &filter_down] {
        layout::stretch(b);
    }
    let filter_buttons = layout::grid(
        &[vec![Some(&*filter_add), Some(&*filter_remove)], vec![Some(&*filter_up), Some(&*filter_down)]],
        mtm,
    );
    let filter_list = filled(&[&filters_scroll, &filter_buttons], mtm);
    layout::width(&filter_list, 200.0);

    // the filter's fields
    let (name, pattern, not, save, episodes) = (field(), field(), field(), field(), field());
    layout::min_width(&name, 240.0);
    let save_browse = button("...", BROWSE);
    let save_row = layout::hstack(&[&save, &save_browse], mtm);
    let feed_popup = ui::popup(&[], 0, cmd(id, STORE), mtm);
    let quality = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 24.0)),
        true,
    );
    ui::auto(&quality);
    quality.addItemWithTitle(&ns(&quality_text(Quality::ALL)));
    let names: Vec<String> = std::iter::once(tr!(RssQualityAll).to_owned())
        .chain(Quality::LIST.iter().map(|(_, n)| (*n).to_owned()))
        .collect();
    if let Some(menu) = quality.menu() {
        // the check marks are the dialog's, not `RPTarget`'s validation's
        menu.setAutoenablesItems(false);
        for (i, n) in names.iter().enumerate() {
            // SAFETY: no action yet; `wire` sets RPTarget's.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(n), None, &ns(""))
            };
            ui::target::wire(&item, cmd(id, QUALITY + i as u8), mtm);
            menu.addItem(&item);
        }
    }
    let intervals: Vec<String> = MIN_INTERVALS.iter().map(|(_, n)| ui::text(lang::text(*n))).collect();
    let interval = ui::popup(&intervals, 0, cmd(id, STORE), mtm);
    let label = NSComboBox::new(mtm);
    for l in peek(|c| c.labels()).unwrap_or_default() {
        // SAFETY: a combo box's items are strings here (no data source).
        unsafe { label.addItemWithObjectValue(&ns(&l)) };
    }
    label.setNumberOfVisibleItems(10);
    label.setCompletes(true);
    if let Some(c) = label.cell() {
        c.setSendsActionOnEndEditing(true);
    }
    ui::target::wire(&label, cmd(id, STORE), mtm);
    ui::auto(&label);
    layout::width(&episodes, 90.0);
    let example = ui::note(&ui::text(tr!(RssEpisodesExample)), mtm);
    let episodes_row = layout::hstack(&[&episodes, &example], mtm);
    let caption = |t: &str| ui::label_tr(t, mtm);
    let captions = [
        caption(tr!(RssName)),
        caption(tr!(RssFilter)),
        caption(tr!(RssNot)),
        caption(tr!(RssSaveIn)),
        caption(tr!(RssFeed)),
        caption(tr!(RssQuality)),
        caption(tr!(RssEpisodes)),
        caption(tr!(RssMinInterval)),
        caption(tr!(RssLabel)),
    ];
    let controls: [&NSView; 9] =
        [&name, &pattern, &not, &save_row, &feed_popup, &quality, &episodes_row, &interval, &label];
    let rows: Vec<Vec<Option<&NSView>>> =
        captions.iter().zip(controls).map(|(c, v)| vec![Some(&**c as &NSView), Some(v)]).collect();
    let fields = layout::grid(&rows, mtm);
    layout::label_column(&fields);
    let check = |t: &str| ui::check(&ui::text(t), false, cmd(id, STORE), mtm);
    let (smart, priority, paused) =
        (check(tr!(RssSmart)), check(tr!(RssHighPriority)), check(tr!(RssPaused)));
    let checks = layout::vstack(&[&smart, &priority, &paused], mtm);
    let matching = ui::label_tr(tr!(RssMatching), mtm);

    // the preview and the history
    let preview_rows: Rows<[String; 2]> = Rc::default();
    let (n, c) = (preview_rows.clone(), preview_rows.clone());
    let hooks = Hooks::flat(
        move || n.borrow().len(),
        move |row, i, _| {
            c.borrow()
                .get(row as usize)
                .and_then(|r| r.get(i).cloned())
                .map_or(CellValue::Empty, CellValue::Text)
        },
    );
    let (preview_scroll, _, preview) = table_of(
        "rss preview",
        specs(&[(S::ColName, 220, false), (S::ColFeed, 120, false)]),
        hooks,
        100.0,
        true,
        mtm,
    );
    let right = filled(&[&fields, &checks, &matching, &preview_scroll], mtm);
    let filters_page = layout::hstack(&[&filter_list, &right], mtm);
    filters_page.setAlignment(NSLayoutAttribute::Top);
    filters_page.setSpacing(12.0);
    layout::activate(&[
        filter_list.heightAnchor().constraintEqualToAnchor(&filters_page.heightAnchor()),
        right.heightAnchor().constraintEqualToAnchor(&filters_page.heightAnchor()),
    ]);

    let history_rows: Rows<[String; 3]> = Rc::default();
    let (n, c) = (history_rows.clone(), history_rows.clone());
    let hooks = Hooks::flat(
        move || n.borrow().len(),
        move |row, i, _| {
            c.borrow()
                .get(row as usize)
                .and_then(|r| r.get(i).cloned())
                .map_or(CellValue::Empty, CellValue::Text)
        },
    );
    let (history_scroll, _, history) =
        table_of("rss history", specs(&view::HISTORY_COLUMNS), hooks, 200.0, true, mtm);
    let clear = button(tr!(RssClearHistory), CLEAR_HISTORY);
    let clear_row = layout::hstack(&[&layout::spacer(mtm), &clear], mtm);
    let history_page = filled(&[&history_scroll, &clear_row], mtm);

    let tabs = NSTabView::new(mtm);
    ui::auto(&tabs);
    for (title, page) in [(tr!(RssTabFilters), &*filters_page), (tr!(RssTabHistory), &*history_page)] {
        let item = NSTabViewItem::new();
        item.setLabel(&ui::nstext(title));
        // the page in a plain view the tab view sizes, at the tab's margins
        let host = NSView::new(mtm);
        host.addSubview(page);
        layout::pin(page, &host, 12.0);
        item.setView(Some(&host));
        tabs.addTabViewItem(&item);
    }
    layout::stretch(&tabs);
    let panes = layout::hstack(&[&left, &tabs], mtm);
    panes.setAlignment(NSLayoutAttribute::Top);
    layout::activate(&[
        left.heightAnchor().constraintEqualToAnchor(&panes.heightAnchor()),
        tabs.heightAnchor().constraintEqualToAnchor(&panes.heightAnchor()),
    ]);
    activate_height(&panes, 400.0);
    let close = button(tr!(BtnClose), CLOSE);
    close.setKeyEquivalent(&ns("\u{1b}"));
    let bottom = layout::hstack(&[&layout::spacer(mtm), &close], mtm);
    let root = filled(&[&panes, &bottom], mtm);
    root.setSpacing(12.0);
    let window = dialogs::window(&ui::text(tr!(RssTitle)), &root, Some(NSSize::new(940.0, 620.0)), true, mtm);

    let d = Rc::new(Dialog {
        id,
        window: window.clone(),
        observer: RefCell::new(None),
        feeds_view,
        feeds,
        feed_rows,
        feed_status,
        feed_edit,
        feed_remove,
        filters_view,
        filters,
        filter_rows,
        filter: Cell::new(None),
        name,
        pattern,
        not,
        save,
        save_browse,
        feed: feed_popup,
        feed_choices: RefCell::default(),
        quality,
        episodes,
        interval,
        label,
        smart,
        priority,
        paused,
        filter_remove,
        filter_up,
        filter_down,
        preview,
        preview_rows,
        history,
        history_rows,
        tabs,
        loading: Cell::new(false),
    });
    *d.observer.borrow_mut() = Some(dialogs::on_close(&window, id, CLOSED));
    let handler = d.clone();
    dialogs::set_handler(id, move |act| handler.act(act));
    OPEN.with(|o| *o.borrow_mut() = Rc::downgrade(&d));
    d.fill_feeds(feed);
    d.fill_filters(None, true);
    d.fill_history();
    window.setInitialFirstResponder(Some(&d.feeds_view));
    window.center();
    window.makeKeyAndOrderFront(None);
    super::tour::opened();
}

impl Dialog {
    pub(super) fn act(self: &Rc<Self>, act: u8) {
        // a field being typed in counts before anything else happens
        if act != STORE && act != CLOSED {
            self.store_fields();
        }
        match act {
            CLOSE => self.window.close(),
            CLOSED => self.closed(),
            FEED_ADD => super::add_feed(Some(self.window.clone())),
            FEED_EDIT => {
                if let Some(id) = self.selected_feed() {
                    super::edit_feed(Some(self.window.clone()), id);
                }
            }
            FEED_REMOVE => {
                if let Some(id) = self.selected_feed() {
                    super::delete_feed(Some(self.window.clone()), id);
                }
            }
            FEED_UPDATE => super::update_feed(self.selected_feed()),
            FILTER_ADD => self.add_filter(),
            FILTER_REMOVE => self.remove_filter(),
            FILTER_UP => self.move_filter(true),
            FILTER_DOWN => self.move_filter(false),
            BROWSE => self.pick_save_dir(),
            STORE => self.store_fields(),
            CLEAR_HISTORY => {
                with_core(|c| c.edit_rss(|d| d.history.clear()));
                self.fill_history();
            }
            a if a >= QUALITY => self.toggle_quality(usize::from(a - QUALITY)),
            _ => {}
        }
    }

    /// Everything again after the downloader changed underneath; the fields being edited stay.
    fn refresh(&self) {
        self.store_fields();
        self.fill_feeds(self.selected_feed());
        self.fill_filters(self.filter.get(), false);
        self.fill_preview();
        self.fill_history();
    }

    /// The feed picked in the Feeds list (`None`: All Feeds or nothing).
    pub(super) fn selected_feed(&self) -> Option<FeedId> {
        let id = *self.feeds.selected().first()?;
        if id == ALL_FEEDS { None } else { FeedId::try_from(id).ok() }
    }

    /// The Feeds list (All Feeds, then each feed) with `select` picked, the Feed pop-up's
    /// choices, and the state line.
    pub(super) fn fill_feeds(&self, select: Option<FeedId>) {
        let Some(feeds) =
            peek(|c| c.rss().feeds.iter().map(|f| (f.id, f.name().to_owned())).collect::<Vec<_>>())
        else {
            return;
        };
        let mut rows = vec![(None, ui::text(tr!(RssAllFeeds)))];
        rows.extend(feeds.iter().map(|(id, n)| (Some(*id), n.clone())));
        let pick = if rows.iter().any(|r| r.0 == select) { select } else { None };
        *self.feed_rows.borrow_mut() = rows.clone();
        self.loading.set(true);
        self.feeds.reload_keep_selection();
        self.feeds.select(&[pick.map_or(ALL_FEEDS, u64::from)]);
        // the Feed pop-up keeps its choice
        let current =
            ui::chosen(&self.feed).and_then(|i| self.feed_choices.borrow().get(i).copied()).flatten();
        if let Some(menu) = self.feed.menu() {
            menu.removeAllItems();
            if let Some(mtm) = MainThreadMarker::new() {
                for (_, name) in &rows {
                    // SAFETY: no action: the pop-up's own runs on a choice.
                    let item = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            NSMenuItem::alloc(mtm),
                            &ns(name),
                            None,
                            &ns(""),
                        )
                    };
                    menu.addItem(&item);
                }
            }
        }
        let choices: Vec<Option<FeedId>> = rows.iter().map(|r| r.0).collect();
        let at = choices.iter().position(|c| *c == current).unwrap_or(0);
        *self.feed_choices.borrow_mut() = choices;
        self.feed.selectItemAtIndex(at as isize);
        self.loading.set(false);
        self.feed_selected();
    }

    /// The state line and the buttons for the feed picked in the list.
    fn feed_selected(&self) {
        if self.loading.get() {
            return;
        }
        let feed = self.selected_feed();
        let text = match feed {
            Some(id) => peek(|c| c.rss().feed(id).map(|f| view::feed_status(f, c.feed_updating(id))))
                .flatten()
                .unwrap_or_default(),
            None => tr!(RssFeedCount, count = peek(|c| c.rss().feeds.len()).unwrap_or(0)),
        };
        self.feed_status.setStringValue(&ns(&ui::text(&text)));
        self.feed_edit.setEnabled(feed.is_some());
        self.feed_remove.setEnabled(feed.is_some());
    }

    /// The filter list with `select` picked (else the first filter); its fields too with
    /// `show`.
    fn fill_filters(&self, select: Option<FilterId>, show: bool) {
        let Some(rows) =
            peek(|c| c.rss().filters.iter().map(|f| (f.id, f.name.clone(), f.enabled)).collect::<Vec<_>>())
        else {
            return;
        };
        let pick = select.filter(|s| rows.iter().any(|r| r.0 == *s)).or(rows.first().map(|r| r.0));
        *self.filter_rows.borrow_mut() = rows;
        self.loading.set(true);
        self.filters.reload_keep_selection();
        self.filters.select(&pick.map(u64::from).into_iter().collect::<Vec<_>>());
        if let Some(row) = self
            .filters
            .selected()
            .first()
            .and_then(|&id| self.filter_rows.borrow().iter().position(|r| u64::from(r.0) == id))
        {
            self.filters_view.scrollRowToVisible(row as isize);
        }
        self.loading.set(false);
        if show || pick != self.filter.get() {
            self.show_filter(pick);
        }
    }

    /// The filter list's selection changed (the user's click).
    fn filter_picked(&self, id: Option<FilterId>) {
        if self.loading.get() || id.is_none() || id == self.filter.get() {
            return;
        }
        self.store_fields();
        self.show_filter(id);
    }

    /// Writes filter `id`'s fields into the controls (empty and greyed without one).
    fn show_filter(&self, id: Option<FilterId>) {
        self.filter.set(id);
        let f = id.and_then(|id| peek(|c| c.rss().filter(id).cloned()).flatten());
        let f2 = f.clone().unwrap_or_else(|| RssFilter { name: String::new(), ..RssFilter::default() });
        self.loading.set(true);
        for (field, text) in [
            (&self.name, &f2.name),
            (&self.pattern, &f2.pattern),
            (&self.not, &f2.not),
            (&self.save, &f2.save_in),
            (&self.episodes, &f2.episodes),
        ] {
            field.setStringValue(&ns(text));
        }
        self.label.setStringValue(&ns(&f2.label));
        let feed = self.feed_choices.borrow().iter().position(|&c| c == f2.feed).unwrap_or(0);
        self.feed.selectItemAtIndex(feed as isize);
        let interval = MIN_INTERVALS.iter().position(|&(s, _)| s == f2.min_interval);
        self.interval.selectItemAtIndex(interval.map_or(-1, |i| i as isize));
        self.show_quality(f2.quality);
        ui::set_checked(&self.smart, f2.smart);
        ui::set_checked(&self.priority, f2.top_priority);
        ui::set_checked(&self.paused, f2.paused);
        let on = f.is_some();
        for f in [&self.name, &self.pattern, &self.not, &self.save, &self.episodes] {
            f.setEnabled(on);
        }
        self.label.setEnabled(on);
        for p in [&self.feed, &self.quality, &self.interval] {
            p.setEnabled(on);
        }
        for b in [
            &self.save_browse,
            &self.smart,
            &self.priority,
            &self.paused,
            &self.filter_remove,
            &self.filter_up,
            &self.filter_down,
        ] {
            b.setEnabled(on);
        }
        self.loading.set(false);
        self.fill_preview();
    }

    /// The Quality button's text and its menu's check marks.
    fn show_quality(&self, q: Quality) {
        if let Some(first) = self.quality.itemAtIndex(0) {
            first.setTitle(&ns(&quality_text(q)));
        }
        for i in 0..=Quality::LIST.len() {
            let Some(item) = self.quality.itemAtIndex(i as isize + 1) else { continue };
            let on = match i {
                0 => q.is_all(),
                i => !q.is_all() && q.contains(Quality::LIST[i - 1].0),
            };
            item.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        }
    }

    /// The items of the feeds' last fetches the filter matches on its fields.
    fn fill_preview(&self) {
        let rows = self
            .filter
            .get()
            .and_then(|id| {
                peek(|c| {
                    let d = c.rss();
                    let Some(f) = d.filter(id) else { return Vec::new() };
                    d.matching_items(f)
                        .take(PREVIEW_ROWS)
                        .map(|(feed, i)| {
                            [i.title.clone(), d.feed(feed).map(|f| f.name().to_owned()).unwrap_or_default()]
                        })
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        *self.preview_rows.borrow_mut() = rows;
        self.preview.reload_keep_selection();
    }

    fn fill_history(&self) {
        let rows = peek(|c| view::history_rows(c.rss())).unwrap_or_default();
        *self.history_rows.borrow_mut() = rows;
        self.history.reload_keep_selection();
    }

    /// Stores the fields into the picked filter when they changed; its row and the preview
    /// follow.
    pub(super) fn store_fields(&self) {
        if self.loading.get() {
            return;
        }
        let Some(id) = self.filter.get() else { return };
        let Some(old) = peek(|c| c.rss().filter(id).cloned()).flatten() else { return };
        let mut f = old.clone();
        f.name = ui::value(&self.name);
        f.pattern = ui::value(&self.pattern);
        f.not = ui::value(&self.not);
        f.save_in = ui::value(&self.save).trim().to_owned();
        f.feed = ui::chosen(&self.feed).and_then(|i| self.feed_choices.borrow().get(i).copied()).flatten();
        f.episodes = ui::value(&self.episodes);
        if let Some(&(s, _)) = ui::chosen(&self.interval).and_then(|i| MIN_INTERVALS.get(i)) {
            f.min_interval = s;
        }
        f.label = self.label.stringValue().to_string().trim().to_owned();
        f.smart = ui::checked(&self.smart);
        f.top_priority = ui::checked(&self.priority);
        f.paused = ui::checked(&self.paused);
        if f == old {
            return;
        }
        let renamed = f.name != old.name;
        with_core(|c| {
            c.edit_rss(|d| {
                if let Some(slot) = d.filter_mut(id) {
                    *slot = f;
                }
            })
        });
        if renamed {
            self.fill_filters(Some(id), false);
        }
        self.fill_preview();
    }

    /// A Quality menu entry: 0 is All, `1 + i` is `Quality::LIST[i]`.
    fn toggle_quality(&self, choice: usize) {
        let Some(id) = self.filter.get() else { return };
        let q = with_core(|c| {
            let mut q = Quality::ALL;
            c.edit_rss(|d| {
                let Some(f) = d.filter_mut(id) else { return };
                f.quality = match choice {
                    0 => Quality::ALL,
                    // All is no bit set: the first pick is that quality alone
                    c => match Quality::LIST.get(c - 1) {
                        Some(&(one, _)) if f.quality.contains(one) => Quality(f.quality.0 & !one.0),
                        Some(&(one, _)) => f.quality.union(one),
                        None => f.quality,
                    },
                };
                q = f.quality;
            });
            q
        });
        if let Some(q) = q {
            self.show_quality(q);
        }
        self.fill_preview();
    }

    fn add_filter(&self) {
        // µTorrent bound a new filter to the feed picked on the left
        let feed = self.selected_feed();
        let Some(id) = with_core(|c| {
            let mut id = 0;
            c.edit_rss(|d| id = d.add_filter(RssFilter { feed, ..RssFilter::default() }));
            id
        }) else {
            return;
        };
        self.fill_filters(Some(id), true);
        self.window.makeFirstResponder(Some(&self.name));
    }

    fn remove_filter(&self) {
        let Some(filter) = self.filter.get() else { return };
        let rows: Vec<FilterId> = self.filter_rows.borrow().iter().map(|r| r.0).collect();
        let row = rows.iter().position(|&f| f == filter).unwrap_or(0);
        with_core(|c| c.edit_rss(|d| d.remove_filter(filter)));
        // the one that took its place, else the one above
        let rest: Vec<FilterId> = rows.into_iter().filter(|&f| f != filter).collect();
        self.filter.set(None);
        self.fill_filters(rest.get(row).or(rest.last()).copied(), true);
    }

    fn move_filter(&self, up: bool) {
        let Some(filter) = self.filter.get() else { return };
        with_core(|c| c.edit_rss(|d| d.move_filter(filter, up)));
        self.fill_filters(Some(filter), false);
    }

    fn pick_save_dir(self: &Rc<Self>) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let now = PathBuf::from(ui::value(&self.save).trim());
        let pick = ui::Pick {
            folders: true,
            start: now.is_dir().then_some(now),
            message: ui::text(tr!(RssPickSaveDir)),
            ..ui::Pick::default()
        };
        let me = Rc::downgrade(self);
        dialogs::open_panel(
            Some(&self.window),
            pick,
            move |paths| {
                if let (Some(d), Some(p)) = (me.upgrade(), paths.first()) {
                    d.save.setStringValue(&ns(&p.to_string_lossy()));
                    d.store_fields();
                }
            },
            mtm,
        );
    }

    /// The window closed: the filters run over the feeds' items.
    fn closed(&self) {
        self.store_fields();
        if let Some(o) = self.observer.borrow_mut().take() {
            dialogs::forget(&o);
        }
        OPEN.with(|o| *o.borrow_mut() = Weak::new());
        dialogs::retire(self.id);
        apply(with_core(|c| {
            c.rss_closed();
            c.events()
        }));
    }
}
