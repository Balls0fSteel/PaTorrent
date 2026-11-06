//! The Search Results window (GTK's Search Torrents, Win32's Search Results; one, not modal): the
//! query, a category and a source to search again, Search and Stop, each source's status line,
//! the merged results (Name, Size, Seeds, Peers, Category, Age, Source; a click on a header
//! sorts), the count, the last Add's outcome, Add (a double click or Return on a row too), Manage
//! Sources and Close.
//!
//! One search runs at a time (`frontend::search::Search`): its worker threads wake the main
//! thread through `mainq::on_main` ([`collected`]), which merges their answers and redraws the
//! window. Adding a result fetches it on a thread (`fetch_download`: its magnet, or the
//! `.torrent` behind its link, checked against the result's info-hash) and hands it to
//! `Core::add_download` and `dialogs::add::handle`, as an opened file or link. A result whose
//! info-hash is in the list already says so instead. Closing the window stops the search.
//!
//! Its controls run through `RPTarget` as the torrent dialogs' do (`dialogs::cmd`, a handler).

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSColor, NSLayoutConstraintOrientation, NSMenu, NSMenuItem, NSPopUpButton,
    NSStackViewDistribution, NSTextField, NSView, NSWindow,
};
use objc2_foundation::NSSize;
use rookparcel_core::util::time::unix_now;
use rookparcel_frontend::app::AddStep;
use rookparcel_frontend::rss::Download;
use rookparcel_frontend::search::view::{
    Col, Mode, category_choice, category_names, cells, picked, query_text, result_key, sorted,
    source_choices, status_line,
};
use rookparcel_frontend::search::{Query, Search, SearchResult, Source, fetch_download, redact};
use rookparcel_frontend::tr;

use crate::app::{peek, with_core};
use crate::dialogs::{self, NOTHING, Observer, cmd};
use crate::table::{self, Align, CellValue, ColumnSpec, Hooks, Key, RPTableSource, RPTableView, TableKind};
use crate::ui::{self, layout, ns};

const GO: u8 = 1;
const STOP: u8 = 2;
const ADD: u8 = 3;
const MANAGE: u8 = 4;
const CLOSE: u8 = 5;
/// The window closed.
const CLOSED: u8 = 6;

/// The rows as shown, for the table's data source.
#[derive(Default)]
struct Rows {
    cells: Vec<[String; 7]>,
    /// The rows' results with the source each came from (to add it).
    shown: Vec<(SearchResult, Source)>,
    /// The rows' ids: a hash of [`result_key`], so the selection follows a result as rows move.
    ids: Vec<u64>,
    sort: Option<(Col, bool)>,
}

pub struct Results {
    id: u16,
    pub window: Retained<NSWindow>,
    query: Retained<NSTextField>,
    category: Retained<NSPopUpButton>,
    source: Retained<NSPopUpButton>,
    /// The source list's choices: "All sources", then each enabled source.
    modes: RefCell<Vec<Mode>>,
    stop: Retained<NSButton>,
    add: Retained<NSButton>,
    status: Retained<NSTextField>,
    count: Retained<NSTextField>,
    message: Retained<NSTextField>,
    /// A message instead of the status lines (nothing to search).
    notice: RefCell<String>,
    pub table: Retained<RPTableView>,
    pub rows: Retained<RPTableSource>,
    observer: RefCell<Option<Observer>>,
}

thread_local! {
    static SEARCH: RefCell<Option<Search>> = const { RefCell::new(None) };
    static OPEN: RefCell<Option<Rc<Results>>> = const { RefCell::new(None) };
    static ROWS: RefCell<Rows> = RefCell::new(Rows::default());
}

fn with_search<R>(f: impl FnOnce(&mut Search) -> R) -> R {
    SEARCH.with(|s| {
        let mut s = s.borrow_mut();
        // the worker threads only wake the main thread; `collected` takes their answers there
        f(s.get_or_insert_with(|| Search::new(|| crate::mainq::on_main(collected))))
    })
}

/// The open window.
pub fn open() -> Option<Rc<Results>> {
    OPEN.with(|o| o.borrow().clone())
}

/// Answers came in: merged here, and the open window shows them.
fn collected() {
    if with_search(Search::collect) {
        if let Some(r) = open() {
            r.refresh();
        }
    }
    super::tour::collected();
}

