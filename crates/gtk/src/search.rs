//! Torrent search through the user's Prowlarr or Jackett over Torznab (D41,
//! `docs/INTEGRATIONS.md` part 2, slice 4), on [`patorrent_frontend::search`] as the Win32
//! client's and the Web UI's.
//!
//! The toolbar's search box keeps filtering the torrent list by name; the menu beside it
//! ([`toolbar_menu`]) picks what Enter does instead: "Search torrent names" (the filter, the
//! default), one source, or "All sources", and "Manage Sources...". With a source picked,
//! Enter starts a search and opens the Search Torrents window ([`search`]): the query, a
//! category and a source to search again, each source's status line, the merged results
//! (Name, Size, Seeds, Peers, Category, Age, Source; a click on a header sorts), Stop, and
//! Add (a double click or Enter on a row too). Adding fetches the result on GIO's thread pool
//! ([`fetch_download`]) and hands it to the owner's usual add path ([`Host::add`]), which
//! shows the Add dialog unless that is turned off.
//!
//! The Search Sources window ([`show_sources`]) lists the sources with a check box each, and
//! has Add, Edit (name, Torznab URL, API key: left empty, the stored key stays, enabled),
//! Remove and Test (the source's caps: "OK, N categories" or the error, the key taken out).
//! Edits are stored at once through [`Host::store`] (the preferences' `search.source.N.*`).
//!
//! One search runs at a time ([`Search`]): its worker threads only wake the main loop
//! (`MainContext::invoke`), where [`collected`] merges their answers and redraws the window.
//! Closing the window stops the search.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_core::fetch::FetchProxy;
use patorrent_frontend::detail::ColumnDef;
use patorrent_frontend::lang::{S, gtk_mnemonic};
use patorrent_frontend::rss::Download;
use patorrent_frontend::search::source::url_host;
use patorrent_frontend::search::{
    Query, Search, SearchResult, SearchSource, Source, SourceStatus, category_text, fetch_caps,
    fetch_download, main_categories, redact,
};
use patorrent_frontend::{fmt, tr};

use crate::table::Table;

/// What the windows need of their owner.
pub trait Host {
    /// The sources, in their stored order (keys included).
    fn sources(&self) -> Vec<Source>;
    /// Stores the edited sources (the owner saves its preferences and updates the toolbar's
    /// menu).
    fn store(&self, sources: &[Source]);
    /// The engine's proxy (used for sources on public addresses only).
    fn proxy(&self) -> Option<FetchProxy>;
    /// The name of the torrent with this info-hash (lowercase hex) when it is in the list.
    fn in_list(&self, info_hash: &str) -> Option<String>;
    /// Adds what a result fetched, the way an opened `.torrent` or a magnet link is added.
    fn add(&self, download: Download);
}

/// The longest query sent (as the Web UI's).
const MAX_QUERY: usize = 256;

// ---- what the toolbar's menu picks (no widgets: tested) ----------------------------------

/// What Enter in the toolbar's search box does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Filters the torrent list by name (the box's own job).
    #[default]
    Filter,
    /// Searches every enabled source.
    All,
    /// Searches source `n` (an index into the stored sources).
    Source(usize),
}

impl Mode {
    /// The menu's action target for this mode.
    pub fn target(self) -> String {
        match self {
            Mode::Filter => "filter".to_owned(),
            Mode::All => "all".to_owned(),
            Mode::Source(n) => n.to_string(),
        }
    }

    pub fn from_target(t: &str) -> Mode {
        match t {
            "all" => Mode::All,
            _ => t.parse().map_or(Mode::Filter, Mode::Source),
        }
    }
}

/// The toolbar menu's choices: the name filter, each enabled source, then "All sources" (when
/// there is an enabled source).
pub fn menu_choices(sources: &[Source]) -> Vec<(Mode, String)> {
    let mut out = vec![(Mode::Filter, tr!(SearchCue).to_owned())];
    let enabled: Vec<(Mode, String)> = sources
        .iter()
        .enumerate()
        .filter(|(_, s)| s.enabled)
        .map(|(n, s)| (Mode::Source(n), s.label()))
        .collect();
    if !enabled.is_empty() {
        out.extend(enabled);
        out.push((Mode::All, tr!(SearchAllSources).to_owned()));
    }
    out
}

/// `mode` after the sources changed from `old` to `new`: the same source (by URL and name)
/// where it is now; the name filter when it is gone or switched off, or for "All sources" with
/// none left on.
pub fn remap(mode: Mode, old: &[Source], new: &[Source]) -> Mode {
    match mode {
        Mode::Filter => Mode::Filter,
        Mode::All if new.iter().any(|s| s.enabled) => Mode::All,
        Mode::All => Mode::Filter,
        Mode::Source(n) => {
            let Some(was) = old.get(n) else { return Mode::Filter };
            new.iter()
                .position(|s| s.url == was.url && s.name == was.name)
                .filter(|&i| new[i].enabled)
                .map_or(Mode::Filter, Mode::Source)
        }
    }
}

/// The sources a search in `mode` asks: every enabled one, or the one picked (searched even
/// when switched off since it was picked); `None` for the name filter or nothing to search.
pub fn picked(mode: Mode, all: &[Source]) -> Option<Vec<Source>> {
    let out: Vec<Source> = match mode {
        Mode::Filter => return None,
        Mode::All => all.iter().filter(|s| s.enabled).cloned().collect(),
        Mode::Source(n) => all.get(n).map(|s| Source { enabled: true, ..s.clone() }).into_iter().collect(),
    };
    (!out.is_empty()).then_some(out)
}

/// A GMenu label shows `_` as a mnemonic: a source's name keeps its underscores.
fn menu_label(text: &str) -> String {
    text.replace('_', "__")
}

