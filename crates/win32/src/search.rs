//! Torrent search through the user's Prowlarr or Jackett (D41, `docs/INTEGRATIONS.md` part 2)
//! in the Win32 window, on the shared model ([`patorrent_frontend::search`]).
//!
//! - The toolbar search box keeps filtering the list by name (D35); its drop-down
//!   ([`mode_menu`]) picks what Enter does instead: search one source or all of them
//!   ([`SearchMode`], remembered as `ui.search_source`).
//! - **Search Results** ([`open`]): a modeless, resizable window with the query, a category and
//!   a source, the per-source status lines, the merged results in a 2.2.1 list view (Name,
//!   Size, Seeds, Peers, Category, Age, Source; sorted by a header click), Stop, and Add (the
//!   button, a double-click or Enter in the list). Its [`Search`] answers on worker threads,
//!   whose notify posts [`WM_APP_SEARCH`] to the window; adding a result fetches it on a thread
//!   ([`fetch_download`]) and hands it to the window's own add path ([`Host::add`]).
//! - **Search Sources** ([`manage`]): the sources (Name, the URL with the key masked, a check
//!   box for on and off) with Add, Edit, Remove and Test (`t=caps` on a thread). Edits go to the
//!   window's preferences at once ([`Host::store_sources`]: `patorrent.ini`, the key sealed),
//!   and from there to the Web UI the window serves.
//!
//! An API key is never shown: the list and every message have it masked ([`redact`]), and the
//! edit dialog's key box starts empty (left empty, the stored key stays).

use std::cell::RefCell;
use std::cmp::Ordering;
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use std::sync::Arc;

use patorrent_core::fetch::FetchProxy;
use patorrent_core::util::time::unix_now;
use patorrent_frontend::prefs::Prefs;
use patorrent_frontend::rss::Download;
use patorrent_frontend::search::source::url_host;
use patorrent_frontend::search::worker::Fetcher;
use patorrent_frontend::search::{
    Category, Query, Search, SearchError, SearchResult, Source, SourceStatus, category_text, fetch_caps,
    fetch_download, main_categories, redact,
};
use patorrent_frontend::{fmt, tr};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetDC, GetTextExtentPoint32W, HGDIOBJ, InvalidateRect, ReleaseDC, SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::controls;
use crate::dialogs::{self, BUTTON, COMBOBOX, Class, DWLP_MSGRESULT, DWLP_USER, EDIT, STATIC, Template};
use crate::util::{copy_to, hiword, loword, send, wide, window_text};

/// A static control style (in `System::SystemServices`, a feature we do not otherwise need).
const SS_NOPREFIX: u32 = 0x80;

/// Posted to the Search Results window by its search's worker threads: answers came in.
pub const WM_APP_SEARCH: u32 = WM_APP + 45;
/// Posted to the Search Results window and the Search Sources dialog when the sources changed
/// (here or in a browser): they fill their lists again.
pub const WM_SEARCH_SOURCES: u32 = WM_APP + 46;
/// A result was fetched for Add; `lParam` is a `Box<Fetched>`.
const WM_SEARCH_FETCHED: u32 = WM_APP + 47;
/// A source's Test answered; `lParam` is a `Box<Tested>`.
const WM_SEARCH_TESTED: u32 = WM_APP + 48;
/// Sent to an open Search Results window: search for `*(lParam as *const Start)`.
const WM_SEARCH_START: u32 = WM_APP + 49;

/// Longest query taken (as the Web UI's).
const MAX_QUERY: usize = 512;
/// Most sources kept.
pub use patorrent_frontend::search::source::MAX_SOURCES;

/// What the windows need of their owner.
pub trait Host {
    /// The sources, in order (their keys included: needed to search; never show them).
    fn sources(&mut self) -> Vec<Source>;
    /// Stores the sources (the owner saves, mirrors them to its Web UI and tells the windows).
    fn store_sources(&mut self, sources: &[Source]);
    /// The engine's proxy (a source on this machine or network never uses it).
    fn proxy(&mut self) -> Option<FetchProxy>;
    /// Adds a fetched result through the window's add path (the Add dialog unless Preferences
    /// turned it off); `owner` owns that dialog. `name` is the result's title.
    fn add(&mut self, owner: HWND, d: Download, name: &str);
    /// The Search Results window while it is open, null once it closes.
    fn set_results_window(&mut self, dlg: HWND);
    /// The Search Sources dialog while it is open, null once it closes.
    fn set_sources_dialog(&mut self, dlg: HWND);
}

// ---- the toolbar search box's mode ------------------------------------------------------

/// What Enter in the toolbar search box does: filter the list (as typing does), or search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SearchMode {
    /// Typing filters the list by name (D35); Enter goes to the list.
    #[default]
    Filter,
    /// Enter searches every enabled source.
    All,
    /// Enter searches the source with this [`source_id`].
    Source(String),
}

/// The preference that remembers the mode: empty or missing for Filter, `*` for All sources,
/// else the source's [`source_id`].
pub const MODE_PREF: &str = "ui.search_source";

/// A source as the mode remembers it: its URL with the key masked (so `patorrent.ini` holds
/// no key in the clear; two Prowlarr indexers on one host differ by their path).
pub fn source_id(s: &Source) -> String {
    redact(s.url.trim(), &s.api_key)
}

impl SearchMode {
    pub fn load(prefs: &Prefs) -> SearchMode {
        match prefs.get(MODE_PREF).map(str::trim) {
            None | Some("") => SearchMode::Filter,
            Some("*") => SearchMode::All,
            Some(id) => SearchMode::Source(id.to_owned()),
        }
    }

    pub fn store(&self, prefs: &mut Prefs) {
        match self {
            SearchMode::Filter => prefs.remove(MODE_PREF),
            SearchMode::All => prefs.set(MODE_PREF, "*"),
            SearchMode::Source(id) => prefs.set(MODE_PREF, id),
        }
    }

    /// The mode with these sources: a picked source that is gone or switched off falls back
    /// to Filter.
    pub fn resolve(self, sources: &[Source]) -> SearchMode {
        match &self {
            SearchMode::Source(id) if !sources.iter().any(|s| s.enabled && source_id(s) == *id) => {
                SearchMode::Filter
            }
            _ => self,
        }
    }

    /// The search box's grey hint, which says what Enter does.
    pub fn cue(&self, sources: &[Source]) -> String {
        match self {
            SearchMode::Filter => tr!(SearchCue).to_owned(),
            SearchMode::All => tr!(SearchCueAll).to_owned(),
            SearchMode::Source(id) => {
                let label = sources.iter().find(|s| source_id(s) == *id).map(menu_label_of);
                tr!(SearchCueSource, source = label.unwrap_or_else(|| id.clone()))
            }
        }
    }
}

/// A source's name in the menus: its [`Source::label`].
fn menu_label_of(s: &Source) -> String {
    s.label()
}

/// One entry of the search box's drop-down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub text: String,
    pub mode: SearchMode,
    pub checked: bool,
    pub enabled: bool,
}

/// The drop-down's entries: Filter torrents, each enabled source, All sources (greyed with
/// none). The caller adds a separator and Manage Sources. Two sources with the same name
/// (Prowlarr indexers on one host, unnamed) show their masked URLs instead.
pub fn mode_menu(sources: &[Source], current: &SearchMode) -> Vec<MenuEntry> {
    let entry = |text: String, mode: SearchMode, enabled: bool| MenuEntry {
        checked: mode == *current,
        text,
        mode,
        enabled,
    };
    let on: Vec<&Source> = sources.iter().filter(|s| s.enabled).collect();
    let mut out = vec![entry(tr!(SearchFilterTorrents).to_owned(), SearchMode::Filter, true)];
    for s in &on {
        let label = s.label();
        let twin = on.iter().filter(|o| o.label() == label).count() > 1;
        let text = if twin { source_id(s) } else { label };
        out.push(entry(text, SearchMode::Source(source_id(s)), true));
    }
    out.push(entry(tr!(SearchAllSources).to_owned(), SearchMode::All, !on.is_empty()));
    out
}

// ---- the Search Results window ----------------------------------------------------------

const ID_QUERY: u16 = 100;
const ID_CATEGORY: u16 = 101;
const ID_SOURCE: u16 = 102;
const ID_GO: u16 = 103;
const ID_STOP: u16 = 104;
const ID_STATUS: u16 = 105;
pub(crate) const ID_LIST: u16 = 106;
const ID_COUNT: u16 = 107;
pub(crate) const ID_ADD: u16 = 108;

/// The result list's columns: title, width in dialog units, right-aligned.
const COLUMNS: [(Col, i32, bool); 7] = [
    (Col::Name, 190, false),
    (Col::Size, 44, true),
    (Col::Seeds, 32, true),
    (Col::Peers, 32, true),
    (Col::Category, 40, false),
    (Col::Age, 40, true),
    (Col::Source, 60, false),
];

/// A result list column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Name,
    Size,
    Seeds,
    Peers,
    Category,
    Age,
    Source,
}

impl Col {
    fn title(self) -> &'static str {
        match self {
            Col::Name => tr!(ColName),
            Col::Size => tr!(ColSize),
            Col::Seeds => tr!(ColSeeds),
            Col::Peers => tr!(ColPeers),
            Col::Category => tr!(SearchColCategory),
            Col::Age => tr!(SearchColAge),
            Col::Source => tr!(SearchColSource),
        }
    }

    /// The first click sorts numbers most first, words A to Z, the age newest first.
    fn first_descending(self) -> bool {
        matches!(self, Col::Size | Col::Seeds | Col::Peers)
    }
}

