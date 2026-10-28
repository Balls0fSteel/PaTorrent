//! The RSS Downloader dialog (µTorrent 2.2.1: Options > RSS Downloader and the toolbar's RSS
//! button) and its Add RSS Feed dialog, on [`patorrent_frontend::rss`].
//!
//! Left, the Feeds list (All Feeds, then each feed) with Add, Edit, Remove and Update and the
//! selected feed's state under it. Right, two tabs: Filters (the filter list with check boxes
//! for on and off, the selected filter's fields, and the items of the feeds' last fetches it
//! matches) and History (what the downloader added). Edits go to the live downloader at once
//! through [`Host`] (the dialog has Close rather than OK and Cancel, as far as remembered);
//! the owner runs the filters over the feeds' current items when it closes. A feed's items show
//! in the main window's list when the feed is picked in the category tree, as in µTorrent.

use std::mem::zeroed;
use std::ptr::{null, null_mut};

use patorrent_frontend::icons::UiIcon;
pub use patorrent_frontend::rss::view::FeedForm;
use patorrent_frontend::rss::view::{self, quality_text};
use patorrent_frontend::rss::{FeedId, FilterId, MIN_INTERVALS, Quality, RssDownloader, RssFilter};
use patorrent_frontend::{lang, tr};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::controls;
use crate::dialogs::{self, BUTTON, COMBOBOX, Class, DWLP_MSGRESULT, DWLP_USER, EDIT, STATIC, Template};
use crate::util::{from_wide, hiword, loword, rtl, send, wide, window_text};

/// A static control style (in `System::SystemServices`, a feature we do not otherwise need).
const SS_NOPREFIX: u32 = 0x80;

/// Posted to the open dialog when a fetch changed the downloader: the lists are filled again.
pub const WM_RSS_CHANGED: u32 = WM_APP + 40;

/// What the dialog needs of its owner.
pub trait Host {
    /// Runs `f` on the live downloader to read it.
    fn view(&mut self, f: &mut dyn FnMut(&RssDownloader));
    /// Runs `f` on the live downloader to change it (the owner saves `rss.dat`).
    fn edit(&mut self, f: &mut dyn FnMut(&mut RssDownloader));
    /// Whether feed `id` is being fetched.
    fn updating(&mut self, id: FeedId) -> bool;
    /// "Update": fetch feed `id` now (`None`: every feed).
    fn update(&mut self, id: Option<FeedId>);
    /// The labels in use, for the Label box.
    fn labels(&mut self) -> Vec<String>;
    /// The dialog's window while it is open, null once it closes ([`WM_RSS_CHANGED`] goes
    /// there).
    fn set_dialog(&mut self, dlg: HWND);
}

/// Reads the downloader through `host`.
fn read<R>(host: &mut dyn Host, f: impl FnOnce(&RssDownloader) -> R) -> Option<R> {
    let mut f = Some(f);
    let mut out = None;
    host.view(&mut |d| {
        if let Some(f) = f.take() {
            out = Some(f(d));
        }
    });
    out
}

fn edit(host: &mut dyn Host, f: impl FnOnce(&mut RssDownloader)) {
    let mut f = Some(f);
    host.edit(&mut |d| {
        if let Some(f) = f.take() {
            f(d);
        }
    });
}

// ---- the Add RSS Feed dialog -------------------------------------------------------------

const ID_URL: u16 = 100;
const ID_ALIAS: u16 = 101;
const ID_NO_AUTO: u16 = 102;
const ID_AUTO: u16 = 103;
const ID_AUTO_SMART: u16 = 104;

struct FeedState {
    title: String,
    form: FeedForm,
    result: Option<FeedForm>,
}

/// Shows the Add RSS Feed dialog (`title` says which); the fields on OK.
pub fn feed_dialog(owner: HWND, title: &str, form: FeedForm) -> Option<FeedForm> {
    let mut st = FeedState { title: title.to_owned(), form, result: None };
    let t = feed_template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(feed_proc),
            &mut st as *mut FeedState as LPARAM,
        );
    }
    st.result
}

fn feed_template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    const TAB: u32 = C | WS_TABSTOP;
    const LINE: u32 = TAB | WS_BORDER | ES_AUTOHSCROLL as u32;
    let mut t = Template::new(260, 138);
    t.item(STATIC, 0xffff, C, [7, 7, 246, 9], tr!(RssFeedUrl));
    t.item(EDIT, ID_URL, LINE, [7, 18, 246, 13], "");
    t.item(STATIC, 0xffff, C, [7, 36, 246, 9], tr!(RssAlias));
    t.item(EDIT, ID_ALIAS, LINE, [7, 47, 246, 13], "");
    t.item(BUTTON, 0xffff, C | BS_GROUPBOX as u32, [7, 65, 246, 48], tr!(RssSubscription));
    let radio = TAB | BS_AUTORADIOBUTTON as u32;
    t.item(BUTTON, ID_NO_AUTO, radio | WS_GROUP, [14, 77, 232, 10], tr!(RssNoAuto));
    t.item(BUTTON, ID_AUTO, radio, [14, 89, 232, 10], tr!(RssAuto));
    t.item(
        BUTTON,
        ID_AUTO_SMART,
        TAB | WS_GROUP | BS_AUTOCHECKBOX as u32,
        [24, 101, 222, 10],
        tr!(RssSmartAuto),
    );
    t.item(BUTTON, IDOK as u16, TAB | BS_DEFPUSHBUTTON as u32, [149, 118, 50, 14], tr!(BtnOk));
    t.item(BUTTON, IDCANCEL as u16, TAB | BS_PUSHBUTTON as u32, [203, 118, 50, 14], tr!(BtnCancel));
    t.finish()
}

unsafe extern "system" fn feed_proc(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            let st = &*(lp as *const FeedState);
            set_text(dlg, &st.title);
            set_text(item(dlg, ID_URL), &st.form.url);
            set_text(item(dlg, ID_ALIAS), &st.form.alias);
            set_check(dlg, if st.form.subscribe { ID_AUTO } else { ID_NO_AUTO }, true);
            set_check(dlg, ID_AUTO_SMART, st.form.smart);
            EnableWindow(item(dlg, ID_AUTO_SMART), st.form.subscribe as i32);
            crate::fit::fit_texts(dlg, |_| Some(0));
            return 1;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut FeedState;
        if st.is_null() || msg != WM_COMMAND {
            return 0;
        }
        let st = &mut *st;
        match loword(wp) {
            ID_NO_AUTO | ID_AUTO => EnableWindow(item(dlg, ID_AUTO_SMART), checked(dlg, ID_AUTO) as i32),
            id if id == IDOK as u16 => {
                let url = window_text(item(dlg, ID_URL)).trim().to_owned();
                if url.is_empty() {
                    if !cfg!(test) {
                        dialogs::error(dlg, tr!(RssNoUrl));
                    }
                    SetFocus(item(dlg, ID_URL));
                    return 1;
                }
                let subscribe = checked(dlg, ID_AUTO);
                st.result = Some(FeedForm {
                    url,
                    alias: window_text(item(dlg, ID_ALIAS)).trim().to_owned(),
                    subscribe,
                    smart: subscribe && checked(dlg, ID_AUTO_SMART),
                });
                EndDialog(dlg, IDOK as isize)
            }
            id if id == IDCANCEL as u16 => EndDialog(dlg, IDCANCEL as isize),
            _ => return 0,
        };
        1
    }
}