/// The toolbar search box's menu: the [`menu_choices`] as radio items of `win.search-mode`
/// (their [`Mode::target`]), then `win.search-sources`.
pub fn toolbar_menu(sources: &[Source]) -> gio::Menu {
    let menu = gio::Menu::new();
    let modes = gio::Menu::new();
    for (mode, label) in menu_choices(sources) {
        let item = gio::MenuItem::new(Some(&menu_label(&label)), None);
        item.set_action_and_target_value(Some("win.search-mode"), Some(&mode.target().to_variant()));
        modes.append_item(&item);
    }
    menu.append_section(None, &modes);
    let manage = gio::Menu::new();
    manage.append(Some(&menu_label(tr!(SearchManage))), Some("win.search-sources"));
    menu.append_section(None, &manage);
    menu
}

// ---- editing the sources (no widgets: tested) ---------------------------------------------

/// The Add / Edit Search Source dialog's fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceForm {
    pub name: String,
    pub url: String,
    /// Empty when editing: the stored key stays.
    pub key: String,
    pub enabled: bool,
}

/// An `http://` or `https://` URL with a host, trimmed.
pub fn valid_url(url: &str) -> Option<String> {
    let url = url.trim();
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    (matches!(scheme.as_deref(), Some("http" | "https")) && url_host(url).is_some()).then(|| url.to_owned())
}

/// Puts `form` into `sources`: a new source (`index` `None`) at the end, else source `index`
/// changed (an empty key keeps its stored one). Returns the source's index, or the message
/// when the URL is not an `http(s)://` one.
pub fn apply_form(
    sources: &mut Vec<Source>,
    index: Option<usize>,
    form: &SourceForm,
) -> Result<usize, String> {
    let url = valid_url(&form.url).ok_or_else(|| tr!(SearchNoUrl).to_owned())?;
    let key = form.key.trim().to_owned();
    let mut s = Source { name: form.name.trim().to_owned(), url, api_key: key, enabled: form.enabled };
    match index.filter(|&i| i < sources.len()) {
        Some(i) => {
            if s.api_key.is_empty() {
                s.api_key = std::mem::take(&mut sources[i].api_key);
            }
            sources[i] = s;
            Ok(i)
        }
        None => {
            sources.push(s);
            Ok(sources.len() - 1)
        }
    }
}

// ---- the results' rows (no widgets: tested) -----------------------------------------------

/// The Search Torrents window's columns.
pub const COLUMNS: [ColumnDef; 7] = [
    (S::ColName, 320, false),
    (S::ColSize, 80, true),
    (S::ColSeeds, 60, true),
    (S::ColPeers, 60, true),
    (S::SearchColCategory, 80, false),
    (S::SearchColAge, 90, true),
    (S::SearchColSource, 120, false),
];

/// A source's status line.
pub fn status_line(s: &SearchSource) -> String {
    let source = s.source.label();
    match &s.status {
        SourceStatus::Searching => tr!(SearchSearching, source),
        SourceStatus::Done(count) => tr!(SearchFound, source, count),
        SourceStatus::Failed(error) => tr!(SearchFailed, source, error = s.source.redact(error)),
        SourceStatus::Stopped => tr!(SearchStopped, source),
    }
}

/// The Source cell: every source that returned the result.
fn source_names(r: &SearchResult, names: &[String]) -> String {
    let from: Vec<&str> = r.sources.iter().filter_map(|&i| names.get(i)).map(String::as_str).collect();
    from.join(", ")
}

/// A result's cells; `names` are the searched sources' labels, `now` the Unix time.
pub fn cells(r: &SearchResult, names: &[String], now: i64) -> [String; 7] {
    let count = |v: Option<u32>| v.map(|v| v.to_string()).unwrap_or_default();
    [
        r.name.clone(),
        r.size.map(fmt::size).unwrap_or_default(),
        count(r.seeders),
        count(r.peers),
        category_text(&r.categories),
        r.published.map(|p| fmt::duration(now.saturating_sub(p).max(0) as u64)).unwrap_or_default(),
        source_names(r, names),
    ]
}

/// The order the rows show `results` in: as merged (most seeders first) without a sort, else
/// by column `sort.0` (descending with `sort.1`); ties keep the merged order. Age sorts by the
/// date published, newest first ascending; an unknown value counts as the smallest.
pub fn order(results: &[SearchResult], names: &[String], sort: Option<(usize, bool)>) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..results.len()).collect();
    let Some((column, descending)) = sort else { return rows };
    let cmp = |a: &SearchResult, b: &SearchResult| -> Ordering {
        match column {
            0 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            1 => a.size.cmp(&b.size),
            2 => a.seeders.cmp(&b.seeders),
            3 => a.peers.cmp(&b.peers),
            4 => category_text(&a.categories).cmp(&category_text(&b.categories)),
            // younger first: a later date is a smaller age
            5 => match (a.published, b.published) {
                (Some(x), Some(y)) => y.cmp(&x),
                (x, y) => x.is_some().cmp(&y.is_some()),
            },
            _ => source_names(a, names).to_lowercase().cmp(&source_names(b, names).to_lowercase()),
        }
    };
    rows.sort_by(|&a, &b| {
        let o = cmp(&results[a], &results[b]);
        if descending { o.reverse() } else { o }
    });
    rows
}