/// A result's cells, in [`COLUMNS`] order; `names` are the searched sources' labels, `now` the
/// time the ages count to.
pub fn cells(r: &SearchResult, names: &[String], now: i64) -> [String; 7] {
    let num = |v: Option<u32>| v.map(|v| v.to_string()).unwrap_or_default();
    let from: Vec<&str> = r.sources.iter().filter_map(|&i| names.get(i)).map(String::as_str).collect();
    [
        r.name.clone(),
        r.size.map(fmt::size).unwrap_or_default(),
        num(r.seeders),
        num(r.peers),
        category_text(&r.categories),
        r.published.map(|p| fmt::duration(now.saturating_sub(p).max(0) as u64)).unwrap_or_default(),
        from.join(", "),
    ]
}

/// The display order of `results` sorted by `col` (unknown values last either way; ties keep
/// the merged order, most seeders first).
pub fn sorted(results: &[SearchResult], names: &[String], col: Col, descending: bool) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..results.len()).collect();
    let text = |r: &SearchResult| match col {
        Col::Name => r.name.to_lowercase(),
        Col::Category => category_text(&r.categories).to_lowercase(),
        Col::Source => r.sources.iter().filter_map(|&i| names.get(i)).cloned().collect::<Vec<_>>().join(", "),
        _ => String::new(),
    };
    let number = |r: &SearchResult| -> Option<i64> {
        match col {
            Col::Size => r.size.map(|v| v.min(i64::MAX as u64) as i64),
            Col::Seeds => r.seeders.map(i64::from),
            Col::Peers => r.peers.map(i64::from),
            // the newest has the smallest age
            Col::Age => r.published.map(|p| -p),
            _ => None,
        }
    };
    let numeric = !matches!(col, Col::Name | Col::Category | Col::Source);
    rows.sort_by(|&a, &b| {
        let (ra, rb) = (&results[a], &results[b]);
        let o = if numeric {
            match (number(ra), number(rb)) {
                (Some(x), Some(y)) => {
                    let o = x.cmp(&y);
                    if descending { o.reverse() } else { o }
                }
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        } else {
            let o = text(ra).cmp(&text(rb));
            if descending { o.reverse() } else { o }
        };
        o.then(a.cmp(&b))
    });
    rows
}

/// What makes a result the same torrent across refreshes (the selection follows it).
fn result_key(r: &SearchResult) -> (String, Option<u64>) {
    if r.info_hash.is_empty() { (r.name.to_lowercase(), r.size) } else { (r.info_hash.clone(), None) }
}

/// Which sources a search covers: all the enabled ones, or one by its [`source_id`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    All,
    Source(String),
}

impl Pick {
    pub fn of(mode: &SearchMode) -> Pick {
        match mode {
            SearchMode::Source(id) => Pick::Source(id.clone()),
            _ => Pick::All,
        }
    }
}

struct Start {
    query: String,
    pick: Pick,
}

/// What Add hands back from its thread.
struct Fetched {
    name: String,
    label: String,
    result: Result<Download, String>,
}

type DownloadFn = fn(&SearchResult, &Source, Option<&FetchProxy>) -> Result<Download, String>;

#[cfg(test)]
thread_local! {
    /// The fetcher a test's windows search with (no network).
    pub static TEST_FETCHER: RefCell<Option<Arc<dyn Fetcher>>> = const { RefCell::new(None) };
}

#[cfg(not(test))]
thread_local! {
    static TEST_FETCHER: RefCell<Option<Arc<dyn Fetcher>>> = const { RefCell::new(None) };
}

struct Results {
    host: Box<dyn Host>,
    search: Option<Search>,
    download: DownloadFn,
    /// The Source box's sources after All sources (enabled ones).
    choices: Vec<Source>,
    /// The searched sources' labels ([`SearchResult::sources`] indexes them).
    names: Vec<String>,
    /// The list's rows: indices into the search's results.
    rows: Vec<usize>,
    sort: (Col, bool),
    /// The last Add's outcome, beside the count.
    note: String,
    /// The window's smallest size, and the client area's at the template's size.
    min: (i32, i32),
    start: Option<Start>,
}

/// Opens the Search Results window owned by `owner` and searches for `query` in `pick`
/// (nothing when `query` is empty); returns the window (null when it could not be made).
pub fn open(owner: HWND, host: Box<dyn Host>, query: &str, pick: Pick) -> HWND {
    let st = Box::new(Results {
        host,
        search: None,
        download: fetch_download,
        choices: Vec::new(),
        names: Vec::new(),
        rows: Vec::new(),
        sort: (Col::Seeds, true),
        note: String::new(),
        min: (0, 0),
        start: Some(Start { query: query.to_owned(), pick }),
    });
    let t = results_template();
    let raw = Box::into_raw(st);
    unsafe {
        let dlg = CreateDialogIndirectParamW(
            GetModuleHandleW(null()),
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(results_proc),
            raw as LPARAM,
        );
        if dlg.is_null() {
            drop(Box::from_raw(raw));
            return null_mut();
        }
        if !cfg!(test) {
            ShowWindow(dlg, SW_SHOW);
        }
        dlg
    }
}

/// Searches again in the open window `dlg` and brings it forward.
pub fn start_in(dlg: HWND, query: &str, pick: Pick) {
    let start = Start { query: query.to_owned(), pick };
    unsafe {
        SendMessageW(dlg, WM_SEARCH_START, 0, &start as *const Start as LPARAM);
        if !cfg!(test) {
            if IsIconic(dlg) != 0 {
                ShowWindow(dlg, SW_RESTORE);
            }
            SetForegroundWindow(dlg);
        }
    }
}

fn results_template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    const TAB: u32 = C | WS_TABSTOP;
    const PUSH: u32 = TAB | BS_PUSHBUTTON as u32;
    const DROPLIST: u32 = TAB | WS_VSCROLL | CBS_DROPDOWNLIST as u32;
    const LISTVIEW: Class = Class::Name("SysListView32");
    let mut t = Template::new(440, 260);
    t.resizable();
    // positions are set by `layout`; the order is the tab order
    t.item(EDIT, ID_QUERY, TAB | WS_BORDER | ES_AUTOHSCROLL as u32, [7, 7, 150, 13], "");
    t.item(COMBOBOX, ID_CATEGORY, DROPLIST, [160, 7, 80, 120], "");
    t.item(COMBOBOX, ID_SOURCE, DROPLIST, [244, 7, 80, 120], "");
    t.item(BUTTON, ID_GO, TAB | BS_DEFPUSHBUTTON as u32, [328, 7, 50, 14], tr!(WebSearch));
    t.item(BUTTON, ID_STOP, PUSH, [383, 7, 50, 14], tr!(ToolStop));
    let status = TAB | WS_BORDER | WS_VSCROLL | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32;
    t.item(EDIT, ID_STATUS, status, [7, 25, 426, 26], "");
    let list = TAB | WS_BORDER | LVS_REPORT | LVS_SHOWSELALWAYS | LVS_OWNERDATA;
    t.item(LISTVIEW, ID_LIST, list, [7, 55, 426, 178], "");
    t.item(BUTTON, ID_ADD, PUSH, [329, 239, 50, 14], tr!(WebAdd));
    t.item(BUTTON, IDCANCEL as u16, PUSH, [383, 239, 50, 14], tr!(BtnClose));
    t.item(STATIC, ID_COUNT, C | SS_NOPREFIX, [7, 242, 300, 9], "");
    t.finish()
}

unsafe fn item(dlg: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(dlg, id as i32) }
}

unsafe fn set_text(hwnd: HWND, s: &str) {
    let w = wide(s);
    unsafe { SetWindowTextW(hwnd, w.as_ptr()) };
}

unsafe fn checked(dlg: HWND, id: u16) -> bool {
    unsafe { send(item(dlg, id), BM_GETCHECK, 0, 0) == BST_CHECKED as isize }
}

unsafe fn set_check(dlg: HWND, id: u16, on: bool) {
    unsafe { send(item(dlg, id), BM_SETCHECK, if on { BST_CHECKED } else { BST_UNCHECKED } as usize, 0) };
}

/// Dialog units to pixels: (x, y).
unsafe fn dlu(dlg: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut rc = RECT { left: 0, top: 0, right: x, bottom: y };
    unsafe { MapDialogRect(dlg, &mut rc) };
    (rc.right, rc.bottom)
}

/// How wide a control's text is in its font (`&` mnemonics left out).
unsafe fn text_width(hwnd: HWND) -> i32 {
    unsafe {
        let text: Vec<u16> = window_text(hwnd).replace('&', "").encode_utf16().collect();
        let dc = GetDC(hwnd);
        let old = SelectObject(dc, send(hwnd, WM_GETFONT, 0, 0) as HGDIOBJ);
        let mut size: SIZE = zeroed();
        GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
        SelectObject(dc, old);
        ReleaseDC(hwnd, dc);
        size.cx
    }
}

/// A push button's width: 50 dialog units, or its text and some room.
unsafe fn button_width(dlg: HWND, id: u16) -> i32 {
    unsafe {
        let (min, pad) = (dlu(dlg, 50, 0).0, dlu(dlg, 12, 0).0);
        min.max(text_width(item(dlg, id)) + pad)
    }
}