/// Whether the search still waits for a source.
pub fn running() -> bool {
    SEARCH.with(|s| s.borrow().as_ref().is_some_and(Search::is_running))
}

/// Stops the search (quit, the window closing).
pub fn stop() {
    SEARCH.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut() {
            s.stop();
        }
    });
}

/// The search field's Return in a source mode: searches `mode`'s sources for `text` in the
/// window (the open one, else a new one), which comes forward.
pub fn search(text: &str, mode: &Mode) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let r = open().unwrap_or_else(|| build(mtm));
    r.query.setStringValue(&ns(text.trim()));
    r.fill_sources(mode, mtm);
    r.category.selectItemAtIndex(0);
    r.start();
    r.window.makeKeyAndOrderFront(None);
}

/// The sources changed: the open window's source list follows.
pub fn sources_changed() {
    let (Some(r), Some(mtm)) = (open(), MainThreadMarker::new()) else { return };
    let m = r.mode();
    r.fill_sources(&m, mtm);
}

fn row_id(r: &SearchResult) -> u64 {
    let mut h = DefaultHasher::new();
    result_key(r).hash(&mut h);
    h.finish()
}

/// The rows again from the search, in the chosen order.
fn recompute() {
    let now = unix_now();
    let sort = ROWS.with(|r| r.borrow().sort);
    let rows = with_search(|s| {
        let names: Vec<String> = s.sources().iter().map(|x| x.source.label()).collect();
        let results = s.results();
        let order = sorted(results, &names, sort);
        let mut rows = Rows { sort, ..Rows::default() };
        for &i in &order {
            let r = &results[i];
            let Some(src) = s.source_of(r) else { continue };
            rows.cells.push(cells(r, &names, now));
            rows.ids.push(row_id(r));
            rows.shown.push((r.clone(), src.clone()));
        }
        rows
    });
    ROWS.with(|r| *r.borrow_mut() = rows);
}

fn cell(row: u64, col: usize) -> CellValue {
    ROWS.with(|r| {
        r.borrow()
            .cells
            .get(row as usize)
            .and_then(|c| c.get(col))
            .map_or(CellValue::Empty, |t| CellValue::Text(t.clone()))
    })
}

fn hooks() -> Hooks {
    let mut h = Hooks::flat(|| ROWS.with(|r| r.borrow().cells.len()), |row, col, _dark| cell(row, col));
    h.id = Box::new(|row| ROWS.with(|r| r.borrow().ids.get(row).copied()));
    h.rows_of = Box::new(|ids| {
        ROWS.with(|r| {
            let r = r.borrow();
            r.ids.iter().enumerate().filter(|(_, id)| ids.contains(id)).map(|(i, _)| i).collect()
        })
    });
    h.sort = Some(Box::new(|(i, ascending)| {
        let col = Col::ALL.get(i).copied().unwrap_or(Col::Name);
        ROWS.with(|r| r.borrow_mut().sort = Some((col, !ascending)));
        recompute();
    }));
    h.selection = Some(Box::new(|ids| {
        if let Some(r) = open() {
            r.add.setEnabled(!ids.is_empty());
        }
    }));
    h.activate = Some(Box::new(|id| add_ids(&[id])));
    h.key = Some(Box::new(|key, ids| {
        if key != Key::Return || ids.is_empty() {
            return false;
        }
        add_ids(ids);
        true
    }));
    h
}

fn specs() -> Vec<ColumnSpec> {
    Col::ALL
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let mut s = ColumnSpec::text(&format!("search.{i}"), &ui::text(c.title()), c.width());
            if c.right_aligned() {
                s.align = Align::Right;
            }
            s.sortable = true;
            s.descending_first = c.first_descending();
            s.hideable = c != Col::Name;
            s
        })
        .collect()
}