/// What keeps a result's row selected while answers come in and the rows move: its
/// info-hash, else its name and size (as [`patorrent_frontend::search::merge`] de-duplicates).
pub fn result_key(r: &SearchResult) -> String {
    if r.info_hash.is_empty() {
        format!("{}\n{}", r.name.to_lowercase(), r.size.map(|s| s.to_string()).unwrap_or_default())
    } else {
        r.info_hash.clone()
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

// ---- the search --------------------------------------------------------------------------

thread_local! {
    static SEARCH: RefCell<Option<Search>> = const { RefCell::new(None) };
    /// What Enter in the toolbar's search box does.
    static MODE: Cell<Mode> = const { Cell::new(Mode::Filter) };
    static RESULTS: RefCell<Option<Weak<Results>>> = const { RefCell::new(None) };
    static SOURCES: RefCell<Option<Weak<Manager>>> = const { RefCell::new(None) };
}

/// The toolbar's mode.
pub fn mode() -> Mode {
    MODE.with(Cell::get)
}

pub fn set_mode(mode: Mode) {
    MODE.with(|m| m.set(mode));
}

fn with_search<R>(f: impl FnOnce(&mut Search) -> R) -> R {
    SEARCH.with(|s| {
        let mut s = s.borrow_mut();
        // the worker threads only wake the main loop; `collected` takes their answers there
        let s = s.get_or_insert_with(|| Search::new(|| glib::MainContext::default().invoke(collected)));
        f(s)
    })
}

/// Answers came in: merged here, and the open window shows them.
fn collected() {
    if with_search(Search::collect) {
        if let Some(r) = open_results() {
            r.refresh();
        }
    }
}

fn open_results() -> Option<Rc<Results>> {
    RESULTS.with(|o| o.borrow().as_ref().and_then(Weak::upgrade))
}

/// Starts searching `mode`'s sources for `text`; the message when there is nothing to search.
fn start(host: &dyn Host, text: &str, categories: Vec<u32>, mode: Mode) -> Result<(), String> {
    let Some(sources) = picked(mode, &host.sources()) else {
        with_search(Search::stop);
        return Err(tr!(SearchNoSources).to_owned());
    };
    let text: String = text.trim().chars().take(MAX_QUERY).collect();
    let proxy = host.proxy();
    with_search(|s| s.start(&sources, Query { text, categories }, proxy.as_ref()));
    Ok(())
}

/// The toolbar's Enter with a source picked: searches `mode`'s sources for `text` and shows
/// the Search Torrents window over `parent` (the open one, when there is one).
pub fn search(parent: &gtk::ApplicationWindow, host: Rc<dyn Host>, text: &str, mode: Mode) {
    if text.trim().is_empty() {
        return;
    }
    let r = open_results().unwrap_or_else(|| Results::new(parent, host));
    r.query.set_text(text.trim());
    r.fill_sources(mode);
    r.category.set_selected(0);
    r.start();
    if let Some(w) = r.window.upgrade() {
        w.present();
    }
}

/// The sources changed (the Search Sources window stored them): the open Search Torrents
/// window's source list follows.
fn sources_changed() {
    if let Some(r) = open_results() {
        let mode = r.mode();
        r.fill_sources(mode);
    }
}

// ---- the Search Torrents window ----------------------------------------------------------

struct Results {
    host: Rc<dyn Host>,
    window: glib::WeakRef<gtk::Window>,
    query: gtk::Entry,
    category: gtk::DropDown,
    source: gtk::DropDown,
    /// The source list's choices: "All sources", then each enabled source.
    source_modes: RefCell<Vec<Mode>>,
    stop: gtk::Button,
    add: gtk::Button,
    status: gtk::Label,
    count: gtk::Label,
    message: gtk::Label,
    /// A message instead of the status lines (nothing to search).
    notice: RefCell<String>,
    table: Table,
    rows: Rc<RefCell<Vec<[String; 7]>>>,
    /// The rows' results with their sources, as shown.
    shown: RefCell<Vec<(SearchResult, Source)>>,
    sort: Cell<Option<(usize, bool)>>,
}

impl Results {
    fn new(parent: &gtk::ApplicationWindow, host: Rc<dyn Host>) -> Rc<Results> {
        let window = gtk::Window::builder()
            .title(tr!(SearchTitle))
            .transient_for(parent)
            .default_width(900)
            .default_height(560)
            .build();
        let button = |t: &str| gtk::Button::with_mnemonic(&gtk_mnemonic(t));
        let rows: Rc<RefCell<Vec<[String; 7]>>> = Rc::default();
        let text_rows = rows.clone();
        let table = Table::new(
            &COLUMNS,
            Rc::new(move |r, c| text_rows.borrow().get(r).map(|row| row[c].clone()).unwrap_or_default()),
            None,
            None,
        );
        let mut categories = vec![tr!(SearchAllCategories)];
        categories.extend(main_categories().iter().map(|(_, n)| *n));
        let query = gtk::Entry::new();
        query.set_hexpand(true);
        query.set_placeholder_text(Some(tr!(SearchQueryCue)));
        let r = Rc::new(Results {
            host,
            window: window.downgrade(),
            query,
            category: gtk::DropDown::from_strings(&categories),
            source: gtk::DropDown::from_strings(&[]),
            source_modes: RefCell::default(),
            stop: button(tr!(ToolStop)),
            add: button(tr!(WebAdd)),
            status: gtk::Label::new(None),
            count: gtk::Label::new(None),
            message: gtk::Label::new(None),
            notice: RefCell::default(),
            table,
            rows,
            shown: RefCell::default(),
            sort: Cell::new(None),
        });
        RESULTS.with(|o| *o.borrow_mut() = Some(Rc::downgrade(&r)));

        let go = button(tr!(WebSearch));
        go.add_css_class("suggested-action");
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.append(&r.query);
        bar.append(&r.category);
        bar.append(&r.source);
        bar.append(&go);
        bar.append(&r.stop);
        r.status.set_xalign(0.0);
        r.status.set_wrap(true);
        r.status.set_selectable(true);
        r.status.set_visible(false);
        // every column sorts: the header shows the choice, `order` puts the rows in it
        let columns = r.table.view.columns();
        for i in 0..columns.n_items() {
            if let Some(col) = columns.item(i).and_downcast::<gtk::ColumnViewColumn>() {
                col.set_sorter(Some(&gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal)));
            }
        }
        if let Some(sorter) = r.table.view.sorter() {
            let w = Rc::downgrade(&r);
            sorter.connect_changed(move |_, _| {
                if let Some(r) = w.upgrade() {
                    r.read_sort();
                    r.refresh();
                }
            });
        }
        let scroll =
            gtk::ScrolledWindow::builder().child(&r.table.view).vexpand(true).has_frame(true).build();
        r.message.set_xalign(0.0);
        r.message.set_hexpand(true);
        r.message.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let manage = button(tr!(SearchManage));
        let close = button(tr!(BtnClose));
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        foot.append(&r.count);
        foot.append(&r.message);
        foot.append(&r.add);
        foot.append(&manage);
        foot.append(&close);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        margins(&body, 12);
        body.append(&bar);
        body.append(&r.status);
        body.append(&scroll);
        body.append(&foot);
        window.set_child(Some(&body));
        escape_closes(&window);

        let w = Rc::downgrade(&r);
        let again = move || {
            if let Some(r) = w.upgrade() {
                r.start();
            }
        };
        let a = again.clone();
        go.connect_clicked(move |_| a());
        r.query.connect_activate(move |_| again());
        let w = Rc::downgrade(&r);
        r.stop.connect_clicked(move |_| {
            with_search(Search::stop);
            if let Some(r) = w.upgrade() {
                r.refresh();
            }
        });
        let w = Rc::downgrade(&r);
        r.add.connect_clicked(move |_| {
            if let Some(r) = w.upgrade() {
                r.add_selected();
            }
        });
        let w = Rc::downgrade(&r);
        r.table.view.connect_activate(move |_, _| {
            if let Some(r) = w.upgrade() {
                r.add_selected();
            }
        });
        let w = Rc::downgrade(&r);
        r.table.selection.connect_selection_changed(move |_, _, _| {
            if let Some(r) = w.upgrade() {
                r.add.set_sensitive(!r.table.selected().is_empty());
            }
        });
        let w = Rc::downgrade(&r);
        manage.connect_clicked(move |_| {
            if let Some(r) = w.upgrade() {
                if let Some(win) = r.window.upgrade() {
                    show_sources(&win, r.host.clone());
                }
            }
        });
        let ww = window.downgrade();
        close.connect_clicked(move |_| {
            if let Some(w) = ww.upgrade() {
                w.close();
            }
        });
        window.connect_close_request({
            let r = r.clone();
            move |_| {
                let _ = &r;
                // nobody looks at the answers any more
                with_search(Search::stop);
                RESULTS.with(|o| o.borrow_mut().take());
                glib::Propagation::Proceed
            }
        });
        r.add.set_sensitive(false);
        r.refresh();
        r
    }

    /// The source picked in the window's list.
    fn mode(&self) -> Mode {
        self.source_modes.borrow().get(self.source.selected() as usize).copied().unwrap_or(Mode::All)
    }

    /// Fills the source list ("All sources", then each enabled source) with `mode` picked.
    fn fill_sources(&self, mode: Mode) {
        let sources = self.host.sources();
        let mut modes = vec![Mode::All];
        let mut names = vec![tr!(SearchAllSources).to_owned()];
        for (n, s) in sources.iter().enumerate() {
            if s.enabled || mode == Mode::Source(n) {
                modes.push(Mode::Source(n));
                names.push(s.label());
            }
        }
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        self.source.set_model(Some(&gtk::StringList::new(&names)));
        let at = modes.iter().position(|&m| m == mode).unwrap_or(0);
        *self.source_modes.borrow_mut() = modes;
        self.source.set_selected(at as u32);
    }

    /// Search (or Enter in the query box): the query, the category and the source picked.
    fn start(&self) {
        let text = self.query.text();
        if text.trim().is_empty() {
            self.query.grab_focus();
            return;
        }
        let categories = match self.category.selected() {
            0 => Vec::new(),
            n => main_categories().get(n as usize - 1).map(|(id, _)| vec![*id]).unwrap_or_default(),
        };
        let notice = match start(self.host.as_ref(), &text, categories, self.mode()) {
            Ok(()) => String::new(),
            Err(e) => e,
        };
        *self.notice.borrow_mut() = notice;
        self.message.set_text("");
        self.table.unselect_all();
        self.refresh();
    }

    /// The header's sort choice into `sort`.
    fn read_sort(&self) {
        self.sort.set(self.header_sort());
    }

    fn header_sort(&self) -> Option<(usize, bool)> {
        let sorter = self.table.view.sorter().and_downcast::<gtk::ColumnViewSorter>()?;
        let col = sorter.primary_sort_column()?;
        let columns = self.table.view.columns();
        let n = (0..columns.n_items())
            .find(|&i| columns.item(i).as_ref() == Some(col.upcast_ref::<glib::Object>()))?;
        Some((n as usize, sorter.primary_sort_order() == gtk::SortType::Descending))
    }

    /// The rows, the status lines and the buttons again from the search.
    fn refresh(&self) {
        let now = unix_now();
        let sort = self.sort.get();
        let (rows, shown, lines, total, running) = with_search(|s| {
            let names: Vec<String> = s.sources().iter().map(|x| x.source.label()).collect();
            let results = s.results();
            let order = order(results, &names, sort);
            let rows: Vec<[String; 7]> = order.iter().map(|&i| cells(&results[i], &names, now)).collect();
            let shown: Vec<(SearchResult, Source)> = order
                .iter()
                .filter_map(|&i| {
                    let r = &results[i];
                    s.source_of(r).map(|src| (r.clone(), src.clone()))
                })
                .collect();
            let lines: Vec<String> = s.sources().iter().map(status_line).collect();
            (rows, shown, lines, results.len(), s.is_running())
        });
        let keep: HashSet<String> = {
            let old = self.shown.borrow();
            self.table.selected().iter().filter_map(|&i| old.get(i)).map(|(r, _)| result_key(r)).collect()
        };
        let n = rows.len();
        *self.rows.borrow_mut() = rows;
        let again: Vec<u32> = shown
            .iter()
            .enumerate()
            .filter(|(_, (r, _))| keep.contains(&result_key(r)))
            .map(|(i, _)| i as u32)
            .collect();
        *self.shown.borrow_mut() = shown;
        self.table.update(n);
        let set = gtk::Bitset::new_empty();
        for i in again {
            set.add(i);
        }
        self.table.selection.set_selection(&set, &gtk::Bitset::new_range(0, n as u32));
        let notice = self.notice.borrow().clone();
        let status = if notice.is_empty() { lines.join("\n") } else { notice };
        self.status.set_text(&status);
        self.status.set_visible(!status.is_empty());
        self.count.set_text(&tr!(SearchResultCount, count = total));
        self.stop.set_sensitive(running);
        self.add.set_sensitive(!self.table.selected().is_empty());
    }

    /// Add (a double click, Enter): each selected result.
    fn add_selected(self: &Rc<Self>) {
        let picked: Vec<(SearchResult, Source)> = {
            let shown = self.shown.borrow();
            self.table.selected().iter().filter_map(|&i| shown.get(i).cloned()).collect()
        };
        for (result, source) in picked {
            self.add_result(result, source);
        }
    }

    /// Fetches what `result` adds on GIO's thread pool (its magnet, else the `.torrent` or
    /// magnet behind its link), then hands it to the owner.
    fn add_result(self: &Rc<Self>, result: SearchResult, source: Source) {
        if !result.info_hash.is_empty() {
            if let Some(name) = self.host.in_list(&result.info_hash) {
                self.say(&tr!(InfoAlreadyInList, name), true);
                return;
            }
        }
        let (host, w, proxy) = (self.host.clone(), Rc::downgrade(self), self.host.proxy());
        glib::spawn_future_local(async move {
            let (name, label) = (result.name.clone(), source.label());
            let key = source.api_key.clone();
            let fetched = gio::spawn_blocking(move || fetch_download(&result, &source, proxy.as_ref())).await;
            let said = match fetched {
                Ok(Ok(d)) => {
                    host.add(d);
                    (tr!(SearchAdded, name), false)
                }
                Ok(Err(e)) => (tr!(SearchFailed, source = label, error = redact(&e, &key)), true),
                // a panic on the pool's thread: nothing was fetched
                Err(_) => return,
            };
            if let Some(r) = w.upgrade() {
                r.say(&said.0, said.1);
            }
        });
    }

    /// The line beside the buttons (red for a failure).
    fn say(&self, text: &str, failed: bool) {
        self.message.set_text(text);
        self.message.set_tooltip_text(Some(text));
        if failed {
            self.message.add_css_class("error");
        } else {
            self.message.remove_css_class("error");
        }
    }
}

// ---- the Search Sources window -----------------------------------------------------------

struct Manager {
    host: Rc<dyn Host>,
    window: glib::WeakRef<gtk::Window>,
    list: gtk::ListBox,
    status: gtk::Label,
    edit: gtk::Button,
    remove: gtk::Button,
    test: gtk::Button,
    /// The rows are being filled: their check boxes' signals are not edits.
    loading: Cell<bool>,
}

/// Shows the Search Sources window over `parent` (the open one, when there is one).
pub fn show_sources(parent: &gtk::Window, host: Rc<dyn Host>) {
    if let Some(m) = SOURCES.with(|o| o.borrow().as_ref().and_then(Weak::upgrade)) {
        if let Some(w) = m.window.upgrade() {
            w.present();
        }
        return;
    }
    let window = gtk::Window::builder()
        .title(tr!(SearchSourcesTitle))
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .default_height(380)
        .build();
    let button = |t: &str| gtk::Button::with_mnemonic(&gtk_mnemonic(t));
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(false);
    let m = Rc::new(Manager {
        host,
        window: window.downgrade(),
        list,
        status: gtk::Label::new(None),
        edit: button(tr!(RssFeedsEdit)),
        remove: button(tr!(RssFeedsRemove)),
        test: button(tr!(SearchTest)),
        loading: Cell::new(false),
    });
    SOURCES.with(|o| *o.borrow_mut() = Some(Rc::downgrade(&m)));
    let add = button(tr!(RssFeedsAdd));
    let close = button(tr!(BtnClose));
    let scroll = gtk::ScrolledWindow::builder().child(&m.list).vexpand(true).has_frame(true).build();
    m.status.set_xalign(0.0);
    m.status.set_wrap(true);
    m.status.set_selectable(true);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.append(&add);
    buttons.append(&m.edit);
    buttons.append(&m.remove);
    buttons.append(&m.test);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    buttons.append(&spacer);
    buttons.append(&close);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    margins(&body, 12);
    body.append(&scroll);
    body.append(&m.status);
    body.append(&buttons);
    window.set_child(Some(&body));
    escape_closes(&window);

    let w = Rc::downgrade(&m);
    add.connect_clicked(move |_| {
        if let Some(m) = w.upgrade() {
            m.open_editor(None);
        }
    });
    let w = Rc::downgrade(&m);
    m.edit.connect_clicked(move |_| {
        if let Some(m) = w.upgrade() {
            m.open_editor(m.selected());
        }
    });
    let w = Rc::downgrade(&m);
    m.list.connect_row_activated(move |_, row| {
        if let Some(m) = w.upgrade() {
            m.open_editor(usize::try_from(row.index()).ok());
        }
    });
    let w = Rc::downgrade(&m);
    m.remove.connect_clicked(move |_| {
        if let Some(m) = w.upgrade() {
            m.ask_remove();
        }
    });
    let w = Rc::downgrade(&m);
    m.test.connect_clicked(move |_| {
        if let Some(m) = w.upgrade() {
            m.run_test();
        }
    });
    let w = Rc::downgrade(&m);
    m.list.connect_selected_rows_changed(move |_| {
        if let Some(m) = w.upgrade() {
            m.buttons();
        }
    });
    let ww = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(w) = ww.upgrade() {
            w.close();
        }
    });
    window.connect_close_request({
        let m = m.clone();
        move |_| {
            let _ = &m;
            SOURCES.with(|o| o.borrow_mut().take());
            glib::Propagation::Proceed
        }
    });
    m.fill(Some(0));
    window.present();
}