unsafe fn place(dlg: HWND, id: u16, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        SetWindowPos(item(dlg, id), null_mut(), x, y, w.max(0), h.max(0), SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Lays the controls out over the client area: the query row on top, the status lines, the
/// list taking the rest, Add and Close at the bottom right with the count on their left.
unsafe fn layout(dlg: HWND) {
    unsafe {
        let mut rc: RECT = zeroed();
        GetClientRect(dlg, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        let (m, mv) = dlu(dlg, 7, 7);
        let (gap, gapv) = dlu(dlg, 4, 4);
        let (_, row) = dlu(dlg, 0, 14);
        let (_, edit_h) = dlu(dlg, 0, 13);
        let (_, drop_h) = dlu(dlg, 0, 120);
        let (_, status_h) = dlu(dlg, 0, 26);
        let (_, label_h) = dlu(dlg, 0, 9);
        let (cat_w, src_w) = (dlu(dlg, 74, 0).0, dlu(dlg, 96, 0).0);
        // the top row, from the right
        let mut x = w - m;
        let stop = button_width(dlg, ID_STOP);
        x -= stop;
        place(dlg, ID_STOP, x, mv, stop, row);
        let go = button_width(dlg, ID_GO);
        x -= gap + go;
        place(dlg, ID_GO, x, mv, go, row);
        x -= gap + src_w;
        place(dlg, ID_SOURCE, x, mv, src_w, drop_h);
        x -= gap + cat_w;
        place(dlg, ID_CATEGORY, x, mv, cat_w, drop_h);
        place(dlg, ID_QUERY, m, mv + (row - edit_h) / 2, x - gap - m, edit_h);
        let status_top = mv + row + gapv;
        place(dlg, ID_STATUS, m, status_top, w - 2 * m, status_h);
        // the bottom row, from the right
        let bottom = h - mv - row;
        let close = button_width(dlg, IDCANCEL as u16);
        let mut x = w - m - close;
        place(dlg, IDCANCEL as u16, x, bottom, close, row);
        let add = button_width(dlg, ID_ADD);
        x -= gap + add;
        place(dlg, ID_ADD, x, bottom, add, row);
        place(dlg, ID_COUNT, m, bottom + (row - label_h) / 2, x - gap - m, label_h);
        let list_top = status_top + status_h + gapv;
        place(dlg, ID_LIST, m, list_top, w - 2 * m, bottom - gapv - list_top);
        InvalidateRect(dlg, null(), 1);
    }
}

unsafe extern "system" fn results_proc(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            results_init(dlg, &mut *(lp as *mut Results));
            return 0; // the focus was set
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut Results;
        if st.is_null() {
            return 0;
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(dlg, DWLP_USER, 0);
            drop(Box::from_raw(st));
            return 0;
        }
        let st = &mut *st;
        match msg {
            WM_COMMAND => results_command(dlg, st, loword(wp), hiword(wp) as u32) as isize,
            WM_NOTIFY => {
                let nm = &*(lp as *const NMHDR);
                if nm.idFrom as u16 != ID_LIST {
                    return 0;
                }
                match nm.code {
                    NM_CUSTOMDRAW => {
                        let r = controls::selection_custom_draw(&mut *(lp as *mut NMLVCUSTOMDRAW));
                        SetWindowLongPtrW(dlg, DWLP_MSGRESULT, r);
                        1
                    }
                    LVN_GETDISPINFOW => {
                        let di = &mut *(lp as *mut NMLVDISPINFOW);
                        if di.item.mask & LVIF_TEXT != 0 {
                            let text = cell(st, di.item.iItem as usize, di.item.iSubItem as usize);
                            copy_to(di.item.pszText, di.item.cchTextMax, &text);
                        }
                        0
                    }
                    LVN_COLUMNCLICK => {
                        let lv = &*(lp as *const NMLISTVIEW);
                        if let Some(&(col, ..)) = COLUMNS.get(lv.iSubItem as usize) {
                            let desc = if st.sort.0 == col { !st.sort.1 } else { col.first_descending() };
                            sort_by(dlg, st, col, desc);
                        }
                        0
                    }
                    LVN_ITEMCHANGED => {
                        update_buttons(dlg, st);
                        0
                    }
                    NM_DBLCLK => {
                        add_selected(dlg, st);
                        0
                    }
                    _ => 0,
                }
            }
            WM_SIZE => {
                layout(dlg);
                0
            }
            WM_GETMINMAXINFO => {
                let mm = &mut *(lp as *mut MINMAXINFO);
                if st.min.0 > 0 {
                    mm.ptMinTrackSize = POINT { x: st.min.0, y: st.min.1 };
                }
                0
            }
            WM_APP_SEARCH => {
                if st.search.as_mut().is_some_and(Search::collect) {
                    refill(dlg, st);
                }
                1
            }
            WM_SEARCH_SOURCES => {
                fill_choices(dlg, st, None);
                1
            }
            WM_SEARCH_START => {
                let start = &*(lp as *const Start);
                begin(dlg, st, &start.query, &start.pick);
                1
            }
            WM_SEARCH_FETCHED => {
                let f = *Box::from_raw(lp as *mut Fetched);
                match f.result {
                    Ok(d) => {
                        st.note = tr!(SearchAdded, name = f.name);
                        show_count(dlg, st);
                        st.host.add(dlg, d, &f.name);
                    }
                    Err(e) => {
                        st.note = tr!(SearchFailed, source = f.label, error = e);
                        show_count(dlg, st);
                    }
                }
                1
            }
            WM_DESTROY => {
                st.host.set_results_window(null_mut());
                0
            }
            _ => 0,
        }
    }
}

unsafe fn results_init(dlg: HWND, st: &mut Results) {
    unsafe {
        set_text(dlg, tr!(SearchTitle));
        st.host.set_results_window(dlg);
        let target = dlg as isize;
        // runs on a worker thread: only post a message
        let notify = move || {
            PostMessageW(target as HWND, WM_APP_SEARCH, 0, 0);
        };
        let fetcher = TEST_FETCHER.with(|f| f.borrow().clone());
        st.search = Some(match fetcher {
            Some(f) => Search::with_fetcher(f, notify),
            None => Search::new(notify),
        });
        let lv = item(dlg, ID_LIST);
        let cols: Vec<(&str, i32, bool)> =
            COLUMNS.iter().map(|&(c, w, right)| (c.title(), dlu(dlg, w, 0).0, right)).collect();
        controls::set_columns(lv, &cols);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_LABELTIP | LVS_EX_DOUBLEBUFFER;
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        let cat = item(dlg, ID_CATEGORY);
        let all = wide(tr!(SearchAllCategories));
        send(cat, CB_ADDSTRING, 0, all.as_ptr() as LPARAM);
        for (_, name) in main_categories() {
            let w = wide(name);
            send(cat, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        send(cat, CB_SETCURSEL, 0, 0);
        send(item(dlg, ID_QUERY), EM_SETLIMITTEXT, MAX_QUERY, 0);
        // the smallest size: the window less half its list
        let mut wr: RECT = zeroed();
        GetWindowRect(dlg, &mut wr);
        let (min_w, min_h) = dlu(dlg, 300, 150);
        let mut cr: RECT = zeroed();
        GetClientRect(dlg, &mut cr);
        st.min = ((wr.right - wr.left) - cr.right + min_w, (wr.bottom - wr.top) - cr.bottom + min_h);
        layout(dlg);
        let start = st.start.take();
        let pick = start.as_ref().map(|s| s.pick.clone());
        fill_choices(dlg, st, pick.as_ref());
        refill(dlg, st);
        match start {
            Some(s) if !s.query.trim().is_empty() => {
                begin(dlg, st, &s.query, &s.pick);
                SetFocus(lv);
            }
            _ => {
                SetFocus(item(dlg, ID_QUERY));
            }
        }
    }
}

/// The Source box: All sources, then each enabled source; `pick` is chosen (else what was).
unsafe fn fill_choices(dlg: HWND, st: &mut Results, pick: Option<&Pick>) {
    unsafe {
        let combo = item(dlg, ID_SOURCE);
        let was = match pick {
            Some(p) => p.clone(),
            None => {
                let i = send(combo, CB_GETCURSEL, 0, 0);
                match usize::try_from(i).ok().filter(|&i| i > 0).and_then(|i| st.choices.get(i - 1)) {
                    Some(s) => Pick::Source(source_id(s)),
                    None => Pick::All,
                }
            }
        };
        st.choices = st.host.sources().into_iter().filter(|s| s.enabled).collect();
        send(combo, CB_RESETCONTENT, 0, 0);
        let all = wide(tr!(SearchAllSources));
        send(combo, CB_ADDSTRING, 0, all.as_ptr() as LPARAM);
        let entries = mode_menu(&st.choices, &SearchMode::Filter);
        // the menu's names (the twins told apart), without Filter and All
        for e in &entries[1..entries.len() - 1] {
            let w = wide(&e.text);
            send(combo, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        let sel = match &was {
            Pick::All => 0,
            Pick::Source(id) => st.choices.iter().position(|s| source_id(s) == *id).map_or(0, |i| i + 1),
        };
        send(combo, CB_SETCURSEL, sel, 0);
    }
}

/// Starts searching for `query` in `pick` (the boxes show both).
unsafe fn begin(dlg: HWND, st: &mut Results, query: &str, pick: &Pick) {
    unsafe {
        set_text(item(dlg, ID_QUERY), query);
        fill_choices(dlg, st, Some(pick));
        go(dlg, st);
    }
}

/// The Search button: the query in the boxes' category and source.
unsafe fn go(dlg: HWND, st: &mut Results) {
    unsafe {
        let text: String = window_text(item(dlg, ID_QUERY)).trim().chars().take(MAX_QUERY).collect();
        if text.is_empty() {
            SetFocus(item(dlg, ID_QUERY));
            return;
        }
        let cat = send(item(dlg, ID_CATEGORY), CB_GETCURSEL, 0, 0);
        let categories: Vec<u32> = usize::try_from(cat)
            .ok()
            .filter(|&i| i > 0)
            .and_then(|i| main_categories().get(i - 1).map(|&(id, _)| vec![id]))
            .unwrap_or_default();
        let src = send(item(dlg, ID_SOURCE), CB_GETCURSEL, 0, 0);
        let picked: Vec<Source> = match usize::try_from(src).ok().filter(|&i| i > 0) {
            Some(i) => st.choices.get(i - 1).cloned().into_iter().collect(),
            None => st.host.sources(),
        };
        st.note.clear();
        if !picked.iter().any(|s| s.enabled && !s.url.trim().is_empty()) {
            if let Some(s) = st.search.as_mut() {
                s.stop();
            }
            refill(dlg, st);
            set_text(item(dlg, ID_STATUS), tr!(SearchNoSources));
            return;
        }
        let proxy = st.host.proxy();
        if let Some(s) = st.search.as_mut() {
            s.start(&picked, Query { text, categories }, proxy.as_ref());
        }
        controls::set_state(item(dlg, ID_LIST), None, false, false);
        refill(dlg, st);
    }
}

unsafe fn sort_by(dlg: HWND, st: &mut Results, col: Col, desc: bool) {
    unsafe {
        st.sort = (col, desc);
        refill(dlg, st);
    }
}

/// The list, the status lines, the count and the buttons after the search moved; the
/// selected results stay selected.
unsafe fn refill(dlg: HWND, st: &mut Results) {
    unsafe {
        let lv = item(dlg, ID_LIST);
        let Some(s) = st.search.as_ref() else { return };
        let results = s.results();
        let keep: Vec<(String, Option<u64>)> = controls::selected(lv)
            .into_iter()
            .filter_map(|row| st.rows.get(row).and_then(|&i| results.get(i)))
            .map(result_key)
            .collect();
        st.names = s.sources().iter().map(|x| x.source.label()).collect();
        st.rows = sorted(results, &st.names, st.sort.0, st.sort.1);
        let lines: Vec<String> = s
            .sources()
            .iter()
            .zip(&st.names)
            .map(|(x, label)| match &x.status {
                SourceStatus::Searching => tr!(SearchSearching, source = label),
                SourceStatus::Done(n) => tr!(SearchFound, source = label, count = n),
                SourceStatus::Failed(e) => tr!(SearchFailed, source = label, error = e),
                SourceStatus::Stopped => tr!(SearchStopped, source = label),
            })
            .collect();
        let status = item(dlg, ID_STATUS);
        let text = lines.join("\r\n");
        if window_text(status) != text {
            set_text(status, &text);
        }
        controls::set_state(lv, None, false, false);
        controls::set_count(lv, st.rows.len());
        for (row, &i) in st.rows.iter().enumerate() {
            if keep.contains(&result_key(&results[i])) {
                controls::set_state(lv, Some(row), true, false);
            }
        }
        let col = COLUMNS.iter().position(|&(c, ..)| c == st.sort.0);
        controls::set_sort_arrow(lv, COLUMNS.len(), col, st.sort.1);
        InvalidateRect(lv, null(), 0);
        show_count(dlg, st);
        update_buttons(dlg, st);
    }
}

unsafe fn show_count(dlg: HWND, st: &Results) {
    unsafe {
        let n = st.search.as_ref().map_or(0, |s| s.results().len());
        let mut text = tr!(SearchResultCount, count = n);
        if !st.note.is_empty() {
            text = format!("{text}    {}", st.note);
        }
        set_text(item(dlg, ID_COUNT), &text);
    }
}

unsafe fn update_buttons(dlg: HWND, st: &Results) {
    unsafe {
        let running = st.search.as_ref().is_some_and(Search::is_running);
        EnableWindow(item(dlg, ID_STOP), running as i32);
        let any = controls::selected_count(item(dlg, ID_LIST)) > 0;
        EnableWindow(item(dlg, ID_ADD), any as i32);
    }
}

/// A cell of the list's row `row`.
fn cell(st: &Results, row: usize, col: usize) -> String {
    let Some(s) = st.search.as_ref() else { return String::new() };
    let Some(r) = st.rows.get(row).and_then(|&i| s.results().get(i)) else { return String::new() };
    cells(r, &st.names, unix_now()).get(col).cloned().unwrap_or_default()
}

/// Add: each selected result fetched on a thread of its own (its magnet, else its link), then
/// handed to the window's add path ([`WM_SEARCH_FETCHED`]).
unsafe fn add_selected(dlg: HWND, st: &mut Results) {
    unsafe {
        let Some(s) = st.search.as_ref() else { return };
        let picked: Vec<(SearchResult, Source)> = controls::selected(item(dlg, ID_LIST))
            .into_iter()
            .filter_map(|row| st.rows.get(row).and_then(|&i| s.results().get(i)))
            .filter_map(|r| s.source_of(r).map(|src| (r.clone(), src.clone())))
            .collect();
        if picked.is_empty() {
            return;
        }
        let proxy = st.host.proxy();
        let download = st.download;
        let target = dlg as isize;
        for (r, src) in picked {
            let proxy = proxy.clone();
            std::thread::spawn(move || {
                let result = download(&r, &src, proxy.as_ref()).map_err(|e| src.redact(&e));
                let f = Box::into_raw(Box::new(Fetched { name: r.name.clone(), label: src.label(), result }));
                if PostMessageW(target as HWND, WM_SEARCH_FETCHED, 0, f as LPARAM) == 0 {
                    drop(Box::from_raw(f));
                }
            });
        }
    }
}

unsafe fn results_command(dlg: HWND, st: &mut Results, id: u16, code: u32) -> bool {
    unsafe {
        match id {
            // Enter: Add in the list, Search anywhere else
            ID_GO if GetFocus() == item(dlg, ID_LIST) => add_selected(dlg, st),
            ID_GO => go(dlg, st),
            ID_STOP => {
                if let Some(s) = st.search.as_mut() {
                    s.stop();
                }
                refill(dlg, st);
            }
            ID_ADD => add_selected(dlg, st),
            ID_CATEGORY | ID_SOURCE if code == CBN_SELCHANGE => {}
            _ if id == IDCANCEL as u16 || id == IDOK as u16 => {
                DestroyWindow(dlg);
            }
            _ => return false,
        }
        true
    }
}

// ---- the Search Sources dialog ----------------------------------------------------------

const ID_SOURCES: u16 = 200;
const ID_SRC_ADD: u16 = 201;
const ID_SRC_EDIT: u16 = 202;
const ID_SRC_REMOVE: u16 = 203;
const ID_SRC_TEST: u16 = 204;
const ID_SRC_STATUS: u16 = 205;

type CapsFn = fn(&Source, Option<&FetchProxy>) -> Result<Vec<Category>, SearchError>;

struct Sources<'a> {
    host: &'a mut dyn Host,
    caps: CapsFn,
    /// The list is being filled: its notifications are not the user's.
    loading: bool,
}

/// What a Test hands back from its thread.
struct Tested {
    label: String,
    /// Categories and subcategories, or the error with the key removed.
    result: Result<usize, String>,
}

/// Options > Search Sources (the search box's Manage Sources...): modal over `owner`.
pub fn manage(owner: HWND, host: &mut dyn Host) {
    let mut st = Sources { host, caps: fetch_caps, loading: false };
    let t = sources_template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(sources_proc),
            &mut st as *mut Sources<'_> as LPARAM,
        );
    }
}

fn sources_template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    const TAB: u32 = C | WS_TABSTOP;
    const PUSH: u32 = TAB | BS_PUSHBUTTON as u32;
    const LISTVIEW: Class = Class::Name("SysListView32");
    let mut t = Template::new(360, 200);
    let list = TAB | WS_BORDER | LVS_REPORT | LVS_SHOWSELALWAYS | LVS_SINGLESEL;
    t.item(LISTVIEW, ID_SOURCES, list, [7, 7, 290, 152], "");
    t.item(BUTTON, ID_SRC_ADD, PUSH, [303, 7, 50, 14], tr!(RssFeedsAdd));
    t.item(BUTTON, ID_SRC_EDIT, PUSH, [303, 25, 50, 14], tr!(RssFeedsEdit));
    t.item(BUTTON, ID_SRC_REMOVE, PUSH, [303, 43, 50, 14], tr!(RssFeedsRemove));
    t.item(BUTTON, ID_SRC_TEST, PUSH, [303, 67, 50, 14], tr!(SearchTest));
    t.item(STATIC, ID_SRC_STATUS, C | SS_NOPREFIX, [7, 163, 346, 18], "");
    t.item(BUTTON, IDOK as u16, TAB | BS_DEFPUSHBUTTON as u32, [303, 181, 50, 14], tr!(BtnClose));
    t.finish()
}

unsafe extern "system" fn sources_proc(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            sources_init(dlg, &mut *(lp as *mut Sources<'_>));
            crate::fit::fit_texts(dlg, |_| Some(0));
            return 1;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut Sources<'_>;
        if st.is_null() {
            return 0;
        }
        let st = &mut *st;
        match msg {
            WM_COMMAND => sources_command(dlg, st, loword(wp)) as isize,
            WM_NOTIFY => {
                let nm = &*(lp as *const NMHDR);
                if nm.idFrom as u16 != ID_SOURCES {
                    return 0;
                }
                match nm.code {
                    NM_CUSTOMDRAW => {
                        let r = controls::selection_custom_draw(&mut *(lp as *mut NMLVCUSTOMDRAW));
                        SetWindowLongPtrW(dlg, DWLP_MSGRESULT, r);
                        return 1;
                    }
                    LVN_ITEMCHANGED if !st.loading => {
                        let lv = &*(lp as *const NMLISTVIEW);
                        if lv.uChanged & LVIF_STATE != 0 && lv.iItem >= 0 {
                            let row = lv.iItem as usize;
                            if (lv.uNewState ^ lv.uOldState) & LVIS_STATEIMAGEMASK != 0
                                && lv.uOldState & LVIS_STATEIMAGEMASK != 0
                            {
                                let on = lv_checked(item(dlg, ID_SOURCES), row);
                                set_enabled(dlg, st, row, on);
                            }
                            source_selected(dlg);
                        }
                    }
                    NM_DBLCLK => edit_source(dlg, st),
                    _ => {}
                }
                0
            }
            WM_SEARCH_SOURCES => {
                fill_sources(dlg, st, None);
                1
            }
            WM_SEARCH_TESTED => {
                let t = *Box::from_raw(lp as *mut Tested);
                let text = match t.result {
                    Ok(n) => tr!(SearchTestOk, source = t.label, count = n),
                    Err(e) => tr!(SearchFailed, source = t.label, error = e),
                };
                set_text(item(dlg, ID_SRC_STATUS), &text);
                1
            }
            WM_DESTROY => {
                st.host.set_sources_dialog(null_mut());
                0
            }
            _ => 0,
        }
    }
}

unsafe fn sources_init(dlg: HWND, st: &mut Sources<'_>) {
    unsafe {
        set_text(dlg, tr!(SearchSourcesTitle));
        st.host.set_sources_dialog(dlg);
        let lv = item(dlg, ID_SOURCES);
        let cols = [(tr!(ColName), dlu(dlg, 100, 0).0, false), (tr!(ColUrl), dlu(dlg, 180, 0).0, false)];
        controls::set_columns(lv, &cols);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_LABELTIP | LVS_EX_DOUBLEBUFFER | LVS_EX_CHECKBOXES;
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        fill_sources(dlg, st, Some(0));
    }
}

/// The list again, `select` picked (else the row that was).
unsafe fn fill_sources(dlg: HWND, st: &mut Sources<'_>, select: Option<usize>) {
    unsafe {
        let lv = item(dlg, ID_SOURCES);
        let select = select.or_else(|| controls::first_selected(lv));
        let sources = st.host.sources();
        st.loading = true;
        send(lv, LVM_DELETEALLITEMS, 0, 0);
        for (i, s) in sources.iter().enumerate() {
            lv_add(lv, i, &[&s.label(), &source_id(s)]);
            lv_set_checked(lv, i, s.enabled);
        }
        if let Some(row) = select.filter(|_| !sources.is_empty()) {
            let row = row.min(sources.len() - 1);
            controls::set_state(lv, Some(row), true, true);
            send(lv, LVM_ENSUREVISIBLE, row, 0);
        }
        st.loading = false;
        source_selected(dlg);
    }
}

unsafe fn source_selected(dlg: HWND) {
    unsafe {
        let any = controls::first_selected(item(dlg, ID_SOURCES)).is_some();
        for id in [ID_SRC_EDIT, ID_SRC_REMOVE, ID_SRC_TEST] {
            EnableWindow(item(dlg, id), any as i32);
        }
    }
}

unsafe fn lv_add(lv: HWND, row: usize, cells: &[&str]) {
    unsafe {
        for (i, c) in cells.iter().enumerate() {
            let mut text = wide(c);
            let mut it: LVITEMW = zeroed();
            it.mask = LVIF_TEXT;
            it.iItem = row as i32;
            it.iSubItem = i as i32;
            it.pszText = text.as_mut_ptr();
            if i == 0 {
                send(lv, LVM_INSERTITEMW, 0, &it as *const _ as LPARAM);
            } else {
                send(lv, LVM_SETITEMTEXTW, row, &it as *const _ as LPARAM);
            }
        }
    }
}

/// The check box of a row (`LVS_EX_CHECKBOXES`: state image 2 is on, 1 off).
unsafe fn lv_checked(lv: HWND, row: usize) -> bool {
    unsafe {
        (send(lv, LVM_GETITEMSTATE, row, LVIS_STATEIMAGEMASK as isize) as u32 & LVIS_STATEIMAGEMASK) >> 12
            == 2
    }
}

unsafe fn lv_set_checked(lv: HWND, row: usize, on: bool) {
    unsafe {
        let mut it: LVITEMW = zeroed();
        it.stateMask = LVIS_STATEIMAGEMASK;
        it.state = (if on { 2 } else { 1 }) << 12;
        send(lv, LVM_SETITEMSTATE, row, &it as *const _ as LPARAM);
    }
}

unsafe fn selected_source(dlg: HWND) -> Option<usize> {
    unsafe { controls::first_selected(item(dlg, ID_SOURCES)) }
}

unsafe fn set_enabled(_dlg: HWND, st: &mut Sources<'_>, row: usize, on: bool) {
    let mut sources = st.host.sources();
    let Some(s) = sources.get_mut(row) else { return };
    if s.enabled != on {
        s.enabled = on;
        st.host.store_sources(&sources);
    }
}

unsafe fn sources_command(dlg: HWND, st: &mut Sources<'_>, id: u16) -> bool {
    unsafe {
        match id {
            ID_SRC_ADD => {
                let form = SourceForm::new(None);
                if let Some(form) = source_dialog(dlg, tr!(SearchAddSource), form) {
                    add_source(dlg, st, &form);
                }
            }
            ID_SRC_EDIT => edit_source(dlg, st),
            ID_SRC_REMOVE => {
                let Some(row) = selected_source(dlg) else { return true };
                let name = st.host.sources().get(row).map(Source::label).unwrap_or_default();
                if dialogs::confirm(dlg, &tr!(AskRemoveSource, name)) || cfg!(test) {
                    remove_source(dlg, st, row);
                }
            }
            ID_SRC_TEST => test_source(dlg, st),
            _ if id == IDOK as u16 || id == IDCANCEL as u16 => {
                EndDialog(dlg, IDOK as isize);
            }
            _ => return false,
        }
        true
    }
}

unsafe fn add_source(dlg: HWND, st: &mut Sources<'_>, form: &SourceForm) {
    unsafe {
        let mut sources = st.host.sources();
        if sources.len() >= patorrent_frontend::search::source::MAX_SOURCES {
            return;
        }
        let mut s = Source::default();
        form.apply(&mut s);
        sources.push(s);
        st.host.store_sources(&sources);
        fill_sources(dlg, st, Some(sources.len() - 1));
    }
}

unsafe fn edit_source(dlg: HWND, st: &mut Sources<'_>) {
    unsafe {
        let Some(row) = selected_source(dlg) else { return };
        let Some(s) = st.host.sources().get(row).cloned() else { return };
        let Some(form) = source_dialog(dlg, tr!(SearchEditSource), SourceForm::new(Some(&s))) else {
            return;
        };
        store_edit(dlg, st, row, &form);
    }
}

/// Stores the edit dialog's `form` into source `row`.
unsafe fn store_edit(dlg: HWND, st: &mut Sources<'_>, row: usize, form: &SourceForm) {
    unsafe {
        let mut sources = st.host.sources();
        let Some(s) = sources.get_mut(row) else { return };
        form.apply(s);
        st.host.store_sources(&sources);
        fill_sources(dlg, st, Some(row));
    }
}

unsafe fn remove_source(dlg: HWND, st: &mut Sources<'_>, row: usize) {
    unsafe {
        let mut sources = st.host.sources();
        if row >= sources.len() {
            return;
        }
        sources.remove(row);
        st.host.store_sources(&sources);
        fill_sources(dlg, st, Some(row));
    }
}

/// Test: the source's caps (`t=caps`) on a thread; the answer comes back as
/// [`WM_SEARCH_TESTED`].
unsafe fn test_source(dlg: HWND, st: &mut Sources<'_>) {
    unsafe {
        let Some(row) = selected_source(dlg) else { return };
        let Some(source) = st.host.sources().get(row).cloned() else { return };
        let label = source.label();
        set_text(item(dlg, ID_SRC_STATUS), &tr!(SearchTesting, source = label));
        let (proxy, caps, target) = (st.host.proxy(), st.caps, dlg as isize);
        std::thread::spawn(move || {
            let result = caps(&source, proxy.as_ref())
                .map(|cats| cats.len() + cats.iter().map(|c| c.subcats.len()).sum::<usize>())
                .map_err(|e| source.redact(&e.to_string()));
            let t = Box::into_raw(Box::new(Tested { label, result }));
            if PostMessageW(target as HWND, WM_SEARCH_TESTED, 0, t as LPARAM) == 0 {
                drop(Box::from_raw(t));
            }
        });
    }
}

// ---- the Add / Edit Search Source dialog ------------------------------------------------

const ID_NAME: u16 = 300;
const ID_URL: u16 = 301;
const ID_HINT: u16 = 302;
const ID_KEY: u16 = 303;
const ID_ENABLED: u16 = 304;

/// The edit dialog's fields. The URL shows with its key masked and the key box starts empty:
/// an unchanged URL keeps the stored one, an empty key box the stored key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceForm {
    pub name: String,
    /// The URL as shown (masked), then as typed.
    pub url: String,
    /// What the URL box showed, to tell an edit from the masked text.
    pub shown_url: String,
    /// Typed; empty keeps the stored key.
    pub key: String,
    /// Whether a key is stored (the key box's grey hint says it stays).
    pub has_key: bool,
    pub enabled: bool,
}

impl SourceForm {
    pub fn new(s: Option<&Source>) -> SourceForm {
        match s {
            Some(s) => {
                let url = source_id(s);
                SourceForm {
                    name: s.name.clone(),
                    shown_url: url.clone(),
                    url,
                    key: String::new(),
                    has_key: !s.api_key.trim().is_empty(),
                    enabled: s.enabled,
                }
            }
            None => SourceForm { enabled: true, ..SourceForm::default() },
        }
    }

    /// Writes the fields into `s`: a URL left as shown and an empty key keep the stored ones.
    pub fn apply(&self, s: &mut Source) {
        s.name = self.name.trim().to_owned();
        let url = self.url.trim();
        if url != self.shown_url.trim() || self.shown_url.is_empty() {
            s.url = url.to_owned();
        }
        if !self.key.trim().is_empty() {
            s.api_key = self.key.trim().to_owned();
        }
        s.enabled = self.enabled;
    }
}

/// A Torznab URL the search can use: `http://` or `https://` with a host.
pub fn valid_url(url: &str) -> bool {
    let url = url.trim();
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    matches!(scheme.as_deref(), Some("http" | "https")) && url_host(url).is_some()
}

struct FormState {
    title: String,
    form: SourceForm,
    result: Option<SourceForm>,
}

/// Shows the Add / Edit Search Source dialog; the fields on OK.
fn source_dialog(owner: HWND, title: &str, form: SourceForm) -> Option<SourceForm> {
    let mut st = FormState { title: title.to_owned(), form, result: None };
    let t = source_template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(source_proc),
            &mut st as *mut FormState as LPARAM,
        );
    }
    st.result
}