/// The clipboard's text when it is a feed URL (µTorrent filled the Add RSS Feed dialog from
/// it), else nothing.
pub fn url_from_clipboard(owner: HWND) -> String {
    dialogs::clipboard_text(owner).and_then(|s| view::feed_url(&s)).unwrap_or_default()
}

// ---- the RSS Downloader dialog -----------------------------------------------------------

const ID_FEEDS: u16 = 200;
const ID_FEED_STATUS: u16 = 201;
const ID_FEED_ADD: u16 = 202;
const ID_FEED_EDIT: u16 = 203;
const ID_FEED_REMOVE: u16 = 204;
const ID_FEED_UPDATE: u16 = 205;
const ID_TABS: u16 = 206;

const ID_FILTERS: u16 = 300;
const ID_FILTER_ADD: u16 = 301;
const ID_FILTER_REMOVE: u16 = 302;
const ID_FILTER_UP: u16 = 303;
const ID_FILTER_DOWN: u16 = 304;
const ID_NAME: u16 = 310;
const ID_PATTERN: u16 = 311;
const ID_NOT: u16 = 312;
const ID_SAVE: u16 = 313;
const ID_SAVE_BROWSE: u16 = 314;
const ID_FEED: u16 = 315;
const ID_QUALITY: u16 = 316;
const ID_EPISODES: u16 = 317;
const ID_INTERVAL: u16 = 318;
const ID_LABEL: u16 = 319;
const ID_SMART: u16 = 320;
const ID_PRIORITY: u16 = 321;
const ID_PAUSED: u16 = 322;
const ID_PREVIEW: u16 = 323;
/// The labels of the filter fields (`0xffff` would make them unreachable for hiding).
const ID_FILTER_LABELS: u16 = 330;
const FILTER_LABELS: usize = 11;

const ID_HISTORY: u16 = 400;
const ID_HISTORY_CLEAR: u16 = 401;

/// The Filters tab's controls, shown and hidden with it (and its field labels,
/// `ID_FILTER_LABELS..`).
const FILTERS_PAGE: [u16; 19] = [
    ID_FILTERS,
    ID_FILTER_ADD,
    ID_FILTER_REMOVE,
    ID_FILTER_UP,
    ID_FILTER_DOWN,
    ID_NAME,
    ID_PATTERN,
    ID_NOT,
    ID_SAVE,
    ID_SAVE_BROWSE,
    ID_FEED,
    ID_QUALITY,
    ID_EPISODES,
    ID_INTERVAL,
    ID_LABEL,
    ID_SMART,
    ID_PRIORITY,
    ID_PAUSED,
    ID_PREVIEW,
];
const HISTORY_PAGE: [u16; 2] = [ID_HISTORY, ID_HISTORY_CLEAR];

/// The fields a filter's edits are read from (the Quality button is set by its menu).
const FIELDS: [u16; 12] = [
    ID_NAME,
    ID_PATTERN,
    ID_NOT,
    ID_SAVE,
    ID_FEED,
    ID_EPISODES,
    ID_INTERVAL,
    ID_LABEL,
    ID_SMART,
    ID_PRIORITY,
    ID_PAUSED,
    ID_QUALITY,
];

/// Most rows the preview shows.
const PREVIEW_ROWS: usize = 500;

struct State<'a> {
    host: &'a mut dyn Host,
    /// The Feeds list's rows (`None`: All Feeds).
    feeds: Vec<Option<FeedId>>,
    /// The Feed box's entries (`None`: All Feeds).
    feed_choices: Vec<Option<FeedId>>,
    /// The filter list's rows.
    filters: Vec<FilterId>,
    /// The filter whose fields show.
    filter: Option<FilterId>,
    /// Controls are being filled: their notifications are not edits.
    loading: bool,
    /// The visible tab: 0 Filters, 1 History.
    tab: usize,
}

/// Shows the dialog modally; `feed` is selected in the Feeds list first.
pub fn show(owner: HWND, host: &mut dyn Host, feed: Option<FeedId>) {
    let mut st = State::new(host);
    st.feeds = vec![feed];
    let t = template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(proc_),
            &mut st as *mut State<'_> as LPARAM,
        );
    }
}

impl<'a> State<'a> {
    fn new(host: &'a mut dyn Host) -> State<'a> {
        State {
            host,
            feeds: Vec::new(),
            feed_choices: Vec::new(),
            filters: Vec::new(),
            filter: None,
            loading: false,
            tab: 0,
        }
    }
}

fn template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    const TAB: u32 = C | WS_TABSTOP;
    const LINE: u32 = TAB | WS_BORDER | ES_AUTOHSCROLL as u32;
    const PUSH: u32 = TAB | BS_PUSHBUTTON as u32;
    const CHECK: u32 = TAB | BS_AUTOCHECKBOX as u32;
    const LIST: u32 = TAB | WS_BORDER | LVS_REPORT | LVS_SHOWSELALWAYS | LVS_SINGLESEL;
    const DROPLIST: u32 = TAB | WS_VSCROLL | CBS_DROPDOWNLIST as u32;
    const DROPDOWN: u32 = TAB | WS_VSCROLL | (CBS_DROPDOWN | CBS_AUTOHSCROLL | CBS_SORT) as u32;
    const LISTVIEW: Class = Class::Name("SysListView32");
    let mut t = Template::new(470, 292);
    t.item(STATIC, 0xffff, C, [7, 7, 140, 9], tr!(RssFeeds));
    t.item(LISTVIEW, ID_FEEDS, LIST | LVS_NOCOLUMNHEADER, [7, 18, 140, 196], "");
    t.item(STATIC, ID_FEED_STATUS, C | SS_NOPREFIX, [7, 217, 140, 18], "");
    // two rows of two, so longer translations fit
    t.item(BUTTON, ID_FEED_ADD, PUSH, [7, 238, 68, 14], tr!(RssFeedsAdd));
    t.item(BUTTON, ID_FEED_EDIT, PUSH, [79, 238, 68, 14], tr!(RssFeedsEdit));
    t.item(BUTTON, ID_FEED_REMOVE, PUSH, [7, 254, 68, 14], tr!(RssFeedsRemove));
    t.item(BUTTON, ID_FEED_UPDATE, PUSH, [79, 254, 68, 14], tr!(RssFeedsUpdate));