impl Manager {
    fn selected(&self) -> Option<usize> {
        self.list.selected_row().and_then(|r| usize::try_from(r.index()).ok())
    }

    fn buttons(&self) {
        let on = self.selected().is_some();
        self.edit.set_sensitive(on);
        self.remove.set_sensitive(on);
        self.test.set_sensitive(on);
    }

    /// The rows again from the stored sources, `select` picked.
    fn fill(self: &Rc<Self>, select: Option<usize>) {
        self.loading.set(true);
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        for (n, s) in self.host.sources().iter().enumerate() {
            let check = gtk::CheckButton::new();
            check.set_active(s.enabled);
            check.set_tooltip_text(Some(tr!(SearchSourceEnabled)));
            let w = Rc::downgrade(self);
            check.connect_toggled(move |c| {
                let Some(m) = w.upgrade() else { return };
                if m.loading.get() {
                    return;
                }
                let mut sources = m.host.sources();
                if let Some(s) = sources.get_mut(n) {
                    s.enabled = c.is_active();
                    store(m.host.as_ref(), &sources);
                }
            });
            let name = gtk::Label::new(Some(&s.label()));
            name.set_xalign(0.0);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            // never the key: the URL as an error would show it
            let url = gtk::Label::new(Some(&redact(&s.url, &s.api_key)));
            url.set_xalign(0.0);
            url.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            url.add_css_class("dim-label");
            let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
            text.set_hexpand(true);
            text.append(&name);
            text.append(&url);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            margins(&line, 4);
            line.append(&check);
            line.append(&text);
            self.list.append(&line);
        }
        if let Some(row) = select.and_then(|n| self.list.row_at_index(n as i32)) {
            self.list.select_row(Some(&row));
        }
        self.loading.set(false);
        self.buttons();
    }