fn build(mtm: MainThreadMarker) -> Rc<Results> {
    let id = dialogs::new_id();
    let query = ui::field("", &ui::text(tr!(SearchQueryCue)), mtm);
    layout::stretch(&query);
    layout::min_width(&query, 200.0);
    let category = ui::popup(&category_names(), 0, cmd(id, NOTHING), mtm);
    let source = ui::popup(&[], 0, cmd(id, NOTHING), mtm);
    let go = ui::button_tr(tr!(WebSearch), cmd(id, GO), mtm);
    // Return in the query searches
    go.setKeyEquivalent(&ns("\r"));
    let stop = ui::button_tr(tr!(ToolStop), cmd(id, STOP), mtm);
    let bar = layout::hstack(&[&query, &category, &source, &go, &stop], mtm);

    let status = ui::wrapping_label("", mtm);
    status.setSelectable(true);
    status.setHidden(true);
    let (scroll, table, rows) = table::table(TableKind("search results"), specs(), hooks(), None, mtm);
    layout::min_width(&scroll, 560.0);
    layout::activate(&[scroll.heightAnchor().constraintGreaterThanOrEqualToConstant(200.0)]);
    scroll.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);

    let count = ui::label("", mtm);
    let message = ui::truncating_label("", mtm);
    layout::stretch(&message);
    message.setContentCompressionResistancePriority_forOrientation(
        1.0,
        NSLayoutConstraintOrientation::Horizontal,
    );
    let add = ui::button_tr(tr!(WebAdd), cmd(id, ADD), mtm);
    let manage = ui::button_tr(tr!(SearchManage), cmd(id, MANAGE), mtm);
    let close = ui::button_tr(tr!(BtnClose), cmd(id, CLOSE), mtm);
    close.setKeyEquivalent(&ns("\u{1b}"));
    let foot = layout::hstack(&[&count, &message, &add, &manage, &close], mtm);

    let views: [&NSView; 4] = [&bar, &status, &scroll, &foot];
    let root = dialogs::column(&views, mtm);
    root.setDistribution(NSStackViewDistribution::Fill);
    let window =
        dialogs::window(&ui::text(tr!(SearchTitle)), &root, Some(NSSize::new(900.0, 560.0)), true, mtm);
    dialogs::focus_first(&window, &query);
    let r = Rc::new(Results {
        id,
        window: window.clone(),
        query,
        category,
        source,
        modes: RefCell::new(Vec::new()),
        stop,
        add,
        status,
        count,
        message,
        notice: RefCell::new(String::new()),
        table,
        rows,
        observer: RefCell::new(None),
    });
    *r.observer.borrow_mut() = Some(dialogs::on_close(&window, id, CLOSED));
    let handler = r.clone();
    dialogs::set_handler(id, move |act| handler.act(act));
    OPEN.with(|o| *o.borrow_mut() = Some(r.clone()));
    r.add.setEnabled(false);
    window.center();
    r
}

/// A pop-up's items (several may have the same text, which `addItemWithTitle:` would merge).
fn set_items(p: &NSPopUpButton, titles: &[String], selected: usize, mtm: MainThreadMarker) {
    let menu = NSMenu::new(mtm);
    for t in titles {
        // SAFETY: no action: the pop-up's own target and action take the choice.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(t), None, &ns(""))
        };
        menu.addItem(&item);
    }
    p.setMenu(Some(&menu));
    if selected < titles.len() {
        p.selectItemAtIndex(selected as isize);
    }
}

impl Results {
    fn act(self: &Rc<Self>, act: u8) {
        match act {
            GO => self.start(),
            STOP => {
                stop();
                self.refresh();
            }
            ADD => add_ids(&self.rows.selected()),
            MANAGE => crate::search_helper::manage_sources(),
            CLOSE => self.window.close(),
            CLOSED => self.closed(),
            _ => {}
        }
    }

    /// The window closed: nobody looks at the answers any more.
    fn closed(&self) {
        stop();
        if let Some(o) = self.observer.borrow_mut().take() {
            dialogs::forget(&o);
        }
        OPEN.with(|o| o.borrow_mut().take());
        dialogs::retire(self.id);
    }

    /// The source picked in the window's list.
    fn mode(&self) -> Mode {
        let i = ui::chosen(&self.source).unwrap_or(0);
        self.modes.borrow().get(i).cloned().unwrap_or(Mode::All)
    }

    /// Fills the source list ("All sources", then each enabled source) with `mode` picked.
    fn fill_sources(&self, mode: &Mode, mtm: MainThreadMarker) {
        let choices = source_choices(&super::stored_sources(), mode);
        let at = choices.iter().position(|(m, _)| m == mode).unwrap_or(0);
        let titles: Vec<String> = choices.iter().map(|(_, t)| t.clone()).collect();
        *self.modes.borrow_mut() = choices.into_iter().map(|(m, _)| m).collect();
        set_items(&self.source, &titles, at, mtm);
    }