    // Filters
    t.item(LISTVIEW, ID_FILTERS, LIST | LVS_NOCOLUMNHEADER, [160, 25, 96, 189], "");
    t.item(BUTTON, ID_FILTER_ADD, PUSH, [160, 217, 46, 14], tr!(RssFilterAdd));
    t.item(BUTTON, ID_FILTER_REMOVE, PUSH, [210, 217, 46, 14], tr!(RssFilterRemove));
    t.item(BUTTON, ID_FILTER_UP, PUSH, [160, 233, 46, 14], tr!(RssFilterUp));
    t.item(BUTTON, ID_FILTER_DOWN, PUSH, [210, 233, 46, 14], tr!(RssFilterDown));
    let labels = [
        tr!(RssName),
        tr!(RssFilter),
        tr!(RssNot),
        tr!(RssSaveIn),
        tr!(RssFeed),
        tr!(RssQuality),
        tr!(RssEpisodes),
        tr!(RssMinInterval),
        tr!(RssLabel),
        tr!(RssMatching),
        tr!(RssEpisodesExample),
    ];
    let label = |t: &mut Template, i: usize, rect: [i16; 4]| {
        t.item(STATIC, ID_FILTER_LABELS + i as u16, C, rect, labels[i]);
    };
    label(&mut t, 0, [262, 27, 58, 9]);
    t.item(EDIT, ID_NAME, LINE, [322, 25, 135, 12], "");
    label(&mut t, 1, [262, 42, 58, 9]);
    t.item(EDIT, ID_PATTERN, LINE, [322, 40, 135, 12], "");
    label(&mut t, 2, [262, 57, 58, 9]);
    t.item(EDIT, ID_NOT, LINE, [322, 55, 135, 12], "");
    label(&mut t, 3, [262, 72, 58, 9]);
    t.item(EDIT, ID_SAVE, LINE, [322, 70, 115, 12], "");
    t.item(BUTTON, ID_SAVE_BROWSE, PUSH, [439, 70, 18, 12], "...");
    label(&mut t, 4, [262, 87, 58, 9]);
    t.item(COMBOBOX, ID_FEED, DROPLIST, [322, 85, 135, 120], "");
    label(&mut t, 5, [262, 102, 58, 9]);
    t.item(BUTTON, ID_QUALITY, PUSH, [322, 100, 135, 13], tr!(RssQualityAll));
    label(&mut t, 6, [262, 117, 58, 9]);
    t.item(EDIT, ID_EPISODES, LINE, [322, 115, 62, 12], "");
    label(&mut t, 10, [388, 117, 69, 9]);
    label(&mut t, 7, [262, 132, 58, 9]);
    t.item(COMBOBOX, ID_INTERVAL, DROPLIST, [322, 130, 135, 150], "");
    label(&mut t, 8, [262, 147, 58, 9]);
    t.item(COMBOBOX, ID_LABEL, DROPDOWN, [322, 145, 135, 120], "");
    t.item(BUTTON, ID_SMART, CHECK, [262, 162, 195, 10], tr!(RssSmart));
    t.item(BUTTON, ID_PRIORITY, CHECK, [262, 174, 195, 10], tr!(RssHighPriority));
    t.item(BUTTON, ID_PAUSED, CHECK, [262, 186, 195, 10], tr!(RssPaused));
    label(&mut t, 9, [262, 200, 195, 9]);
    t.item(LISTVIEW, ID_PREVIEW, LIST, [262, 210, 195, 52], "");

    // History
    t.item(LISTVIEW, ID_HISTORY, LIST, [160, 25, 297, 217], "");
    t.item(BUTTON, ID_HISTORY_CLEAR, PUSH, [397, 247, 60, 14], tr!(RssClearHistory));