fn source_template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    const TAB: u32 = C | WS_TABSTOP;
    const LINE: u32 = TAB | WS_BORDER | ES_AUTOHSCROLL as u32;
    let mut t = Template::new(300, 150);
    t.item(STATIC, 0xffff, C, [7, 7, 286, 9], tr!(SearchSourceName));
    t.item(EDIT, ID_NAME, LINE, [7, 18, 286, 13], "");
    t.item(STATIC, 0xffff, C, [7, 36, 286, 9], tr!(SearchSourceUrl));
    t.item(EDIT, ID_URL, LINE, [7, 47, 286, 13], "");
    t.item(STATIC, ID_HINT, C | SS_NOPREFIX, [7, 63, 286, 18], tr!(SearchSourceUrlHint));
    t.item(STATIC, 0xffff, C, [7, 86, 286, 9], tr!(SearchSourceKey));
    t.item(EDIT, ID_KEY, LINE | ES_PASSWORD as u32, [7, 97, 286, 13], "");
    t.item(BUTTON, ID_ENABLED, TAB | BS_AUTOCHECKBOX as u32, [7, 115, 286, 10], tr!(SearchSourceEnabled));
    t.item(BUTTON, IDOK as u16, TAB | BS_DEFPUSHBUTTON as u32, [189, 131, 50, 14], tr!(BtnOk));
    t.item(BUTTON, IDCANCEL as u16, TAB | BS_PUSHBUTTON as u32, [243, 131, 50, 14], tr!(BtnCancel));
    t.finish()
}