    /// Add (`index` `None`) or Edit.
    fn open_editor(self: &Rc<Self>, index: Option<usize>) {
        let Some(window) = self.window.upgrade() else { return };
        let sources = self.host.sources();
        let current = index.and_then(|i| sources.get(i));
        if index.is_some() && current.is_none() {
            return;
        }
        let w = Rc::downgrade(self);
        source_dialog(&window, current, move |form| {
            let Some(m) = w.upgrade() else { return Ok(()) };
            let mut sources = m.host.sources();
            let at = apply_form(&mut sources, index, &form)?;
            store(m.host.as_ref(), &sources);
            m.status.set_text("");
            m.fill(Some(at));
            Ok(())
        });
    }

    fn ask_remove(self: &Rc<Self>) {
        let (Some(window), Some(n)) = (self.window.upgrade(), self.selected()) else { return };
        let Some(name) = self.host.sources().get(n).map(Source::label) else { return };
        let d = gtk::AlertDialog::builder()
            .message(tr!(AskRemoveSource, name))
            .buttons([tr!(BtnCancel), tr!(BtnOk)])
            .cancel_button(0)
            .default_button(1)
            .modal(true)
            .build();
        let w = Rc::downgrade(self);
        d.choose(Some(&window), None::<&gio::Cancellable>, move |r| {
            let Some(m) = w.upgrade() else { return };
            if r != Ok(1) {
                return;
            }
            let mut sources = m.host.sources();
            if n < sources.len() {
                sources.remove(n);
                store(m.host.as_ref(), &sources);
            }
            m.status.set_text("");
            m.fill(Some(n.min(sources.len().saturating_sub(1))));
        });
    }