    // last, so it lies under the pages' controls (and does not take their clicks)
    t.item(Class::Name("SysTabControl32"), ID_TABS, C | WS_CLIPSIBLINGS, [154, 7, 309, 261], "");
    t.item(BUTTON, IDOK as u16, TAB | BS_DEFPUSHBUTTON as u32, [413, 273, 50, 14], tr!(BtnClose));
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

/// Dialog units to pixels, horizontally.
unsafe fn dlu(dlg: HWND, x: i32) -> i32 {
    let mut rc = RECT { left: 0, top: 0, right: x, bottom: 0 };
    unsafe { MapDialogRect(dlg, &mut rc) };
    rc.right
}

// list view rows

unsafe fn lv_clear(lv: HWND) {
    unsafe { send(lv, LVM_DELETEALLITEMS, 0, 0) };
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

unsafe fn lv_set_text(lv: HWND, row: usize, text: &str) {
    unsafe {
        let mut w = wide(text);
        let mut it: LVITEMW = zeroed();
        it.pszText = w.as_mut_ptr();
        send(lv, LVM_SETITEMTEXTW, row, &it as *const _ as LPARAM);
    }
}

unsafe fn lv_selected(lv: HWND) -> Option<usize> {
    unsafe { controls::selected(lv).first().copied() }
}

unsafe fn lv_select(lv: HWND, row: usize) {
    unsafe {
        controls::set_state(lv, Some(row), true, true);
        send(lv, LVM_ENSUREVISIBLE, row, 0);
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

/// Columns in dialog units.
unsafe fn lv_columns(dlg: HWND, lv: HWND, cols: &[(&str, i32)]) {
    unsafe {
        let cols: Vec<(&str, i32, bool)> = cols.iter().map(|&(t, w)| (t, dlu(dlg, w), false)).collect();
        controls::set_columns(lv, &cols);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_LABELTIP | LVS_EX_DOUBLEBUFFER;
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
    }
}

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            init(dlg, &mut *(lp as *mut State<'_>));
            crate::fit::fit_texts(dlg, |_| Some(0));
            return 1;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut State<'_>;
        if st.is_null() {
            return 0;
        }
        let st = &mut *st;
        match msg {
            WM_COMMAND => command(dlg, st, loword(wp), hiword(wp) as u32) as isize,
            WM_NOTIFY => {
                let nm = &*(lp as *const NMHDR);
                if nm.code == NM_CUSTOMDRAW
                    && [ID_FEEDS, ID_PREVIEW, ID_HISTORY].contains(&(nm.idFrom as u16))
                {
                    let r = controls::selection_custom_draw(&mut *(lp as *mut NMLVCUSTOMDRAW));
                    SetWindowLongPtrW(dlg, DWLP_MSGRESULT, r);
                    return 1;
                }
                notify(dlg, st, nm);
                0
            }
            WM_CONTEXTMENU if wp as HWND == item(dlg, ID_FEEDS) => {
                feed_menu(dlg, st, lp);
                1
            }
            WM_RSS_CHANGED => {
                refresh(dlg, st);
                1
            }
            WM_DESTROY => {
                st.host.set_dialog(null_mut());
                0
            }
            _ => 0,
        }
    }
}

unsafe fn init(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        set_text(dlg, tr!(RssTitle));
        st.host.set_dialog(dlg);
        let tabs = item(dlg, ID_TABS);
        for (i, name) in [tr!(RssTabFilters), tr!(RssTabHistory)].into_iter().enumerate() {
            let mut text = wide(name);
            let mut tab: TCITEMW = zeroed();
            tab.mask = TCIF_TEXT;
            tab.pszText = text.as_mut_ptr();
            send(tabs, TCM_INSERTITEMW, i, &tab as *const _ as LPARAM);
        }
        lv_columns(dlg, item(dlg, ID_FEEDS), &[(tr!(ColFeed), 128)]);
        lv_columns(dlg, item(dlg, ID_FILTERS), &[(tr!(RssColFilter), 84)]);
        let ex = LVS_EX_CHECKBOXES;
        send(item(dlg, ID_FILTERS), LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        lv_columns(dlg, item(dlg, ID_PREVIEW), &[(tr!(ColName), 125), (tr!(ColFeed), 60)]);
        let cols: Vec<(&str, i32)> =
            view::HISTORY_COLUMNS.iter().zip([170, 70, 50]).map(|(&(t, ..), w)| (lang::text(t), w)).collect();
        lv_columns(dlg, item(dlg, ID_HISTORY), &cols);
        let interval = item(dlg, ID_INTERVAL);
        for (_, name) in MIN_INTERVALS {
            let w = wide(lang::text(name));
            send(interval, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        for l in st.host.labels() {
            let w = wide(&l);
            send(item(dlg, ID_LABEL), CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        let first = st.feeds.first().copied().flatten();
        fill_feeds(dlg, st, first);
        fill_filters(dlg, st, None);
        fill_history(dlg, st);
        show_tab(dlg, st, 0);
    }
}

/// Everything again after the downloader changed underneath (a fetch finished).
unsafe fn refresh(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        let feed = selected_feed(dlg, st);
        fill_feeds(dlg, st, feed);
        let filter = st.filter;
        fill_filters(dlg, st, filter);
        fill_history(dlg, st);
    }
}

unsafe fn show_tab(dlg: HWND, st: &mut State<'_>, tab: usize) {
    unsafe {
        st.tab = tab;
        send(item(dlg, ID_TABS), TCM_SETCURSEL, tab, 0);
        let labels = (0..FILTER_LABELS as u16).map(|i| ID_FILTER_LABELS + i);
        for id in FILTERS_PAGE.into_iter().chain(labels) {
            ShowWindow(item(dlg, id), if tab == 0 { SW_SHOW } else { SW_HIDE });
        }
        for id in HISTORY_PAGE {
            ShowWindow(item(dlg, id), if tab == 1 { SW_SHOW } else { SW_HIDE });
        }
    }
}

/// The feed picked in the Feeds list (`None`: All Feeds or nothing).
unsafe fn selected_feed(dlg: HWND, st: &State<'_>) -> Option<FeedId> {
    unsafe { lv_selected(item(dlg, ID_FEEDS)).and_then(|r| st.feeds.get(r).copied().flatten()) }
}

/// The Feeds list (All Feeds, then each feed) with `select` picked, the Feed box's choices,
/// and the state line.
unsafe fn fill_feeds(dlg: HWND, st: &mut State<'_>, select: Option<FeedId>) {
    unsafe {
        let Some(feeds) =
            read(st.host, |d| d.feeds.iter().map(|f| (f.id, f.name().to_owned())).collect::<Vec<_>>())
        else {
            return;
        };
        st.loading = true;
        let lv = item(dlg, ID_FEEDS);
        lv_clear(lv);
        lv_add(lv, 0, &[tr!(RssAllFeeds)]);
        st.feeds = vec![None];
        for (i, (id, name)) in feeds.iter().enumerate() {
            lv_add(lv, i + 1, &[name]);
            st.feeds.push(Some(*id));
        }
        let row = st.feeds.iter().position(|f| *f == select).unwrap_or(0);
        lv_select(lv, row);
        let combo = item(dlg, ID_FEED);
        send(combo, CB_RESETCONTENT, 0, 0);
        st.feed_choices = vec![None];
        let all = wide(tr!(RssAllFeeds));
        send(combo, CB_ADDSTRING, 0, all.as_ptr() as LPARAM);
        for (id, name) in &feeds {
            let w = wide(name);
            send(combo, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
            st.feed_choices.push(Some(*id));
        }
        st.loading = false;
        feed_selected(dlg, st);
    }
}

/// The state line and the buttons for the feed picked in the list.
unsafe fn feed_selected(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        let feed = selected_feed(dlg, st);
        let text = match feed {
            Some(id) => {
                let updating = st.host.updating(id);
                read(st.host, |d| d.feed(id).map(|f| view::feed_status(f, updating)))
                    .flatten()
                    .unwrap_or_default()
            }
            None => {
                let n = read(st.host, |d| d.feeds.len()).unwrap_or(0);
                tr!(RssFeedCount, count = n)
            }
        };
        set_text(item(dlg, ID_FEED_STATUS), &text);
        EnableWindow(item(dlg, ID_FEED_EDIT), feed.is_some() as i32);
        EnableWindow(item(dlg, ID_FEED_REMOVE), feed.is_some() as i32);
    }
}

/// The filter list with `select` picked (else the first filter), and its fields.
unsafe fn fill_filters(dlg: HWND, st: &mut State<'_>, select: Option<FilterId>) {
    unsafe {
        let Some(filters) = read(st.host, |d| {
            d.filters.iter().map(|f| (f.id, f.name.clone(), f.enabled)).collect::<Vec<_>>()
        }) else {
            return;
        };
        st.loading = true;
        let lv = item(dlg, ID_FILTERS);
        lv_clear(lv);
        st.filters.clear();
        for (i, (id, name, on)) in filters.iter().enumerate() {
            lv_add(lv, i, &[name]);
            lv_set_checked(lv, i, *on);
            st.filters.push(*id);
        }
        let row = select
            .and_then(|s| st.filters.iter().position(|&f| f == s))
            .or((!st.filters.is_empty()).then_some(0));
        if let Some(r) = row {
            lv_select(lv, r);
        }
        st.loading = false;
        show_filter(dlg, st, row.map(|r| st.filters[r]));
    }
}

/// Writes filter `id`'s fields into the controls (empty and greyed without one).
unsafe fn show_filter(dlg: HWND, st: &mut State<'_>, id: Option<FilterId>) {
    unsafe {
        st.filter = id;
        let f = id.and_then(|id| read(st.host, |d| d.filter(id).cloned()).flatten());
        let f2 = f.clone().unwrap_or_else(|| RssFilter { name: String::new(), ..RssFilter::default() });
        st.loading = true;
        for (cid, text) in [
            (ID_NAME, &f2.name),
            (ID_PATTERN, &f2.pattern),
            (ID_NOT, &f2.not),
            (ID_SAVE, &f2.save_in),
            (ID_EPISODES, &f2.episodes),
            (ID_LABEL, &f2.label),
        ] {
            set_text(item(dlg, cid), text);
        }
        let feed = st.feed_choices.iter().position(|&c| c == f2.feed).unwrap_or(0);
        send(item(dlg, ID_FEED), CB_SETCURSEL, feed, 0);
        let interval =
            MIN_INTERVALS.iter().position(|&(s, _)| s == f2.min_interval).map_or(-1, |i| i as isize);
        send(item(dlg, ID_INTERVAL), CB_SETCURSEL, interval as usize, 0);
        set_text(item(dlg, ID_QUALITY), &quality_text(f2.quality));
        set_check(dlg, ID_SMART, f2.smart);
        set_check(dlg, ID_PRIORITY, f2.top_priority);
        set_check(dlg, ID_PAUSED, f2.paused);
        for cid in
            FIELDS.iter().copied().chain([ID_SAVE_BROWSE, ID_FILTER_REMOVE, ID_FILTER_UP, ID_FILTER_DOWN])
        {
            EnableWindow(item(dlg, cid), f.is_some() as i32);
        }
        st.loading = false;
        fill_preview(dlg, st);
    }
}

/// The items of the feeds' last fetches the filter matches on its fields.
unsafe fn fill_preview(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        let lv = item(dlg, ID_PREVIEW);
        lv_clear(lv);
        let Some(id) = st.filter else { return };
        let rows = read(st.host, |d| {
            let Some(f) = d.filter(id) else { return Vec::new() };
            d.matching_items(f)
                .take(PREVIEW_ROWS)
                .map(|(feed, i)| {
                    (i.title.clone(), d.feed(feed).map(|f| f.name().to_owned()).unwrap_or_default())
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
        send(lv, WM_SETREDRAW, 0, 0);
        for (i, (title, feed)) in rows.iter().enumerate() {
            lv_add(lv, i, &[title, feed]);
        }
        send(lv, WM_SETREDRAW, 1, 0);
    }
}

unsafe fn fill_history(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        let rows = read(st.host, view::history_rows).unwrap_or_default();
        let lv = item(dlg, ID_HISTORY);
        send(lv, WM_SETREDRAW, 0, 0);
        lv_clear(lv);
        for (i, r) in rows.iter().enumerate() {
            lv_add(lv, i, &[&r[0], &r[1], &r[2]]);
        }
        send(lv, WM_SETREDRAW, 1, 0);
    }
}

/// The text of a combo box: the picked entry's (a `CBN_SELCHANGE` comes before the edit part
/// follows it), else what is typed.
unsafe fn combo_text(combo: HWND, picked: bool) -> String {
    unsafe {
        let i = send(combo, CB_GETCURSEL, 0, 0);
        if picked && i >= 0 {
            let len = send(combo, CB_GETLBTEXTLEN, i as usize, 0).max(0) as usize;
            let mut buf = vec![0u16; len + 1];
            send(combo, CB_GETLBTEXT, i as usize, buf.as_mut_ptr() as LPARAM);
            return from_wide(&buf);
        }
        window_text(combo)
    }
}

/// Stores the fields into the selected filter after an edit of control `changed`.
unsafe fn store_fields(dlg: HWND, st: &mut State<'_>, changed: u16, picked: bool) {
    unsafe {
        let Some(id) = st.filter else { return };
        let text = |cid| window_text(item(dlg, cid));
        let feed = send(item(dlg, ID_FEED), CB_GETCURSEL, 0, 0);
        let feed = st.feed_choices.get(feed.max(0) as usize).copied().flatten();
        let interval = send(item(dlg, ID_INTERVAL), CB_GETCURSEL, 0, 0);
        let interval = usize::try_from(interval).ok().and_then(|i| MIN_INTERVALS.get(i)).map(|&(s, _)| s);
        let label = combo_text(item(dlg, ID_LABEL), picked && changed == ID_LABEL);
        let (name, pattern, not, save_in, episodes) =
            (text(ID_NAME), text(ID_PATTERN), text(ID_NOT), text(ID_SAVE), text(ID_EPISODES));
        let (smart, top, paused) =
            (checked(dlg, ID_SMART), checked(dlg, ID_PRIORITY), checked(dlg, ID_PAUSED));
        edit(st.host, |d| {
            let Some(f) = d.filter_mut(id) else { return };
            f.name = name.clone();
            f.pattern = pattern;
            f.not = not;
            f.save_in = save_in.trim().to_owned();
            f.feed = feed;
            f.episodes = episodes;
            if let Some(s) = interval {
                f.min_interval = s;
            }
            f.label = label.trim().to_owned();
            f.smart = smart;
            f.top_priority = top;
            f.paused = paused;
        });
        if changed == ID_NAME {
            if let Some(row) = st.filters.iter().position(|&f| f == id) {
                lv_set_text(item(dlg, ID_FILTERS), row, &name);
            }
        }
        if matches!(changed, ID_PATTERN | ID_NOT | ID_FEED | ID_EPISODES) {
            fill_preview(dlg, st);
        }
    }
}

/// Quality menu entries: 1 is All, `2 + i` is `Quality::LIST[i]`.
unsafe fn toggle_quality(dlg: HWND, st: &mut State<'_>, choice: usize) {
    unsafe {
        let Some(id) = st.filter else { return };
        let mut text = String::new();
        edit(st.host, |d| {
            let Some(f) = d.filter_mut(id) else { return };
            f.quality = match choice {
                1 => Quality::ALL,
                c => match Quality::LIST.get(c.wrapping_sub(2)) {
                    Some(&(q, _)) if f.quality.contains(q) => Quality(f.quality.0 & !q.0),
                    Some(&(q, _)) => f.quality.union(q),
                    None => f.quality,
                },
            };
            text = quality_text(f.quality);
        });
        set_text(item(dlg, ID_QUALITY), &text);
        fill_preview(dlg, st);
    }
}

unsafe fn quality_menu(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        let Some(id) = st.filter else { return };
        let Some(q) = read(st.host, |d| d.filter(id).map(|f| f.quality)).flatten() else { return };
        let menu = CreatePopupMenu();
        let flag = |on: bool| MF_STRING | if on { MF_CHECKED } else { 0 };
        let all = wide(tr!(RssQualityAll));
        AppendMenuW(menu, flag(q.is_all()), 1, all.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        for (i, (one, name)) in Quality::LIST.iter().enumerate() {
            let w = wide(name);
            AppendMenuW(menu, flag(q.contains(*one) && !q.is_all()), 2 + i, w.as_ptr());
        }
        let mut r: RECT = zeroed();
        GetWindowRect(item(dlg, ID_QUALITY), &mut r);
        let choice = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTBUTTON | rtl(TPM_LAYOUTRTL),
            r.left,
            r.bottom,
            0,
            dlg,
            null(),
        );
        DestroyMenu(menu);
        if choice > 0 {
            toggle_quality(dlg, st, choice as usize);
        }
    }
}

unsafe fn command(dlg: HWND, st: &mut State<'_>, id: u16, code: u32) -> bool {
    unsafe {
        match id {
            ID_NAME | ID_PATTERN | ID_NOT | ID_SAVE | ID_EPISODES if code == EN_CHANGE => {
                if !st.loading {
                    store_fields(dlg, st, id, false);
                }
            }
            ID_FEED | ID_INTERVAL if code == CBN_SELCHANGE => store_fields(dlg, st, id, true),
            ID_LABEL if code == CBN_SELCHANGE || code == CBN_EDITCHANGE => {
                store_fields(dlg, st, id, code == CBN_SELCHANGE)
            }
            ID_SMART | ID_PRIORITY | ID_PAUSED if code == BN_CLICKED => store_fields(dlg, st, id, false),
            ID_QUALITY => quality_menu(dlg, st),
            ID_SAVE_BROWSE => {
                let now = window_text(item(dlg, ID_SAVE));
                let title = tr!(RssPickSaveDir);
                if let Some(p) =
                    crate::create::pick_folder_at(dlg, title, Some(now.as_str()).filter(|s| !s.is_empty()))
                {
                    set_text(item(dlg, ID_SAVE), &p);
                }
            }
            ID_FILTER_ADD => add_filter(dlg, st),
            ID_FILTER_REMOVE => {
                let Some(filter) = st.filter else { return true };
                let row = st.filters.iter().position(|&f| f == filter).unwrap_or(0);
                edit(st.host, |d| d.remove_filter(filter));
                // the one that took its place, else the one above
                let rest: Vec<FilterId> = st.filters.iter().copied().filter(|&f| f != filter).collect();
                let next = rest.get(row).or(rest.last()).copied();
                fill_filters(dlg, st, next);
            }
            ID_FILTER_UP | ID_FILTER_DOWN => {
                let Some(filter) = st.filter else { return true };
                let up = id == ID_FILTER_UP;
                edit(st.host, |d| d.move_filter(filter, up));
                fill_filters(dlg, st, Some(filter));
            }
            ID_FEED_ADD => add_feed(dlg, st),
            ID_FEED_EDIT => edit_feed(dlg, st),
            ID_FEED_REMOVE => remove_feed(dlg, st),
            ID_FEED_UPDATE => {
                let feed = selected_feed(dlg, st);
                st.host.update(feed);
                feed_selected(dlg, st);
            }
            ID_HISTORY_CLEAR => {
                edit(st.host, |d| d.history.clear());
                fill_history(dlg, st);
            }
            _ if id == IDOK as u16 || id == IDCANCEL as u16 => {
                EndDialog(dlg, IDOK as isize);
            }
            _ => return false,
        }
        true
    }
}

unsafe fn add_filter(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        // µTorrent named it after the feed picked on the left and bound it to that feed
        let feed = selected_feed(dlg, st);
        let mut id = 0;
        edit(st.host, |d| id = d.add_filter(RssFilter { feed, ..RssFilter::default() }));
        fill_filters(dlg, st, Some(id));
        let name = item(dlg, ID_NAME);
        SetFocus(name);
        send(name, EM_SETSEL, 0, -1);
    }
}

unsafe fn add_feed(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        let form = FeedForm { url: url_from_clipboard(dlg), ..FeedForm::default() };
        let Some(form) = feed_dialog(dlg, tr!(RssAddFeed), form) else { return };
        let mut id = 0;
        edit(st.host, |d| id = form.add_to(d));
        st.host.update(Some(id));
        fill_feeds(dlg, st, Some(id));
    }
}

unsafe fn edit_feed(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        let Some(id) = selected_feed(dlg, st) else { return };
        let Some(form) = read(st.host, |d| d.feed(id).map(FeedForm::of)).flatten() else { return };
        let Some(form) = feed_dialog(dlg, tr!(RssEditFeed), form) else { return };
        let mut moved = false;
        edit(st.host, |d| moved = form.store(d, id));
        if moved {
            st.host.update(Some(id));
        }
        fill_feeds(dlg, st, Some(id));
    }
}

unsafe fn remove_feed(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        let Some(id) = selected_feed(dlg, st) else { return };
        let name = read(st.host, |d| d.feed(id).map(|f| f.name().to_owned())).flatten().unwrap_or_default();
        if !cfg!(test) && !dialogs::confirm(dlg, &tr!(AskDeleteFeed, name)) {
            return;
        }
        edit(st.host, |d| d.remove_feed(id));
        fill_feeds(dlg, st, None);
        let filter = st.filter;
        fill_filters(dlg, st, filter);
    }
}

/// The Feeds list's right-click menu: Add, Edit, Enable / Disable, Update, Remove.
unsafe fn feed_menu(dlg: HWND, st: &mut State<'_>, lp: LPARAM) {
    unsafe {
        let feed = selected_feed(dlg, st);
        let enabled = feed.and_then(|id| read(st.host, |d| d.feed(id).map(|f| f.enabled)).flatten());
        let menu = CreatePopupMenu();
        let add = |id: u16, text: &str, on: bool| {
            let w = wide(text);
            AppendMenuW(menu, MF_STRING | if on { 0 } else { MF_GRAYED }, id as usize, w.as_ptr());
        };
        add(ID_FEED_ADD, tr!(MenuAddFeed), true);
        add(ID_FEED_EDIT, tr!(MenuEditFeed), feed.is_some());
        add(
            1,
            if enabled == Some(false) { tr!(MenuEnableFeed) } else { tr!(MenuDisableFeed) },
            feed.is_some(),
        );
        let toggle_icon = if enabled == Some(false) { UiIcon::Feeds } else { UiIcon::FeedDisabled };
        crate::icons::tag_item(menu, toggle_icon);
        add(ID_FEED_UPDATE, tr!(MenuUpdateFeed), true);
        add(ID_FEED_REMOVE, tr!(MenuDeleteFeed), feed.is_some());
        let mut p = POINT { x: (lp & 0xffff) as i16 as i32, y: ((lp >> 16) & 0xffff) as i16 as i32 };
        if lp == -1 {
            GetCursorPos(&mut p);
        }
        let feed_icon = |id: u16| match id {
            ID_FEED_ADD => Some(UiIcon::AddFeed),
            ID_FEED_EDIT => Some(UiIcon::Edit),
            ID_FEED_UPDATE => Some(UiIcon::Refresh),
            ID_FEED_REMOVE => Some(UiIcon::Remove),
            _ => None,
        };
        let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(dlg);
        crate::icons::menu_icons(menu, dpi, &feed_icon);
        let choice = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | rtl(TPM_LAYOUTRTL),
            p.x,
            p.y,
            0,
            dlg,
            null(),
        );
        DestroyMenu(menu);
        match choice as u16 {
            0 => {}
            1 => {
                if let Some(id) = feed {
                    set_feed_enabled(dlg, st, id, enabled != Some(true));
                }
            }
            c => {
                command(dlg, st, c, BN_CLICKED);
            }
        }
    }
}

unsafe fn set_feed_enabled(dlg: HWND, st: &mut State<'_>, id: FeedId, on: bool) {
    unsafe {
        edit(st.host, |d| {
            if let Some(f) = d.feed_mut(id) {
                f.enabled = on;
            }
        });
        feed_selected(dlg, st);
    }
}

unsafe fn notify(dlg: HWND, st: &mut State<'_>, nm: &NMHDR) {
    unsafe {
        match (nm.idFrom as u16, nm.code) {
            (ID_TABS, TCN_SELCHANGE) => {
                let tab = send(item(dlg, ID_TABS), TCM_GETCURSEL, 0, 0).max(0) as usize;
                show_tab(dlg, st, tab);
            }
            (ID_FEEDS, LVN_ITEMCHANGED) if !st.loading => {
                let lv = &*(nm as *const NMHDR as *const NMLISTVIEW);
                if lv.uChanged & LVIF_STATE != 0 && (lv.uNewState ^ lv.uOldState) & LVIS_SELECTED != 0 {
                    feed_selected(dlg, st);
                }
            }
            (ID_FEEDS, NM_DBLCLK) => edit_feed(dlg, st),
            (ID_FILTERS, LVN_ITEMCHANGED) if !st.loading => {
                let lv = &*(nm as *const NMHDR as *const NMLISTVIEW);
                if lv.uChanged & LVIF_STATE == 0 || lv.iItem < 0 {
                    return;
                }
                let row = lv.iItem as usize;
                let Some(&id) = st.filters.get(row) else { return };
                if (lv.uNewState ^ lv.uOldState) & LVIS_STATEIMAGEMASK != 0
                    && lv.uOldState & LVIS_STATEIMAGEMASK != 0
                {
                    let on = lv_checked(item(dlg, ID_FILTERS), row);
                    edit(st.host, |d| {
                        if let Some(f) = d.filter_mut(id) {
                            f.enabled = on;
                        }
                    });
                }
                if lv.uNewState & LVIS_SELECTED != 0
                    && lv.uOldState & LVIS_SELECTED == 0
                    && st.filter != Some(id)
                {
                    show_filter(dlg, st, Some(id));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    //! The dialog built from its template, hidden and modeless, on a downloader of its own.

    use patorrent_frontend::rss::{Feed, FeedItem};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;

    use super::*;

    #[derive(Default)]
    struct TestHost {
        d: RssDownloader,
        updates: Vec<Option<FeedId>>,
        dialog: Option<HWND>,
        edits: usize,
    }

    impl Host for TestHost {
        fn view(&mut self, f: &mut dyn FnMut(&RssDownloader)) {
            f(&self.d)
        }
        fn edit(&mut self, f: &mut dyn FnMut(&mut RssDownloader)) {
            self.edits += 1;
            f(&mut self.d)
        }
        fn updating(&mut self, _: FeedId) -> bool {
            false
        }
        fn update(&mut self, id: Option<FeedId>) {
            self.updates.push(id);
        }
        fn labels(&mut self) -> Vec<String> {
            vec!["Linux".into(), "TV".into()]
        }
        fn set_dialog(&mut self, dlg: HWND) {
            self.dialog = (!dlg.is_null()).then_some(dlg);
        }
    }

    struct Fixture {
        dlg: HWND,
        // the dialog holds pointers into both
        st: Box<State<'static>>,
        host: *mut TestHost,
    }

    impl Fixture {
        fn new(d: RssDownloader) -> Fixture {
            unsafe {
                let icc = INITCOMMONCONTROLSEX {
                    dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                    dwICC: ICC_LISTVIEW_CLASSES | ICC_TAB_CLASSES,
                };
                InitCommonControlsEx(&icc);
            }
            let host: &'static mut TestHost = Box::leak(Box::new(TestHost { d, ..TestHost::default() }));
            let ptr = host as *mut TestHost;
            let mut st = Box::new(State::new(host));
            st.feeds = vec![None];
            let t = template();
            let dlg = unsafe {
                CreateDialogIndirectParamW(
                    GetModuleHandleW(null()),
                    t.as_ptr() as *const DLGTEMPLATE,
                    null_mut(),
                    Some(proc_),
                    &mut *st as *mut State<'_> as LPARAM,
                )
            };
            assert!(!dlg.is_null(), "the template does not load");
            Fixture { dlg, st, host: ptr }
        }

        fn host(&mut self) -> &mut TestHost {
            unsafe { &mut *self.host }
        }

        fn text(&self, id: u16) -> String {
            unsafe { window_text(item(self.dlg, id)) }
        }

        /// Types into an edit (which notifies `EN_CHANGE`).
        fn type_in(&self, id: u16, s: &str) {
            unsafe { set_text(item(self.dlg, id), s) };
        }

        fn click(&self, id: u16) {
            unsafe {
                let wp = (BN_CLICKED as usize) << 16 | id as usize;
                SendMessageW(self.dlg, WM_COMMAND, wp, item(self.dlg, id) as LPARAM);
            }
        }

        fn toggle(&self, id: u16) {
            unsafe {
                let on = checked(self.dlg, id);
                set_check(self.dlg, id, !on);
            }
            self.click(id);
        }

        fn pick(&self, id: u16, index: usize) {
            unsafe {
                send(item(self.dlg, id), CB_SETCURSEL, index, 0);
                let wp = (CBN_SELCHANGE as usize) << 16 | id as usize;
                SendMessageW(self.dlg, WM_COMMAND, wp, item(self.dlg, id) as LPARAM);
            }
        }

        fn rows(&self, id: u16) -> Vec<String> {
            let lv = unsafe { item(self.dlg, id) };
            let n = unsafe { send(lv, LVM_GETITEMCOUNT, 0, 0) } as usize;
            (0..n)
                .map(|row| {
                    let mut buf = [0u16; 260];
                    unsafe {
                        let mut it: LVITEMW = zeroed();
                        it.pszText = buf.as_mut_ptr();
                        it.cchTextMax = buf.len() as i32;
                        send(lv, LVM_GETITEMTEXTW, row, &mut it as *mut _ as LPARAM);
                    }
                    from_wide(&buf)
                })
                .collect()
        }

        fn select_row(&self, id: u16, row: usize) {
            unsafe {
                let lv = item(self.dlg, id);
                controls::set_state(lv, None, false, false);
                lv_select(lv, row);
            }
        }

        fn enabled(&self, id: u16) -> bool {
            unsafe { IsWindowEnabled(item(self.dlg, id)) != 0 }
        }

        fn visible(&self, id: u16) -> bool {
            // the dialog itself is hidden: the control's own style says whether it shows
            unsafe { GetWindowLongW(item(self.dlg, id), GWL_STYLE) as u32 & WS_VISIBLE != 0 }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.dlg);
                drop(Box::from_raw(self.host));
            }
        }
    }

    fn downloader() -> RssDownloader {
        let mut d = RssDownloader::default();
        let a = d.add_feed("https://a/rss", "Shows");
        d.add_feed("https://b/rss", "Linux ISOs");
        let item = |t: &str| FeedItem {
            title: t.into(),
            guid: t.into(),
            link: format!("https://a/{t}.torrent"),
            ..FeedItem::default()
        };
        d.feed_fetched(
            a,
            1,
            Ok(Feed {
                items: vec![
                    item("Show.S01E01.720p.HDTV"),
                    item("Show.S01E02.HDTV"),
                    item("Other.S03E04.720p"),
                ],
                ..Feed::default()
            }),
        );
        d.history.clear();
        d
    }

    #[test]
    fn shows_feeds_filters_and_history() {
        let mut d = downloader();
        d.add_filter(RssFilter { name: "Show".into(), pattern: "show".into(), ..RssFilter::default() });
        d.add_filter(RssFilter { name: "Off".into(), enabled: false, ..RssFilter::default() });
        d.request_item(1, "Other.S03E04.720p", 1_800_000_000);
        let mut f = Fixture::new(d);
        assert_eq!(f.rows(ID_FEEDS), ["All Feeds", "Shows", "Linux ISOs"]);
        assert_eq!(f.text(ID_FEED_STATUS), "Feeds: 2");
        assert!(!f.enabled(ID_FEED_EDIT), "All Feeds cannot be edited");
        assert_eq!(f.rows(ID_FILTERS), ["Show", "Off"]);
        unsafe {
            assert!(lv_checked(item(f.dlg, ID_FILTERS), 0));
            assert!(!lv_checked(item(f.dlg, ID_FILTERS), 1));
        }
        assert_eq!(
            (f.text(ID_NAME), f.text(ID_PATTERN), f.text(ID_QUALITY)),
            ("Show".into(), "show".into(), "All".into())
        );
        assert_eq!(f.rows(ID_PREVIEW), ["Show.S01E01.720p.HDTV", "Show.S01E02.HDTV"]);
        assert_eq!(f.rows(ID_HISTORY), ["Other.S03E04.720p"]);
        assert!(f.visible(ID_PATTERN) && !f.visible(ID_HISTORY));
        unsafe { show_tab(f.dlg, &mut *(GetWindowLongPtrW(f.dlg, DWLP_USER) as *mut State<'_>), 1) };
        assert!(!f.visible(ID_PATTERN) && f.visible(ID_HISTORY));
        f.select_row(ID_FEEDS, 1);
        assert!(f.text(ID_FEED_STATUS).starts_with("Updated "), "{}", f.text(ID_FEED_STATUS));
        assert!(f.enabled(ID_FEED_EDIT));
        f.click(ID_FEED_UPDATE);
        assert_eq!(f.host().updates, [Some(1)]);
        f.click(ID_HISTORY_CLEAR);
        assert!(f.host().d.history.is_empty() && f.rows(ID_HISTORY).is_empty());
        let dlg = f.dlg;
        assert_eq!(f.host().dialog, Some(dlg));
        let _ = &f.st;
    }

    #[test]
    fn editing_a_filter_goes_to_the_downloader() {
        let mut f = Fixture::new(downloader());
        assert!(f.rows(ID_FILTERS).is_empty());
        assert!(!f.enabled(ID_PATTERN), "no filter: the fields are greyed");
        f.select_row(ID_FEEDS, 1);
        f.click(ID_FILTER_ADD);
        assert_eq!(f.rows(ID_FILTERS), ["New filter"]);
        let id = f.host().d.filters[0].id;
        assert_eq!(f.host().d.filters[0].feed, Some(1), "bound to the feed picked on the left");
        assert!(f.enabled(ID_PATTERN));
        f.type_in(ID_NAME, "My show");
        assert_eq!(f.rows(ID_FILTERS), ["My show"]);
        f.type_in(ID_PATTERN, "*show*");
        assert_eq!(f.rows(ID_PREVIEW), ["Show.S01E01.720p.HDTV", "Show.S01E02.HDTV"]);
        f.type_in(ID_NOT, "*S01E02*");
        assert_eq!(f.rows(ID_PREVIEW), ["Show.S01E01.720p.HDTV"]);
        f.type_in(ID_EPISODES, "1x01-05");
        f.type_in(ID_SAVE, r" D:\TV ");
        f.pick(ID_INTERVAL, 5);
        f.toggle(ID_SMART);
        f.toggle(ID_PAUSED);
        f.pick(ID_FEED, 0);
        unsafe {
            let st = &mut *(GetWindowLongPtrW(f.dlg, DWLP_USER) as *mut State<'_>);
            toggle_quality(f.dlg, st, 2 + 11); // 720p
            toggle_quality(f.dlg, st, 2); // HDTV
        }
        assert_eq!(f.text(ID_QUALITY), "HDTV|720p");
        unsafe {
            set_text(item(f.dlg, ID_LABEL), "TV");
            let wp = (CBN_EDITCHANGE as usize) << 16 | ID_LABEL as usize;
            SendMessageW(f.dlg, WM_COMMAND, wp, item(f.dlg, ID_LABEL) as LPARAM);
        }
        let flt = f.host().d.filter(id).unwrap().clone();
        assert_eq!(
            (flt.name.as_str(), flt.pattern.as_str(), flt.not.as_str()),
            ("My show", "*show*", "*S01E02*")
        );
        assert_eq!(
            (flt.episodes.as_str(), flt.save_in.as_str(), flt.label.as_str()),
            ("1x01-05", r"D:\TV", "TV")
        );
        assert_eq!((flt.min_interval, flt.smart, flt.paused, flt.top_priority), (86_400, true, true, false));
        assert_eq!((flt.feed, flt.quality), (None, Quality::HDTV.union(Quality::P720)));
        assert_eq!(f.rows(ID_PREVIEW), ["Show.S01E01.720p.HDTV"]);
        unsafe {
            let st = &mut *(GetWindowLongPtrW(f.dlg, DWLP_USER) as *mut State<'_>);
            toggle_quality(f.dlg, st, 1);
        }
        assert_eq!(f.host().d.filter(id).unwrap().quality, Quality::ALL);
        // a second filter, Up, the check box, Remove
        f.click(ID_FILTER_ADD);
        let second = f.host().d.filters[1].id;
        f.click(ID_FILTER_UP);
        assert_eq!(f.host().d.filters[0].id, second);
        assert_eq!(f.rows(ID_FILTERS), ["New filter", "My show"]);
        unsafe { lv_set_checked(item(f.dlg, ID_FILTERS), 1, false) };
        assert!(!f.host().d.filter(id).unwrap().enabled);
        f.click(ID_FILTER_REMOVE);
        assert_eq!(f.host().d.filters.len(), 1);
        assert_eq!(f.rows(ID_FILTERS), ["My show"]);
        assert_eq!(f.text(ID_NAME), "My show", "the next filter shows");
    }

    #[test]
    fn changes_underneath_refill_the_lists() {
        let mut f = Fixture::new(downloader());
        f.select_row(ID_FEEDS, 2);
        f.host().d.feed_mut(2).unwrap().alias = "Distros".into();
        f.host().d.remove_feed(1);
        unsafe { SendMessageW(f.dlg, WM_RSS_CHANGED, 0, 0) };
        assert_eq!(f.rows(ID_FEEDS), ["All Feeds", "Distros"]);
        assert_eq!(unsafe { lv_selected(item(f.dlg, ID_FEEDS)) }, Some(1), "the picked feed stays picked");
        unsafe {
            let st = &mut *(GetWindowLongPtrW(f.dlg, DWLP_USER) as *mut State<'_>);
            set_feed_enabled(f.dlg, st, 2, false);
        }
        assert!(!f.host().d.feed(2).unwrap().enabled);
        assert_eq!(f.text(ID_FEED_STATUS), "Disabled");
        f.click(ID_FEED_REMOVE);
        assert!(f.host().d.feeds.is_empty());
        assert_eq!(f.rows(ID_FEEDS), ["All Feeds"]);
    }

    #[test]
    fn feed_forms() {
        let mut d = RssDownloader::default();
        let form =
            FeedForm { url: " https://x/rss ".into(), alias: "X".into(), subscribe: false, smart: true };
        let id = form.add_to(&mut d);
        let f = d.feed(id).unwrap();
        assert_eq!(
            (f.url.as_str(), f.alias.as_str(), f.subscribe, f.subscribe_smart),
            ("https://x/rss", "X", false, false)
        );
        assert_eq!(
            FeedForm::of(f),
            FeedForm { url: "https://x/rss".into(), alias: "X".into(), subscribe: false, smart: false }
        );
        d.feed_mut(id).unwrap().last_refresh = Some(5);
        let same = FeedForm { url: "https://x/rss".into(), alias: "".into(), subscribe: true, smart: true };
        assert!(!same.store(&mut d, id));
        assert_eq!(d.feed(id).unwrap().last_refresh, Some(5));
        assert!(d.feed(id).unwrap().subscribe_smart);
        let moved = FeedForm { url: "https://y/rss".into(), ..same };
        assert!(moved.store(&mut d, id));
        assert_eq!(d.feed(id).unwrap().last_refresh, None, "a new URL is fetched again");
        // the dialog itself: OK reads the fields, an empty URL keeps it open
        let t = feed_template();
        let mut st = FeedState { title: "Add RSS Feed".into(), form: FeedForm::default(), result: None };
        unsafe {
            let dlg = CreateDialogIndirectParamW(
                GetModuleHandleW(null()),
                t.as_ptr() as *const DLGTEMPLATE,
                null_mut(),
                Some(feed_proc),
                &mut st as *mut FeedState as LPARAM,
            );
            assert!(!dlg.is_null());
            assert!(checked(dlg, ID_NO_AUTO) && IsWindowEnabled(item(dlg, ID_AUTO_SMART)) == 0);
            SendMessageW(dlg, WM_COMMAND, IDOK as usize, 0);
            assert!(st.result.is_none());
            set_text(item(dlg, ID_URL), "https://z/rss:COOKIE:uid=1");
            set_text(item(dlg, ID_ALIAS), " Zed ");
            set_check(dlg, ID_NO_AUTO, false);
            set_check(dlg, ID_AUTO, true);
            SendMessageW(dlg, WM_COMMAND, ID_AUTO as usize, 0);
            assert!(IsWindowEnabled(item(dlg, ID_AUTO_SMART)) != 0);
            set_check(dlg, ID_AUTO_SMART, true);
            SendMessageW(dlg, WM_COMMAND, IDOK as usize, 0);
            DestroyWindow(dlg);
        }
        assert_eq!(
            st.result,
            Some(FeedForm {
                url: "https://z/rss:COOKIE:uid=1".into(),
                alias: "Zed".into(),
                subscribe: true,
                smart: true
            })
        );
    }
}