unsafe extern "system" fn source_proc(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            let st = &*(lp as *const FormState);
            set_text(dlg, &st.title);
            set_text(item(dlg, ID_NAME), &st.form.name);
            set_text(item(dlg, ID_URL), &st.form.url);
            if st.form.has_key {
                let cue = wide(tr!(SearchSourceKeyKept));
                send(item(dlg, ID_KEY), EM_SETCUEBANNER, 1, cue.as_ptr() as LPARAM);
            }
            set_check(dlg, ID_ENABLED, st.form.enabled);
            crate::fit::fit_texts(dlg, |_| Some(0));
            return 1;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut FormState;
        if st.is_null() || msg != WM_COMMAND {
            return 0;
        }
        let st = &mut *st;
        match loword(wp) {
            id if id == IDOK as u16 => {
                let url = window_text(item(dlg, ID_URL)).trim().to_owned();
                if !valid_url(&url) {
                    dialogs::error(dlg, tr!(SearchNoUrl));
                    SetFocus(item(dlg, ID_URL));
                    return 1;
                }
                st.result = Some(SourceForm {
                    name: window_text(item(dlg, ID_NAME)).trim().to_owned(),
                    url,
                    key: window_text(item(dlg, ID_KEY)).trim().to_owned(),
                    enabled: checked(dlg, ID_ENABLED),
                    ..st.form.clone()
                });
                EndDialog(dlg, IDOK as isize);
                1
            }
            id if id == IDCANCEL as u16 => {
                EndDialog(dlg, IDCANCEL as isize);
                1
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    //! The windows built from their templates, hidden, on a host of their own and a fake
    //! fetcher: no network.

    use std::mem::size_of;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;

    use super::*;
    use crate::util::from_wide;

    #[derive(Default)]
    struct Shared {
        sources: Vec<Source>,
        stores: usize,
        adds: Vec<(String, String)>,
        results_window: Option<HWND>,
        sources_dialog: Option<HWND>,
    }

    struct TestHost(Rc<RefCell<Shared>>);

    impl Host for TestHost {
        fn sources(&mut self) -> Vec<Source> {
            self.0.borrow().sources.clone()
        }
        fn store_sources(&mut self, sources: &[Source]) {
            let mut s = self.0.borrow_mut();
            s.sources = sources.to_vec();
            s.stores += 1;
        }
        fn proxy(&mut self) -> Option<FetchProxy> {
            None
        }
        fn add(&mut self, _: HWND, d: Download, name: &str) {
            let what = match d {
                Download::Magnet(m) => m,
                Download::Torrent(b) => format!("{} bytes", b.len()),
            };
            self.0.borrow_mut().adds.push((name.to_owned(), what));
        }
        fn set_results_window(&mut self, dlg: HWND) {
            self.0.borrow_mut().results_window = (!dlg.is_null()).then_some(dlg);
        }
        fn set_sources_dialog(&mut self, dlg: HWND) {
            self.0.borrow_mut().sources_dialog = (!dlg.is_null()).then_some(dlg);
        }
    }

    const KEY: &str = "s3cr3tk3y";

    fn source(name: &str, url: &str, enabled: bool) -> Source {
        Source { name: name.into(), url: url.into(), api_key: KEY.into(), enabled }
    }

    fn sources() -> Vec<Source> {
        vec![
            source("Prowlarr", "http://127.0.0.1:9696/1/api", true),
            source("", "http://jackett.lan:9117/api/v2.0/indexers/all/results/torznab/", true),
            source("Off", "http://127.0.0.1:9696/3/api?apikey=s3cr3tk3y", false),
        ]
    }

    fn result(name: &str, seeds: Option<u32>, size: u64, hash: &str, published: i64) -> SearchResult {
        SearchResult {
            name: name.into(),
            size: Some(size),
            seeders: seeds,
            peers: seeds.map(|s| s + 1),
            info_hash: hash.into(),
            categories: vec![5040],
            published: Some(published),
            magnet: format!("magnet:?xt=urn:btih:{hash}&dn={name}"),
            ..SearchResult::default()
        }
    }

    /// Answers each source with results named after it; a source whose URL has `fail` fails
    /// with a message that carries its key.
    struct Fake;

    impl Fetcher for Fake {
        fn search(
            &self,
            source: &Source,
            query: &Query,
            _: Option<&FetchProxy>,
        ) -> Result<Vec<SearchResult>, SearchError> {
            if source.url.contains("fail") {
                return Err(SearchError::Fetch(format!("refused: {}?apikey={KEY}", source.url)));
            }
            let now = unix_now();
            Ok(vec![
                result(
                    &format!("{} alpha", query.text),
                    Some(5),
                    2_000_000,
                    "aa".repeat(20).as_str(),
                    now - 60,
                ),
                result(
                    &format!("{} Bravo", query.text),
                    Some(50),
                    1_000,
                    "bb".repeat(20).as_str(),
                    now - 7200,
                ),
                result(
                    &format!("{} charlie", query.text),
                    None,
                    9_000_000_000,
                    "cc".repeat(20).as_str(),
                    now,
                ),
            ])
        }
    }

    fn init_controls() {
        unsafe {
            let icc = INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_LISTVIEW_CLASSES | ICC_WIN95_CLASSES,
            };
            InitCommonControlsEx(&icc);
        }
    }

    /// Pumps this thread's messages until `done` or 10 s.
    fn pump_until(mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            unsafe {
                let mut msg: MSG = zeroed();
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    fn rows(lv: HWND) -> Vec<Vec<String>> {
        let n = unsafe { send(lv, LVM_GETITEMCOUNT, 0, 0) } as usize;
        (0..n)
            .map(|row| {
                (0..COLUMNS.len())
                    .map(|col| {
                        let mut buf = [0u16; 260];
                        unsafe {
                            let mut it: LVITEMW = zeroed();
                            it.iSubItem = col as i32;
                            it.pszText = buf.as_mut_ptr();
                            it.cchTextMax = buf.len() as i32;
                            send(lv, LVM_GETITEMTEXTW, row, &mut it as *mut _ as LPARAM);
                        }
                        from_wide(&buf)
                    })
                    .collect()
            })
            .collect()
    }

    fn click(dlg: HWND, id: u16) {
        unsafe {
            let wp = (BN_CLICKED as usize) << 16 | id as usize;
            SendMessageW(dlg, WM_COMMAND, wp, item(dlg, id) as LPARAM);
        }
    }

    fn header_click(dlg: HWND, col: usize) {
        unsafe {
            let mut nm: NMLISTVIEW = zeroed();
            nm.hdr.hwndFrom = item(dlg, ID_LIST);
            nm.hdr.idFrom = ID_LIST as usize;
            nm.hdr.code = LVN_COLUMNCLICK;
            nm.iSubItem = col as i32;
            SendMessageW(dlg, WM_NOTIFY, ID_LIST as usize, &nm as *const _ as LPARAM);
        }
    }

    fn sort_arrow(dlg: HWND) -> Vec<i32> {
        unsafe {
            let header = controls::header(item(dlg, ID_LIST));
            (0..COLUMNS.len())
                .map(|i| {
                    let mut it: HDITEMW = zeroed();
                    it.mask = HDI_FORMAT;
                    send(header, HDM_GETITEMW, i, &mut it as *mut _ as LPARAM);
                    it.fmt & (HDF_SORTUP | HDF_SORTDOWN)
                })
                .collect()
        }
    }

    fn text(dlg: HWND, id: u16) -> String {
        unsafe { window_text(item(dlg, id)) }
    }

    #[test]
    fn the_mode_is_remembered_and_falls_back() {
        let all = sources();
        let mut prefs = Prefs::default();
        assert_eq!(SearchMode::load(&prefs), SearchMode::Filter);
        let jackett = SearchMode::Source(source_id(&all[1]));
        jackett.store(&mut prefs);
        assert_eq!(SearchMode::load(&prefs), jackett);
        assert!(!prefs.get(MODE_PREF).unwrap().contains(KEY));
        SearchMode::All.store(&mut prefs);
        assert_eq!(prefs.get(MODE_PREF), Some("*"));
        SearchMode::Filter.store(&mut prefs);
        assert_eq!(prefs.get(MODE_PREF), None);
        // the switched-off source's id is masked, and picking it falls back
        let off = SearchMode::Source(source_id(&all[2]));
        assert_eq!(source_id(&all[2]), "http://127.0.0.1:9696/3/api?apikey=***");
        assert_eq!(off.clone().resolve(&all), SearchMode::Filter);
        assert_eq!(jackett.clone().resolve(&all[..1]), SearchMode::Filter, "gone");
        assert_eq!(jackett.clone().resolve(&all), jackett);
        assert_eq!(SearchMode::Filter.cue(&all), "Search torrent names");
        assert_eq!(SearchMode::All.cue(&all), "Search all sources");
        assert_eq!(jackett.cue(&all), "Search jackett.lan");
        assert_eq!(SearchMode::Source(source_id(&all[0])).cue(&all), "Search Prowlarr");
    }

    #[test]
    fn the_drop_down_lists_the_enabled_sources() {
        let all = sources();
        let current = SearchMode::Source(source_id(&all[0]));
        let menu = mode_menu(&all, &current);
        let texts: Vec<&str> = menu.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["Filter torrents", "Prowlarr", "jackett.lan", "All sources"]);
        assert_eq!(menu.iter().filter(|e| e.checked).count(), 1);
        assert!(menu[1].checked && menu.iter().all(|e| e.enabled));
        assert_eq!(menu[3].mode, SearchMode::All);
        // nothing to search: All sources greyed
        let none = mode_menu(&[], &SearchMode::Filter);
        assert_eq!(none.len(), 2);
        assert!(none[0].checked && !none[1].enabled);
        // unnamed Prowlarr indexers on one host show their masked URLs
        let twins = vec![
            Source { name: String::new(), ..source("", "http://h:9696/1/api?apikey=s3cr3tk3y", true) },
            source("", "http://h:9696/2/api", true),
        ];
        let texts: Vec<String> = mode_menu(&twins, &SearchMode::All).into_iter().map(|e| e.text).collect();
        assert_eq!(
            texts,
            ["Filter torrents", "http://h:9696/1/api?apikey=***", "http://h:9696/2/api", "All sources"]
        );
    }

    #[test]
    fn sorting_and_cells() {
        let now = 1_800_000_000;
        let rs = vec![
            result("b", Some(10), 300, "11", now - 100),
            result("A", None, 200, "22", now - 10),
            result("c", Some(30), 100, "33", now - 1000),
        ];
        let names = vec!["One".to_owned()];
        assert_eq!(sorted(&rs, &names, Col::Seeds, true), [2, 0, 1], "unknown last");
        assert_eq!(sorted(&rs, &names, Col::Seeds, false), [0, 2, 1], "unknown last either way");
        assert_eq!(sorted(&rs, &names, Col::Name, false), [1, 0, 2], "any case");
        assert_eq!(sorted(&rs, &names, Col::Size, true), [0, 1, 2]);
        assert_eq!(sorted(&rs, &names, Col::Age, false), [1, 0, 2], "newest first");
        let mut r = rs[0].clone();
        r.sources = vec![0];
        let c = cells(&r, &names, now);
        assert_eq!(c[0], "b");
        assert_eq!(c[1], "300 B");
        assert_eq!((c[2].as_str(), c[3].as_str()), ("10", "11"));
        assert_eq!(c[4], "TV");
        assert_eq!(c[5], fmt::duration(100));
        assert_eq!(c[6], "One");
        assert_eq!(cells(&rs[1], &names, now)[2], "", "unknown seeds");
    }

    fn open_results(shared: &Rc<RefCell<Shared>>, query: &str, pick: Pick) -> HWND {
        init_controls();
        TEST_FETCHER.with(|f| *f.borrow_mut() = Some(Arc::new(Fake)));
        let dlg = open(null_mut(), Box::new(TestHost(shared.clone())), query, pick);
        assert!(!dlg.is_null(), "the template does not load");
        dlg
    }

    #[test]
    fn results_fill_sort_and_add() {
        let mut all = sources();
        all.push(source("Broken", "http://127.0.0.1:1/fail/api", true));
        let shared = Rc::new(RefCell::new(Shared { sources: all, ..Shared::default() }));
        let dlg = open_results(&shared, "ubuntu", Pick::All);
        assert_eq!(shared.borrow().results_window, Some(dlg));
        let lv = unsafe { item(dlg, ID_LIST) };
        // three enabled sources answer (the broken one with an error), the same three torrents
        assert!(pump_until(|| !text(dlg, ID_STATUS).contains("searching")), "{}", text(dlg, ID_STATUS));
        let status = text(dlg, ID_STATUS);
        assert!(status.contains("Prowlarr: 3 results"), "{status}");
        assert!(status.contains("jackett.lan: 3 results"), "{status}");
        assert!(status.starts_with("Prowlarr") && status.contains("\r\nBroken: refused"), "{status}");
        assert!(!status.contains(KEY), "the key is masked: {status}");
        assert!(!status.contains("Off"), "a switched-off source is not searched");
        let r = rows(lv);
        let names: Vec<&str> = r.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(names, ["ubuntu Bravo", "ubuntu alpha", "ubuntu charlie"], "most seeds first");
        assert_eq!(r[0][6], "Prowlarr, jackett.lan", "merged");
        assert_eq!((r[0][2].as_str(), r[0][4].as_str()), ("50", "TV"));
        assert_eq!(text(dlg, ID_COUNT), "Results: 3");
        assert_eq!(sort_arrow(dlg), [0, 0, HDF_SORTDOWN, 0, 0, 0, 0]);
        assert!(unsafe { IsWindowEnabled(item(dlg, ID_STOP)) } == 0, "nothing runs");
        assert!(unsafe { IsWindowEnabled(item(dlg, ID_ADD)) } == 0, "nothing picked");

        // Name, then Name again
        header_click(dlg, 0);
        let names: Vec<String> = rows(lv).into_iter().map(|r| r[0].clone()).collect();
        assert_eq!(names, ["ubuntu alpha", "ubuntu Bravo", "ubuntu charlie"]);
        assert_eq!(sort_arrow(dlg), [HDF_SORTUP, 0, 0, 0, 0, 0, 0]);
        header_click(dlg, 0);
        assert_eq!(rows(lv)[0][0], "ubuntu charlie");
        assert_eq!(sort_arrow(dlg)[0], HDF_SORTDOWN);
        // Size: largest first
        header_click(dlg, 1);
        assert_eq!(rows(lv)[0][0], "ubuntu charlie");

        // pick alpha and Add: fetched on a thread (its magnet), then the window's add path
        let alpha = rows(lv).iter().position(|r| r[0] == "ubuntu alpha").unwrap();
        unsafe { controls::set_state(lv, Some(alpha), true, true) };
        assert!(unsafe { IsWindowEnabled(item(dlg, ID_ADD)) } != 0);
        click(dlg, ID_ADD);
        assert!(pump_until(|| !shared.borrow().adds.is_empty()));
        let (name, what) = shared.borrow().adds[0].clone();
        assert_eq!(name, "ubuntu alpha");
        assert!(what.starts_with("magnet:?xt=urn:btih:aaaa"), "{what}");
        assert_eq!(text(dlg, ID_COUNT), "Results: 3    Added: ubuntu alpha");
        // a double-click adds too, and the selection survives a re-sort
        header_click(dlg, 2);
        let row = rows(lv).iter().position(|r| r[0] == "ubuntu alpha").unwrap();
        assert_eq!(unsafe { controls::selected(lv) }, [row]);
        unsafe {
            let mut nm: NMHDR = zeroed();
            nm.hwndFrom = lv;
            nm.idFrom = ID_LIST as usize;
            nm.code = NM_DBLCLK;
            SendMessageW(dlg, WM_NOTIFY, ID_LIST as usize, &nm as *const _ as LPARAM);
        }
        assert!(pump_until(|| shared.borrow().adds.len() == 2));

        // a new search from the toolbar: one source only, a category from the box
        start_in(dlg, "debian", Pick::Source(source_id(&sources()[1])));
        assert_eq!(text(dlg, ID_QUERY), "debian");
        assert_eq!(unsafe { send(item(dlg, ID_SOURCE), CB_GETCURSEL, 0, 0) }, 2);
        assert!(pump_until(|| text(dlg, ID_STATUS) == "jackett.lan: 3 results"), "{}", text(dlg, ID_STATUS));
        assert_eq!(rows(lv)[0][0], "debian Bravo", "the Seeds sort stays");
        unsafe { DestroyWindow(dlg) };
        assert_eq!(shared.borrow().results_window, None);
    }

    #[test]
    fn enter_searches_stop_stops_and_no_sources_says_so() {
        let shared = Rc::new(RefCell::new(Shared::default()));
        let dlg = open_results(&shared, "", Pick::All);
        assert!(rows(unsafe { item(dlg, ID_LIST) }).is_empty());
        unsafe { set_text(item(dlg, ID_QUERY), "x") };
        click(dlg, ID_GO);
        assert_eq!(text(dlg, ID_STATUS), tr!(SearchNoSources));
        // with a source: Enter (the default button) searches
        shared.borrow_mut().sources = sources();
        unsafe { SendMessageW(dlg, WM_SEARCH_SOURCES, 0, 0) };
        assert_eq!(unsafe { send(item(dlg, ID_SOURCE), CB_GETCOUNT, 0, 0) }, 3, "All, then two enabled");
        unsafe {
            SetFocus(item(dlg, ID_QUERY));
            SendMessageW(dlg, WM_COMMAND, ID_GO as usize, 0);
            // the answers have not been collected yet: Stop
            assert!(IsWindowEnabled(item(dlg, ID_STOP)) != 0);
        }
        click(dlg, ID_STOP);
        assert_eq!(text(dlg, ID_STATUS), "Prowlarr: stopped\r\njackett.lan: stopped");
        let until = Instant::now() + Duration::from_millis(300);
        pump_until(|| Instant::now() > until);
        assert_eq!(text(dlg, ID_COUNT), "Results: 0", "late answers are dropped");
        // Esc closes
        unsafe { SendMessageW(dlg, WM_COMMAND, IDCANCEL as usize, 0) };
        assert_eq!(shared.borrow().results_window, None);
    }

    struct Fixture {
        dlg: HWND,
        _st: Box<Sources<'static>>,
        shared: Rc<RefCell<Shared>>,
    }

    fn caps_fake(s: &Source, _: Option<&FetchProxy>) -> Result<Vec<Category>, SearchError> {
        if s.url.contains("fail") {
            return Err(SearchError::Fetch(format!("HTTP 401 for {}?apikey={}", s.url, s.api_key)));
        }
        Ok(vec![
            Category {
                id: 2000,
                name: "Movies".into(),
                subcats: vec![Category { id: 2040, name: "HD".into(), subcats: Vec::new() }],
            },
            Category { id: 5000, name: "TV".into(), subcats: Vec::new() },
        ])
    }

    impl Fixture {
        fn new(sources: Vec<Source>) -> Fixture {
            init_controls();
            let shared = Rc::new(RefCell::new(Shared { sources, ..Shared::default() }));
            let host: &'static mut TestHost = Box::leak(Box::new(TestHost(shared.clone())));
            let mut st = Box::new(Sources { host, caps: caps_fake, loading: false });
            let t = sources_template();
            let dlg = unsafe {
                CreateDialogIndirectParamW(
                    GetModuleHandleW(null()),
                    t.as_ptr() as *const DLGTEMPLATE,
                    null_mut(),
                    Some(sources_proc),
                    &mut *st as *mut Sources<'_> as LPARAM,
                )
            };
            assert!(!dlg.is_null());
            Fixture { dlg, _st: st, shared }
        }

        fn st(&mut self) -> &mut Sources<'static> {
            unsafe { &mut *(GetWindowLongPtrW(self.dlg, DWLP_USER) as *mut Sources<'static>) }
        }

        fn list(&self) -> Vec<(String, String, bool)> {
            let lv = unsafe { item(self.dlg, ID_SOURCES) };
            rows(lv)
                .into_iter()
                .enumerate()
                .map(|(i, r)| (r[0].clone(), r[1].clone(), unsafe { lv_checked(lv, i) }))
                .collect()
        }

        fn select(&self, row: usize) {
            unsafe {
                let lv = item(self.dlg, ID_SOURCES);
                controls::set_state(lv, None, false, false);
                controls::set_state(lv, Some(row), true, true);
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.dlg) };
        }
    }

    #[test]
    fn sources_dialog_add_edit_remove_and_test() {
        let mut f = Fixture::new(sources());
        assert_eq!(f.shared.borrow().sources_dialog, Some(f.dlg));
        let list = f.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], ("Prowlarr".into(), "http://127.0.0.1:9696/1/api".into(), true));
        assert_eq!(list[1].0, "jackett.lan");
        assert_eq!(list[2], ("Off".into(), "http://127.0.0.1:9696/3/api?apikey=***".into(), false));
        assert!(list.iter().all(|(a, b, _)| !a.contains(KEY) && !b.contains(KEY)), "keys never shown");

        // the check box switches a source on and off
        unsafe { lv_set_checked(item(f.dlg, ID_SOURCES), 2, true) };
        assert!(f.shared.borrow().sources[2].enabled);
        assert_eq!(f.shared.borrow().stores, 1);

        // Add
        let form = SourceForm {
            name: " New ".into(),
            url: " https://idx.example/api ".into(),
            key: "k2".into(),
            ..SourceForm::new(None)
        };
        unsafe { add_source(f.dlg, f.st(), &form) };
        let added = f.shared.borrow().sources[3].clone();
        assert_eq!(
            added,
            Source {
                name: "New".into(),
                url: "https://idx.example/api".into(),
                api_key: "k2".into(),
                enabled: true
            }
        );
        assert_eq!(f.list()[3].0, "New");
        assert_eq!(unsafe { selected_source(f.dlg) }, Some(3));

        // Edit: the URL shows masked; left alone it and the key stay
        let off = f.shared.borrow().sources[2].clone();
        let mut form = SourceForm::new(Some(&off));
        assert_eq!(form.url, "http://127.0.0.1:9696/3/api?apikey=***");
        assert!(form.key.is_empty() && form.has_key);
        form.name = "Renamed".into();
        form.enabled = false;
        unsafe { store_edit(f.dlg, f.st(), 2, &form) };
        let s = f.shared.borrow().sources[2].clone();
        assert_eq!((s.name.as_str(), s.url.as_str(), s.api_key.as_str()), ("Renamed", off.url.as_str(), KEY));
        assert!(!s.enabled);
        // a new URL and key
        form.url = "http://127.0.0.1:9696/4/api".into();
        form.key = "new".into();
        unsafe { store_edit(f.dlg, f.st(), 2, &form) };
        let s = f.shared.borrow().sources[2].clone();
        assert_eq!((s.url.as_str(), s.api_key.as_str()), ("http://127.0.0.1:9696/4/api", "new"));

        // Test: OK with the count of categories and subcategories
        f.select(0);
        click(f.dlg, ID_SRC_TEST);
        assert_eq!(text(f.dlg, ID_SRC_STATUS), "Prowlarr: testing...");
        assert!(
            pump_until(|| text(f.dlg, ID_SRC_STATUS) == "Prowlarr: OK, 3 categories"),
            "{}",
            text(f.dlg, ID_SRC_STATUS)
        );
        // a failing one: the error without its key
        f.shared.borrow_mut().sources[1].url = "http://jackett.lan/fail".into();
        f.select(1);
        click(f.dlg, ID_SRC_TEST);
        assert!(pump_until(|| text(f.dlg, ID_SRC_STATUS).contains("HTTP 401")));
        let status = text(f.dlg, ID_SRC_STATUS);
        assert!(!status.contains(KEY) && status.contains("apikey=***"), "{status}");

        // Remove (the question is answered Yes in a test)
        f.select(0);
        click(f.dlg, ID_SRC_REMOVE);
        let names: Vec<String> = f.shared.borrow().sources.iter().map(|s| s.label()).collect();
        assert_eq!(names, ["jackett.lan", "Renamed", "New"]);
        assert_eq!(f.list().len(), 3);
        let _ = dialogs::take_told();

        // a browser changed them: the list follows
        f.shared.borrow_mut().sources.truncate(1);
        unsafe { SendMessageW(f.dlg, WM_SEARCH_SOURCES, 0, 0) };
        assert_eq!(f.list().len(), 1);
    }

    #[test]
    fn the_source_form_keeps_the_key_out_of_sight() {
        init_controls();
        let s = source("P", "http://p/api?apikey=s3cr3tk3y", true);
        let t = source_template();
        let mut st = FormState { title: "Edit".into(), form: SourceForm::new(Some(&s)), result: None };
        unsafe {
            let dlg = CreateDialogIndirectParamW(
                GetModuleHandleW(null()),
                t.as_ptr() as *const DLGTEMPLATE,
                null_mut(),
                Some(source_proc),
                &mut st as *mut FormState as LPARAM,
            );
            assert!(!dlg.is_null());
            assert_eq!(window_text(item(dlg, ID_URL)), "http://p/api?apikey=***");
            assert_eq!(window_text(item(dlg, ID_KEY)), "", "the key box starts empty");
            let style = GetWindowLongW(item(dlg, ID_KEY), GWL_STYLE) as u32;
            assert!(style & ES_PASSWORD as u32 != 0, "typed keys show as dots");
            // a URL that is not http(s) keeps the dialog open
            set_text(item(dlg, ID_URL), "ftp://x/api");
            SendMessageW(dlg, WM_COMMAND, IDOK as usize, 0);
            assert!(st.result.is_none());
            assert_eq!(dialogs::take_told(), [tr!(SearchNoUrl)]);
            set_text(item(dlg, ID_URL), "http://p/api?apikey=***");
            set_text(item(dlg, ID_KEY), " typed ");
            SendMessageW(dlg, WM_COMMAND, IDOK as usize, 0);
            DestroyWindow(dlg);
        }
        let form = st.result.unwrap();
        let mut stored = s.clone();
        form.apply(&mut stored);
        assert_eq!(stored.url, s.url, "the masked URL left alone keeps the stored one");
        assert_eq!(stored.api_key, "typed");
        assert!(valid_url("https://h:1/x") && !valid_url("http://") && !valid_url("magnet:?x"));
    }
}