    /// Test: asks the selected source for its categories on GIO's thread pool.
    fn run_test(self: &Rc<Self>) {
        let Some(source) = self.selected().and_then(|n| self.host.sources().get(n).cloned()) else { return };
        let label = source.label();
        self.status.set_text(&tr!(SearchTesting, source = label));
        self.status.remove_css_class("error");
        self.test.set_sensitive(false);
        let (w, proxy) = (Rc::downgrade(self), self.host.proxy());
        glib::spawn_future_local(async move {
            let key = source.api_key.clone();
            let answer = gio::spawn_blocking(move || fetch_caps(&source, proxy.as_ref())).await;
            let Some(m) = w.upgrade() else { return };
            m.test.set_sensitive(m.selected().is_some());
            let (text, failed) = match answer {
                Ok(Ok(cats)) => (tr!(SearchTestOk, source = label, count = cats.len()), false),
                Ok(Err(e)) => (tr!(SearchFailed, source = label, error = redact(&e.to_string(), &key)), true),
                Err(_) => return,
            };
            m.status.set_text(&text);
            if failed {
                m.status.add_css_class("error");
            }
        });
    }
}

/// Stores `sources` through `host`; the Search Torrents window's source list follows.
fn store(host: &dyn Host, sources: &[Source]) {
    host.store(sources);
    sources_changed();
}