    /// Search (or Return in the query): the query, the category and the source picked.
    fn start(&self) {
        let text = ui::value(&self.query);
        if text.trim().is_empty() {
            self.window.makeFirstResponder(Some(&self.query));
            return;
        }
        let categories = category_choice(ui::chosen(&self.category).unwrap_or(0));
        let notice = match picked(&self.mode(), &super::stored_sources()) {
            None => {
                stop();
                tr!(SearchNoSources).to_owned()
            }
            Some(list) => {
                // the search helper's source starts it: it needs the folder and the proxy
                super::configure_helper();
                let proxy = peek(|c| c.proxy()).flatten();
                with_search(|s| {
                    s.start(&list, Query { text: query_text(&text), categories }, proxy.as_ref())
                });
                String::new()
            }
        };
        *self.notice.borrow_mut() = notice;
        self.say("", false);
        self.rows.select(&[]);
        self.refresh();
    }

    /// The rows, the status lines and the buttons again from the search.
    pub fn refresh(&self) {
        recompute();
        let (lines, total, running) = with_search(|s| {
            let lines: Vec<String> = s.sources().iter().map(status_line).collect();
            (lines, s.results().len(), s.is_running())
        });
        self.rows.reload_keep_selection();
        let notice = self.notice.borrow().clone();
        let status = if notice.is_empty() { lines.join("\n") } else { notice };
        self.status.setStringValue(&ns(&status));
        self.status.setHidden(status.is_empty());
        self.count.setStringValue(&ns(&tr!(SearchResultCount, count = total)));
        self.stop.setEnabled(running);
        self.add.setEnabled(!self.rows.selected().is_empty());
    }

    /// The line beside the buttons (red for a failure).
    fn say(&self, text: &str, failed: bool) {
        self.message.setStringValue(&ns(text));
        ui::tooltip(&self.message, text);
        let color = if failed { NSColor::systemRedColor() } else { NSColor::labelColor() };
        self.message.setTextColor(Some(&color));
    }
}

/// Add with the open window's selection (the button's, for the test tour).
pub fn add_selected() {
    if let Some(r) = open() {
        add_ids(&r.rows.selected());
    }
}

/// Add (the button, a double click, Return): the results of these row ids.
fn add_ids(ids: &[u64]) {
    let picked: Vec<(SearchResult, Source)> = ROWS.with(|r| {
        let r = r.borrow();
        r.ids.iter().zip(&r.shown).filter(|(id, _)| ids.contains(id)).map(|(_, s)| s.clone()).collect()
    });
    for (result, source) in picked {
        add_result(result, source);
    }
}

fn say(text: &str, failed: bool) {
    if let Some(r) = open() {
        r.say(text, failed);
    }
}

/// Fetches what `result` adds on a thread (its magnet, else the `.torrent` or magnet behind its
/// link), then adds it as an opened file or link is.
fn add_result(result: SearchResult, source: Source) {
    if !result.info_hash.is_empty() {
        if let Some(name) = peek(|c| c.in_list(&result.info_hash.to_ascii_lowercase())).flatten() {
            say(&tr!(InfoAlreadyInList, name), true);
            super::tour::added();
            return;
        }
    }
    let proxy = peek(|c| c.proxy()).flatten();
    std::thread::spawn(move || {
        let (name, label, key) = (result.name.clone(), source.label(), source.api_key.clone());
        let fetched = fetch_download(&result, &source, proxy.as_ref());
        crate::mainq::on_main(move || fetched_one(name, label, key, fetched));
    });
}

fn fetched_one(name: String, label: String, key: String, fetched: Result<Download, String>) {
    match fetched {
        Ok(d) => match with_core(|c| c.add_download(d)) {
            Some(AddStep::AlreadyInList(n)) => say(&tr!(InfoAlreadyInList, name = n), true),
            Some(AddStep::Error(message, _)) => say(&message, true),
            Some(step) => {
                crate::dialogs::add::handle(step);
                say(&tr!(SearchAdded, name), false);
            }
            None => {}
        },
        Err(e) => say(&tr!(SearchFailed, source = label, error = redact(&e, &key)), true),
    }
    super::tour::added();
}