/// The Add / Edit Search Source dialog over `parent` (`current`: the source edited); `done`
/// gets the fields on OK and answers the message to show when they cannot be stored.
fn source_dialog(
    parent: &gtk::Window,
    current: Option<&Source>,
    done: impl Fn(SourceForm) -> Result<(), String> + 'static,
) {
    let title = if current.is_some() { tr!(SearchEditSource) } else { tr!(SearchAddSource) };
    let window =
        gtk::Window::builder().title(title).transient_for(parent).modal(true).resizable(false).build();
    let entry = |t: &str| {
        let e = gtk::Entry::new();
        e.set_text(t);
        e.set_activates_default(true);
        e.set_hexpand(true);
        e
    };
    let name = entry(current.map_or("", |s| s.name.as_str()));
    let url = entry(current.map_or("", |s| s.url.as_str()));
    url.set_input_purpose(gtk::InputPurpose::Url);
    // never the stored key: left empty, it stays
    let mut key = gtk::PasswordEntry::builder().show_peek_icon(true).activates_default(true).hexpand(true);
    if current.is_some_and(|s| !s.api_key.is_empty()) {
        key = key.placeholder_text(tr!(SearchSourceKeyKept));
    }
    let key = key.build();
    let enabled = gtk::CheckButton::with_label(tr!(SearchSourceEnabled));
    enabled.set_active(current.is_none_or(|s| s.enabled));
    let hint = gtk::Label::new(Some(tr!(SearchSourceUrlHint)));
    hint.set_xalign(0.0);
    // a fixed width: a wrapping label in a window that cannot be resized otherwise measures
    // taller than the window GTK gives it
    hint.set_wrap(true);
    hint.set_width_chars(56);
    hint.set_max_width_chars(56);
    hint.set_selectable(true);
    hint.add_css_class("dim-label");
    let caption = |t: &str| {
        let l = gtk::Label::new(Some(t));
        l.set_xalign(0.0);
        l
    };
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(8);
    grid.attach(&caption(tr!(SearchSourceName)), 0, 0, 1, 1);
    grid.attach(&name, 1, 0, 1, 1);
    grid.attach(&caption(tr!(SearchSourceUrl)), 0, 1, 1, 1);
    grid.attach(&url, 1, 1, 1, 1);
    grid.attach(&hint, 1, 2, 1, 1);
    grid.attach(&caption(tr!(SearchSourceKey)), 0, 3, 1, 1);
    grid.attach(&key, 1, 3, 1, 1);
    grid.attach(&enabled, 1, 4, 1, 1);
    let ok = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnOk)));
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnCancel)));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&ok);
    buttons.append(&cancel);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    margins(&body, 12);
    body.append(&grid);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&ok));
    escape_closes(&window);
    let w = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    let (w, name_w, url_w, key_w, enabled_w) =
        (window.downgrade(), name.downgrade(), url.downgrade(), key.downgrade(), enabled.downgrade());
    ok.connect_clicked(move |_| {
        let (Some(w), Some(name), Some(url), Some(key), Some(enabled)) =
            (w.upgrade(), name_w.upgrade(), url_w.upgrade(), key_w.upgrade(), enabled_w.upgrade())
        else {
            return;
        };
        let form = SourceForm {
            name: name.text().to_string(),
            url: url.text().to_string(),
            key: key.text().to_string(),
            enabled: enabled.is_active(),
        };
        match done(form) {
            Ok(()) => w.close(),
            Err(text) => {
                gtk::AlertDialog::builder().message(text).modal(true).build().show(Some(&w));
                url.grab_focus();
            }
        }
    });
    window.present();
    if current.is_some() { &url } else { &name }.grab_focus();
}

fn margins(w: &impl IsA<gtk::Widget>, m: i32) {
    w.set_margin_top(m);
    w.set_margin_bottom(m);
    w.set_margin_start(m);
    w.set_margin_end(m);
}

fn escape_closes(window: &gtk::Window) {
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(name: &str, url: &str, enabled: bool) -> Source {
        Source { name: name.into(), url: url.into(), api_key: format!("key-{name}"), enabled }
    }

    fn result(name: &str, size: Option<u64>, seeders: Option<u32>, published: Option<i64>) -> SearchResult {
        SearchResult {
            name: name.into(),
            size,
            seeders,
            peers: seeders.map(|s| s + 1),
            info_hash: String::new(),
            categories: Vec::new(),
            published,
            link: String::new(),
            magnet: String::new(),
            source: 0,
            sources: vec![0],
        }
    }

    #[test]
    fn mode_targets_round_trip() {
        for m in [Mode::Filter, Mode::All, Mode::Source(0), Mode::Source(12)] {
            assert_eq!(Mode::from_target(&m.target()), m);
        }
        assert_eq!(Mode::from_target("junk"), Mode::Filter);
        assert_eq!(Mode::default(), Mode::Filter);
    }

    #[test]
    fn menu_lists_the_filter_enabled_sources_and_all() {
        assert_eq!(menu_choices(&[]), [(Mode::Filter, "Search torrent names".to_owned())]);
        let off = [src("a", "http://h/1/api", false)];
        assert_eq!(menu_choices(&off).len(), 1);
        let sources = [
            src("a", "http://h/1/api", true),
            src("b", "http://h/2/api", false),
            src("", "http://j/x", true),
        ];
        let modes: Vec<(Mode, String)> = menu_choices(&sources);
        assert_eq!(
            modes,
            [
                (Mode::Filter, "Search torrent names".to_owned()),
                (Mode::Source(0), "a".to_owned()),
                (Mode::Source(2), "j".to_owned()),
                (Mode::All, "All sources".to_owned()),
            ]
        );
        assert_eq!(menu_label("my_indexer"), "my__indexer");
    }

    #[test]
    fn modes_follow_their_source() {
        let old = [src("a", "http://h/1/api", true), src("b", "http://h/2/api", true)];
        // "a" removed: "b" moved up
        let new = [src("b", "http://h/2/api", true)];
        assert_eq!(remap(Mode::Source(1), &old, &new), Mode::Source(0));
        assert_eq!(remap(Mode::Source(0), &old, &new), Mode::Filter);
        assert_eq!(remap(Mode::All, &old, &new), Mode::All);
        assert_eq!(remap(Mode::Filter, &old, &new), Mode::Filter);
        // switched off
        let off = [src("a", "http://h/1/api", true), src("b", "http://h/2/api", false)];
        assert_eq!(remap(Mode::Source(1), &old, &off), Mode::Filter);
        assert_eq!(remap(Mode::All, &old, &[src("a", "http://h/1/api", false)]), Mode::Filter);
        assert_eq!(remap(Mode::Source(5), &old, &old), Mode::Filter);
    }

    #[test]
    fn a_search_asks_the_picked_sources() {
        let all = [src("a", "http://h/1/api", true), src("b", "http://h/2/api", false)];
        assert_eq!(picked(Mode::Filter, &all), None);
        assert_eq!(picked(Mode::All, &all), Some(vec![all[0].clone()]));
        // picked by name: searched although switched off
        assert_eq!(picked(Mode::Source(1), &all), Some(vec![Source { enabled: true, ..all[1].clone() }]));
        assert_eq!(picked(Mode::Source(2), &all), None);
        assert_eq!(picked(Mode::All, &all[1..]), None);
    }

    #[test]
    fn the_form_adds_and_edits_keeping_the_key() {
        let mut sources = vec![src("a", "http://h/1/api", true)];
        let form = |name: &str, url: &str, key: &str| SourceForm {
            name: name.into(),
            url: url.into(),
            key: key.into(),
            enabled: false,
        };
        assert_eq!(apply_form(&mut sources, None, &form("b", " https://j/x ", "k2")), Ok(1));
        assert_eq!(
            sources[1],
            Source { name: "b".into(), url: "https://j/x".into(), api_key: "k2".into(), enabled: false }
        );
        // an empty key keeps the stored one
        assert_eq!(apply_form(&mut sources, Some(0), &form(" a2 ", "http://h/9/api", "")), Ok(0));
        assert_eq!(sources[0].api_key, "key-a");
        assert_eq!(
            (sources[0].name.as_str(), sources[0].url.as_str(), sources[0].enabled),
            ("a2", "http://h/9/api", false)
        );
        // a new key replaces it
        assert_eq!(apply_form(&mut sources, Some(0), &form("a2", "http://h/9/api", " k3 ")), Ok(0));
        assert_eq!(sources[0].api_key, "k3");
        for bad in ["", "h/1/api", "ftp://h/1", "http://", "javascript:alert(1)"] {
            assert_eq!(
                apply_form(&mut sources, None, &form("x", bad, "")),
                Err("Enter the source's Torznab URL.".to_owned()),
                "{bad}"
            );
        }
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn status_lines_never_show_the_key() {
        let s = src("idx", "http://h/1/api", true);
        let line = |status| status_line(&SearchSource { source: s.clone(), status });
        assert_eq!(line(SourceStatus::Searching), "idx: searching...");
        assert_eq!(line(SourceStatus::Done(3)), "idx: 3 results");
        assert_eq!(line(SourceStatus::Stopped), "idx: stopped");
        assert_eq!(
            line(SourceStatus::Failed(
                "GET http://h/1/api?t=caps&apikey=key-idx refused, key key-idx".into()
            )),
            "idx: GET http://h/1/api?t=caps&apikey=*** refused, key ***"
        );
    }

    #[test]
    fn rows_sort_by_any_column() {
        let mut rs = vec![
            result("Beta", Some(2_000), Some(50), Some(1_000)),
            result("alpha", None, Some(10), Some(3_000)),
            result("Gamma", Some(1_000), None, None),
        ];
        rs[1].sources = vec![0, 1];
        rs[2].categories = vec![5040];
        let names = ["One".to_owned(), "Two".to_owned()];
        assert_eq!(order(&rs, &names, None), [0, 1, 2]);
        assert_eq!(order(&rs, &names, Some((0, false))), [1, 0, 2]);
        assert_eq!(order(&rs, &names, Some((0, true))), [2, 0, 1]);
        assert_eq!(order(&rs, &names, Some((1, false))), [1, 2, 0]);
        assert_eq!(order(&rs, &names, Some((2, true))), [0, 1, 2]);
        assert_eq!(order(&rs, &names, Some((4, true))), [2, 0, 1]);
        // the youngest first, unknown dates first too (the smallest)
        assert_eq!(order(&rs, &names, Some((5, false))), [2, 1, 0]);
        assert_eq!(order(&rs, &names, Some((6, true))), [1, 0, 2]);
        let c = cells(&rs[1], &names, 3_000 + 7_200);
        assert_eq!(c[0], "alpha");
        assert_eq!((c[1].as_str(), c[2].as_str(), c[3].as_str()), ("", "10", "11"));
        assert_eq!(c[5], fmt::duration(7_200));
        assert_eq!(c[6], "One, Two");
        assert_eq!(cells(&rs[2], &names, 0)[4], "TV");
    }

    #[test]
    fn rows_keep_their_selection_by_hash_or_name_and_size() {
        let mut a = result("Name", Some(5), Some(1), None);
        assert_eq!(result_key(&a), "name\n5");
        a.info_hash = "ab".repeat(20);
        assert_eq!(result_key(&a), "ab".repeat(20));
    }
}
