//! Options > Setup Guide (D53, Ctrl+G, µTorrent 2.2.1's entry), also shown at the first start of
//! a profile before the main window and the engine: one page at a time, each a question with a
//! short text, the controls of [`rookparcel_frontend::setup_guide`] and their help texts (D55)
//! under them and as tooltips. Skip (or Esc) keeps everything as it is; Finish hands back the
//! options with the answers the user changed.
//!
//! Every page's controls are in the template, hidden but the shown page's; the texts' heights
//! are measured once the dialog has its font, the pages laid out from them, and the dialog made
//! as tall as its tallest page.

use std::mem::zeroed;
use std::ptr::{null, null_mut};

use rookparcel_frontend::lang::{self, H, S};
use rookparcel_frontend::options::Options;
use rookparcel_frontend::pref_pages::{Control, Features, Item, TextKind};
use rookparcel_frontend::setup_guide::{self, Answers, Guide};
use rookparcel_frontend::tr;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_GRAYTEXT, CreateFontIndirectW, DT_CALCRECT, DT_NOPREFIX, DT_WORDBREAK, DeleteObject,
    DrawTextW, FW_BOLD, GetDC, GetObjectW, GetSysColor, GetSysColorBrush, HDC, HFONT, LOGFONTW, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::BST_CHECKED;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON,
    BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, DLGTEMPLATE, DM_SETDEFID,
    DialogBoxIndirectParamW, ES_AUTOHSCROLL, EndDialog, GetClientRect, GetDlgItem, GetWindowLongPtrW,
    GetWindowRect, IDCANCEL, IDOK, MapDialogRect, MoveWindow, SW_HIDE, SW_SHOW, SWP_NOMOVE, SWP_NOZORDER,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY,
    WM_GETFONT, WM_INITDIALOG, WM_SETFONT, WS_BORDER, WS_CHILD, WS_GROUP, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use crate::dialogs::{self, BUTTON, COMBOBOX, DWLP_USER, EDIT, STATIC, Template};
use crate::util::{hiword, loword, send, wide, window_text};

const ID_STEP: u16 = 100;
const ID_LINE: u16 = 101;
const ID_BACK: u16 = 102;
const ID_NEXT: u16 = 103;
/// Finish is OK, Skip is Cancel (Esc and the close box skip too).
const ID_FINISH: u16 = IDOK as u16;
const ID_SKIP: u16 = IDCANCEL as u16;

/// A page's controls are numbered from `1000 + 200 * page`: its title, its text, then ten ids
/// per item ([`item_id`]).
const fn page_base(page: usize) -> u16 {
    1000 + 200 * page as u16
}
const fn item_id(page: usize, item: usize) -> u16 {
    page_base(page) + 10 + 10 * item as u16
}
/// Within an item's ten: the control (the first option button), its caption, its "..." button
/// and the help text or note under it.
const CAPTION: u16 = 5;
const BROWSE: u16 = 6;
const BELOW: u16 = 7;

/// The dialog's width and the room above the buttons it starts with (dialog units); it grows
/// to the tallest page.
const W: i16 = 320;
const H0: i16 = 200;
const MARGIN: i32 = 10;
const INDENT: i32 = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Check,
    Edit,
    Combo,
    /// Option buttons, one per choice.
    Options,
}

/// One field's controls.
struct Ctl {
    key: &'static str,
    id: u16,
    kind: Kind,
    /// The choices' values (option buttons and drop-downs).
    values: Vec<String>,
    needs: &'static [&'static str],
    help: Option<H>,
    /// The folder picker's title for a folder field.
    folder: Option<S>,
}

/// One control the layout places: its id and what kind of row it is.
#[derive(Clone, Copy)]
enum Row {
    /// A text measured for its height, at this indent.
    Text {
        id: u16,
        indent: i32,
        help: bool,
    },
    Check {
        id: u16,
        indent: i32,
    },
    /// An edit box with a "..." button when `browse`.
    Edit {
        id: u16,
        indent: i32,
        browse: bool,
    },
    Combo {
        id: u16,
    },
    Gap,
}

struct State {
    guide: Guide,
    opts: Options,
    ctls: Vec<Ctl>,
    /// Each page's rows, in order.
    rows: Vec<Vec<Row>>,
    page: usize,
    /// Finish's options.
    result: Option<Options>,
    /// The last refused answer (also shown in a message box outside tests).
    error: Option<(&'static str, String)>,
    tips: Option<crate::help::Tips>,
    bold: HFONT,
}

/// Shows the guide modally over `owner` (none at a first start) for `opts`, with
/// `features.desktop` giving the page about this computer (`opts.integration` as the system
/// has it). The options with the answers on Finish, `None` on Skip.
pub fn show(owner: HWND, opts: Options, features: Features) -> Option<Options> {
    let mut st = State::new(opts, features);
    let template = template(&mut st);
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(proc_),
            &mut st as *mut State as LPARAM,
        );
    }
    st.result
}

impl State {
    fn new(opts: Options, features: Features) -> State {
        State {
            guide: Guide::new(features),
            opts,
            ctls: Vec::new(),
            rows: Vec::new(),
            page: 0,
            result: None,
            error: None,
            tips: None,
            bold: null_mut(),
        }
    }

    fn ctl(&self, key: &str) -> Option<&Ctl> {
        self.ctls.iter().find(|c| c.key == key)
    }
}

/// Every page's controls, hidden; fills `st.ctls` and `st.rows`.
fn template(st: &mut State) -> Vec<u16> {
    const C: u32 = WS_CHILD;
    const TAB: u32 = C | WS_TABSTOP;
    let mut t = Template::new(W, H0 + 30);
    let r = [0, 0, 10, 10];
    t.item(STATIC, ID_STEP, C | WS_VISIBLE, r, "");
    let (ctls, rows) = (&mut st.ctls, &mut st.rows);
    for (p, page) in st.guide.pages.iter().enumerate() {
        let base = page_base(p);
        let mut page_rows = Vec::new();
        t.item(STATIC, base, C, r, lang::text(page.title));
        t.item(STATIC, base + 1, C, r, lang::text(setup_guide::page_text(page)));
        page_rows.push(Row::Text { id: base, indent: 0, help: false });
        page_rows.push(Row::Text { id: base + 1, indent: 0, help: false });
        page_rows.push(Row::Gap);
        for (i, item) in page.items.iter().enumerate() {
            let id = item_id(p, i);
            match item {
                Item::Field(f) => {
                    let indent =
                        if f.indent || (f.label.is_none() && !f.needs.is_empty()) { INDENT } else { 0 };
                    let mut ctl = Ctl {
                        key: f.key,
                        id,
                        kind: Kind::Check,
                        values: Vec::new(),
                        needs: f.needs,
                        help: f.help_key(),
                        folder: None,
                    };
                    let caption = f.label.map(lang::text).unwrap_or("");
                    match f.control {
                        Control::Check { .. } => {
                            t.item(BUTTON, id, TAB | BS_AUTOCHECKBOX as u32, r, caption);
                            page_rows.push(Row::Check { id, indent });
                        }
                        Control::Choice { .. } => {
                            if !caption.is_empty() {
                                t.item(STATIC, id + CAPTION, C, r, caption);
                                page_rows.push(Row::Text { id: id + CAPTION, indent, help: false });
                            }
                            match setup_guide::as_options(&f.control, &st.opts) {
                                Some(choices) => {
                                    ctl.kind = Kind::Options;
                                    for (k, (value, text)) in choices.into_iter().enumerate() {
                                        let group = if k == 0 { WS_GROUP } else { 0 };
                                        let style = TAB | group | BS_AUTORADIOBUTTON as u32;
                                        t.item(BUTTON, id + k as u16, style, r, &text);
                                        page_rows.push(Row::Check { id: id + k as u16, indent: indent + 6 });
                                        ctl.values.push(value);
                                    }
                                }
                                None => {
                                    ctl.kind = Kind::Combo;
                                    let style = TAB | WS_VSCROLL | CBS_DROPDOWNLIST as u32;
                                    t.item(COMBOBOX, id, style, [0, 0, 10, 80], "");
                                    page_rows.push(Row::Combo { id });
                                }
                            }
                        }
                        Control::Number { .. } | Control::Text { .. } => {
                            ctl.kind = Kind::Edit;
                            if !caption.is_empty() {
                                t.item(STATIC, id + CAPTION, C, r, caption);
                                page_rows.push(Row::Text { id: id + CAPTION, indent, help: false });
                            }
                            t.item(EDIT, id, TAB | WS_BORDER | ES_AUTOHSCROLL as u32, r, "");
                            if let Control::Text { kind: TextKind::Folder(title), .. } = f.control {
                                ctl.folder = Some(title);
                                t.item(BUTTON, id + BROWSE, TAB | BS_PUSHBUTTON as u32, r, "...");
                            }
                            page_rows.push(Row::Edit { id, indent, browse: ctl.folder.is_some() });
                        }
                    }
                    // the help text under the control; option buttons have the page's note,
                    // a field without a label shares its box's text, shown there
                    let below = ctl.kind != Kind::Options && f.label.is_some();
                    if let (true, Some(h)) = (below, ctl.help) {
                        t.item(STATIC, id + BELOW, C, r, lang::help(h));
                        let indent = if ctl.kind == Kind::Check { indent + INDENT } else { indent };
                        page_rows.push(Row::Text { id: id + BELOW, indent, help: true });
                    }
                    page_rows.push(Row::Gap);
                    ctls.push(ctl);
                }
                Item::Note(text) => {
                    t.item(STATIC, id + BELOW, C, r, &text());
                    page_rows.push(Row::Text { id: id + BELOW, indent: 0, help: false });
                    page_rows.push(Row::Gap);
                }
                _ => {}
            }
        }
        rows.push(page_rows);
    }
    let push = TAB | WS_VISIBLE | BS_PUSHBUTTON as u32;
    t.item(STATIC, ID_LINE, C | WS_VISIBLE | 0x10 /* SS_ETCHEDHORZ */, r, "");
    t.item(BUTTON, ID_SKIP, push, r, tr!(GuideSkip));
    t.item(BUTTON, ID_BACK, push, r, tr!(GuideBack));
    t.item(BUTTON, ID_NEXT, TAB | WS_VISIBLE | BS_DEFPUSHBUTTON as u32, r, tr!(GuideNext));
    t.item(BUTTON, ID_FINISH, push, r, tr!(GuideFinish));
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
    unsafe { send(item(dlg, id), BM_GETCHECK, 0, 0) as u32 == BST_CHECKED }
}

unsafe fn set_check(dlg: HWND, id: u16, on: bool) {
    unsafe { send(item(dlg, id), BM_SETCHECK, on as usize, 0) };
}

/// Dialog units across to pixels.
unsafe fn dx(dlg: HWND, v: i32) -> i32 {
    let mut r = RECT { left: 0, top: 0, right: v, bottom: 0 };
    unsafe { MapDialogRect(dlg, &mut r) };
    r.right
}

/// Dialog units down to pixels.
unsafe fn dy(dlg: HWND, v: i32) -> i32 {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: v };
    unsafe { MapDialogRect(dlg, &mut r) };
    r.bottom
}

/// The height `h`'s text takes wrapped at `width` pixels, in its font.
unsafe fn text_height(h: HWND, width: i32) -> i32 {
    unsafe {
        let text = wide(&window_text(h));
        let dc: HDC = GetDC(h);
        let old = SelectObject(dc, send(h, WM_GETFONT, 0, 0) as HFONT);
        let mut r = RECT { left: 0, top: 0, right: width, bottom: 0 };
        DrawTextW(dc, text.as_ptr(), text.len() as i32 - 1, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
        SelectObject(dc, old);
        ReleaseDC(h, dc);
        r.bottom
    }
}

unsafe fn place(dlg: HWND, id: u16, x: i32, y: i32, w: i32, h: i32) {
    unsafe { MoveWindow(item(dlg, id), x, y, w, h, 0) };
}

/// Places page `p`'s controls from `top`; the bottom of the last one.
unsafe fn layout_page(dlg: HWND, st: &State, p: usize, top: i32) -> i32 {
    unsafe {
        let (x0, width) = (dx(dlg, MARGIN), dx(dlg, W as i32 - 2 * MARGIN));
        let mut y = top;
        for row in &st.rows[p] {
            match *row {
                Row::Text { id, indent, .. } => {
                    let (x, w) = (x0 + dx(dlg, indent), width - dx(dlg, indent));
                    let h = text_height(item(dlg, id), w);
                    place(dlg, id, x, y, w, h);
                    y += h + dy(dlg, 2);
                }
                Row::Check { id, indent } => {
                    let (x, w) = (x0 + dx(dlg, indent), width - dx(dlg, indent));
                    place(dlg, id, x, y, w, dy(dlg, 10));
                    y += dy(dlg, 12);
                }
                Row::Edit { id, indent, browse } => {
                    let (x, mut w) = (x0 + dx(dlg, indent), width - dx(dlg, indent));
                    if browse {
                        w -= dx(dlg, 22);
                        place(dlg, id + BROWSE, x0 + width - dx(dlg, 18), y, dx(dlg, 18), dy(dlg, 13));
                    }
                    place(dlg, id, x, y, w, dy(dlg, 13));
                    y += dy(dlg, 15);
                }
                Row::Combo { id } => {
                    place(dlg, id, x0, y, width, dy(dlg, 80));
                    y += dy(dlg, 15);
                }
                Row::Gap => y += dy(dlg, 4),
            }
        }
        y
    }
}

/// Lays every page out, makes the dialog as tall as the tallest and puts the buttons under it.
unsafe fn layout(dlg: HWND, st: &State) {
    unsafe {
        let (x0, width) = (dx(dlg, MARGIN), dx(dlg, W as i32 - 2 * MARGIN));
        place(dlg, ID_STEP, x0, dy(dlg, 7), width, dy(dlg, 9));
        let top = dy(dlg, 18);
        let bottom = (0..st.rows.len()).map(|p| layout_page(dlg, st, p, top)).max().unwrap_or(top);
        let line = bottom.max(dy(dlg, H0 as i32)) + dy(dlg, 4);
        place(dlg, ID_LINE, x0, line, width, 2);
        let (by, bh, bw) = (line + dy(dlg, 7), dy(dlg, 14), dx(dlg, 54));
        place(dlg, ID_SKIP, x0, by, bw, bh);
        let right = x0 + width;
        place(dlg, ID_FINISH, right - bw, by, bw, bh);
        place(dlg, ID_NEXT, right - 2 * bw - dx(dlg, 6), by, bw, bh);
        place(dlg, ID_BACK, right - 3 * bw - dx(dlg, 6), by, bw, bh);
        // the client area down to under the buttons
        let mut client: RECT = zeroed();
        GetClientRect(dlg, &mut client);
        let mut win: RECT = zeroed();
        GetWindowRect(dlg, &mut win);
        let want = by + bh + dy(dlg, 7);
        let height = win.bottom - win.top + want - client.bottom;
        SetWindowPos(dlg, null_mut(), 0, 0, win.right - win.left, height, SWP_NOMOVE | SWP_NOZORDER);
    }
}

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            init(dlg, &mut *(lp as *mut State));
            return 0;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut State;
        if st.is_null() {
            return 0;
        }
        let st = &mut *st;
        match msg {
            WM_COMMAND => command(dlg, st, loword(wp), hiword(wp) as u32) as isize,
            // the help texts under the controls in grey
            WM_CTLCOLORSTATIC => {
                let id = windows_sys::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(lp as HWND) as u16;
                let help = st
                    .rows
                    .iter()
                    .flatten()
                    .any(|r| matches!(r, Row::Text { id: i, help: true, .. } if *i == id));
                if !help {
                    return 0;
                }
                let dc = wp as HDC;
                SetTextColor(dc, GetSysColor(COLOR_GRAYTEXT));
                SetBkMode(dc, TRANSPARENT as i32);
                GetSysColorBrush(COLOR_BTNFACE) as isize
            }
            WM_DESTROY => {
                if !st.bold.is_null() {
                    DeleteObject(st.bold);
                    st.bold = null_mut();
                }
                0
            }
            _ => 0,
        }
    }
}

unsafe fn init(dlg: HWND, st: &mut State) {
    unsafe {
        set_text(dlg, tr!(GuideTitle));
        // the page titles in bold
        let font = send(dlg, WM_GETFONT, 0, 0) as HFONT;
        let mut lf: LOGFONTW = zeroed();
        if !font.is_null()
            && GetObjectW(font, size_of::<LOGFONTW>() as i32, &mut lf as *mut LOGFONTW as *mut _) != 0
        {
            lf.lfWeight = FW_BOLD as i32;
            st.bold = CreateFontIndirectW(&lf);
            for p in 0..st.rows.len() {
                send(item(dlg, page_base(p)), WM_SETFONT, st.bold as usize, 0);
            }
        }
        for c in &st.ctls {
            if c.kind == Kind::Combo {
                if let Some(Item::Field(f)) = field_item(st, c.key) {
                    if let Control::Choice { choices, .. } = f.control {
                        for (_, text) in choices(&st.opts) {
                            let w = wide(&text);
                            send(item(dlg, c.id), CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
                        }
                    }
                }
            }
        }
        let values = st.guide.write(&st.opts);
        for (key, value) in values {
            if let Some(c) = st.ctl(key) {
                write_value(dlg, c, &value);
            }
        }
        layout(dlg, st);
        let tips = crate::help::Tips::new(dlg);
        for c in &st.ctls {
            let Some(h) = c.help else { continue };
            let text = lang::help(h);
            let mut ids = vec![c.id, c.id + CAPTION, c.id + BROWSE];
            if c.kind == Kind::Options {
                ids.extend((1..c.values.len() as u16).map(|k| c.id + k));
            }
            for id in ids {
                let h = item(dlg, id);
                if !h.is_null() {
                    tips.add(dlg, h, id, text);
                }
            }
        }
        st.tips = Some(tips);
        // each page is a set of its own; the buttons are another
        crate::fit::fit_texts(dlg, |id| {
            Some(if id >= page_base(0) { ((id - page_base(0)) / 200) as i32 } else { -1 })
        });
        update_enabled(dlg, st);
        show_page(dlg, st, 0);
    }
}

fn field_item<'a>(st: &'a State, key: &str) -> Option<&'a Item> {
    st.guide.pages.iter().flat_map(|p| &p.items).find(|i| matches!(i, Item::Field(f) if f.key == key))
}

unsafe fn write_value(dlg: HWND, c: &Ctl, value: &str) {
    unsafe {
        match c.kind {
            Kind::Check => set_check(dlg, c.id, value == "true"),
            Kind::Edit => set_text(item(dlg, c.id), value),
            Kind::Combo => {
                let i = c.values.iter().position(|v| v == value).unwrap_or(0);
                send(item(dlg, c.id), CB_SETCURSEL, i, 0);
            }
            Kind::Options => {
                let i = c.values.iter().position(|v| v == value).unwrap_or(0);
                for k in 0..c.values.len() {
                    set_check(dlg, c.id + k as u16, k == i);
                }
            }
        }
    }
}

unsafe fn read_value(dlg: HWND, c: &Ctl) -> String {
    unsafe {
        match c.kind {
            Kind::Check => checked(dlg, c.id).to_string(),
            Kind::Edit => window_text(item(dlg, c.id)),
            Kind::Combo => {
                let i = send(item(dlg, c.id), CB_GETCURSEL, 0, 0).max(0) as usize;
                c.values.get(i).cloned().unwrap_or_default()
            }
            Kind::Options => (0..c.values.len())
                .find(|&k| checked(dlg, c.id + k as u16))
                .and_then(|k| c.values.get(k).cloned())
                .unwrap_or_default(),
        }
    }
}

/// The answers as the controls have them.
unsafe fn answers(dlg: HWND, st: &State) -> Answers {
    st.ctls.iter().map(|c| (c.key, unsafe { read_value(dlg, c) })).collect()
}

/// Greys a field whose boxes are not all ticked (the completed folder until Move is).
unsafe fn update_enabled(dlg: HWND, st: &State) {
    unsafe {
        let on = |key: &str| {
            st.ctl(key).is_some_and(|c| match c.kind {
                Kind::Check => checked(dlg, c.id),
                _ => {
                    let v = read_value(dlg, c);
                    !v.is_empty() && c.values.first() != Some(&v)
                }
            })
        };
        for c in st.ctls.iter().filter(|c| !c.needs.is_empty()) {
            let enabled = c.needs.iter().all(|k| on(k)) as i32;
            for id in [c.id, c.id + CAPTION, c.id + BROWSE, c.id + BELOW] {
                let h = item(dlg, id);
                if !h.is_null() {
                    EnableWindow(h, enabled);
                }
            }
        }
    }
}

/// Every control a page's rows name, with their extras.
fn page_ids(st: &State, p: usize) -> Vec<u16> {
    let mut ids = Vec::new();
    for row in &st.rows[p] {
        match *row {
            Row::Text { id, .. } | Row::Check { id, .. } | Row::Combo { id } => ids.push(id),
            Row::Edit { id, browse, .. } => {
                ids.push(id);
                if browse {
                    ids.push(id + BROWSE);
                }
            }
            Row::Gap => {}
        }
    }
    ids
}

unsafe fn show_page(dlg: HWND, st: &mut State, page: usize) {
    unsafe {
        let count = st.rows.len();
        let page = page.min(count - 1);
        for p in 0..count {
            let shown = p == page;
            for id in page_ids(st, p) {
                let h = item(dlg, id);
                ShowWindow(h, if shown { SW_SHOW } else { SW_HIDE });
                if let Some(tips) = &st.tips {
                    tips.place(dlg, h, id, shown);
                }
            }
        }
        st.page = page;
        set_text(item(dlg, ID_STEP), &setup_guide::step_text(page, count));
        EnableWindow(item(dlg, ID_BACK), (page > 0) as i32);
        let last = page + 1 == count;
        EnableWindow(item(dlg, ID_NEXT), !last as i32);
        // Enter goes on, and finishes on the last page
        let default = if last { ID_FINISH } else { ID_NEXT };
        send(dlg, DM_SETDEFID, default as usize, 0);
    }
}

unsafe fn command(dlg: HWND, st: &mut State, id: u16, code: u32) -> bool {
    unsafe {
        match id {
            ID_NEXT if code == BN_CLICKED => show_page(dlg, st, st.page + 1),
            ID_BACK if code == BN_CLICKED => show_page(dlg, st, st.page.saturating_sub(1)),
            ID_FINISH => finish(dlg, st),
            ID_SKIP => {
                st.result = None;
                EndDialog(dlg, IDCANCEL as isize);
            }
            _ if id >= page_base(0) && id % 10 == BROWSE && code == BN_CLICKED => {
                let Some(c) = st.ctls.iter().find(|c| c.id + BROWSE == id) else { return false };
                let Some(title) = c.folder else { return false };
                let now = window_text(item(dlg, c.id));
                if let Some(p) = crate::create::pick_folder_at(dlg, lang::text(title), Some(&now)) {
                    set_text(item(dlg, c.id), &p);
                }
            }
            _ if id >= page_base(0) && code == BN_CLICKED => update_enabled(dlg, st),
            _ => return false,
        }
        true
    }
}

/// Finish: the answers applied, or the page of the one refused with a message.
unsafe fn finish(dlg: HWND, st: &mut State) {
    unsafe {
        let answers = answers(dlg, st);
        let mut opts = st.opts.clone();
        match st.guide.apply(&answers, &mut opts) {
            Ok(()) => {
                st.error = None;
                st.result = Some(opts);
                EndDialog(dlg, IDOK as isize);
            }
            Err((key, message)) => {
                if let Some(p) = st.guide.page_of(key) {
                    show_page(dlg, st, p);
                }
                if !cfg!(test) {
                    dialogs::error(dlg, &message);
                }
                if let Some(c) = st.ctl(key) {
                    SetFocus(item(dlg, c.id));
                }
                st.error = Some((key, message));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! The guide built from its template, hidden and modeless, driven through its messages.

    use rookparcel_frontend::prefs::Prefs;
    use rookparcel_frontend::setup_guide::{
        OLDER, OLDER_CONNECTION_SPEED, OLDER_CONNECTIONS, OLDER_PER_TORRENT, ROUTER, SMART_DISK,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateDialogIndirectParamW, DestroyWindow, GWL_STYLE, GetWindowLongW, SendMessageW,
    };

    use super::*;

    const ALL: Features =
        Features { tray: true, desktop: true, packaged: false, updates: true, alternate_rows: true };

    struct Fixture {
        dlg: HWND,
        st: Box<State>,
    }

    impl Fixture {
        fn new(opts: Options, features: Features) -> Fixture {
            let mut st = Box::new(State::new(opts, features));
            let template = template(&mut st);
            let dlg = unsafe {
                CreateDialogIndirectParamW(
                    GetModuleHandleW(null()),
                    template.as_ptr() as *const DLGTEMPLATE,
                    null_mut(),
                    Some(proc_),
                    &mut *st as *mut State as LPARAM,
                )
            };
            assert!(!dlg.is_null(), "the template does not load");
            Fixture { dlg, st }
        }

        fn click(&self, id: u16) {
            unsafe { SendMessageW(self.dlg, WM_COMMAND, id as usize | (BN_CLICKED as usize) << 16, 0) };
        }

        fn ctl(&self, key: &str) -> u16 {
            self.st.ctl(key).unwrap_or_else(|| panic!("no control {key}")).id
        }

        fn check(&self, key: &str, on: bool) {
            unsafe { set_check(self.dlg, self.ctl(key), on) };
            self.click(self.ctl(key));
        }

        fn set(&self, key: &str, s: &str) {
            unsafe { set_text(item(self.dlg, self.ctl(key)), s) };
        }

        fn enabled(&self, id: u16) -> bool {
            unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(item(self.dlg, id)) != 0
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.dlg) };
        }
    }

    /// The control's own visible style (the hidden dialog makes `IsWindowVisible` false for all).
    fn shown(f: &Fixture, id: u16) -> bool {
        unsafe { GetWindowLongW(item(f.dlg, id), GWL_STYLE) as u32 & WS_VISIBLE != 0 }
    }

    fn defaults() -> Options {
        Options::load(&Prefs::default(), r"C:\Downloads")
    }

    #[test]
    fn every_page_and_control_exists() {
        let f = Fixture::new(defaults(), ALL);
        assert_eq!(f.st.rows.len(), 4);
        for id in [ID_STEP, ID_BACK, ID_NEXT, ID_FINISH, ID_SKIP] {
            assert!(!unsafe { item(f.dlg, id) }.is_null(), "control {id}");
        }
        let keys: Vec<&str> = f.st.ctls.iter().map(|c| c.key).collect();
        assert_eq!(
            keys,
            [
                "dir.download",
                "dir.move_completed",
                "dir.completed",
                ROUTER,
                "auto_rate_limit",
                SMART_DISK,
                "assoc.autostart",
                "assoc.torrent",
                "assoc.magnet"
            ]
        );
        for c in &f.st.ctls {
            assert!(!unsafe { item(f.dlg, c.id) }.is_null(), "{}", c.key);
            assert!(c.help.is_some(), "{} has no help", c.key);
        }
        // the router's two answers are option buttons, and the help texts are under the boxes
        let router = f.st.ctl(ROUTER).unwrap();
        assert!(router.kind == Kind::Options && router.values.len() == 2);
        assert!(!unsafe { item(f.dlg, router.id + 1) }.is_null());
        let smart = f.ctl(SMART_DISK);
        assert_eq!(unsafe { window_text(item(f.dlg, smart + BELOW)) }, lang::help(H::GuideSmartDisk));
        // without the desktop page: three pages
        assert_eq!(Fixture::new(defaults(), Features { desktop: false, ..ALL }).st.rows.len(), 3);
    }

    #[test]
    fn next_and_back_walk_the_pages() {
        let mut f = Fixture::new(defaults(), ALL);
        assert_eq!(f.st.page, 0);
        assert!(!f.enabled(ID_BACK) && f.enabled(ID_NEXT));
        assert_eq!(unsafe { window_text(item(f.dlg, ID_STEP)) }, "Step 1 of 4");
        assert!(shown(&f, f.ctl("dir.download")) && !shown(&f, f.ctl(SMART_DISK)));
        for page in 1..4 {
            f.click(ID_NEXT);
            assert_eq!(f.st.page, page);
        }
        assert!(f.enabled(ID_BACK) && !f.enabled(ID_NEXT));
        assert!(shown(&f, f.ctl("assoc.magnet")) && !shown(&f, f.ctl("dir.download")));
        f.click(ID_BACK);
        assert_eq!(f.st.page, 2);
        assert!(shown(&f, f.ctl(SMART_DISK)));
        // the completed folder is greyed until Move is ticked
        let completed = f.ctl("dir.completed");
        assert!(!f.enabled(completed));
        f.check("dir.move_completed", true);
        assert!(f.enabled(completed) && f.enabled(completed + BROWSE));
        let _ = &mut f;
    }

    #[test]
    fn finish_writes_the_answers() {
        let f = Fixture::new(defaults(), ALL);
        f.set("dir.download", r"D:\dl");
        let router = f.ctl(ROUTER);
        unsafe {
            set_check(f.dlg, router, false);
            set_check(f.dlg, router + 1, true);
        }
        f.check(SMART_DISK, true);
        f.check("assoc.magnet", true);
        f.click(ID_FINISH);
        let o = f.st.result.clone().expect("Finish gives the options");
        assert_eq!(o.gui.download_dir, r"D:\dl");
        assert_eq!(o.settings.connections_limit, OLDER_CONNECTIONS);
        assert_eq!(o.settings.connection_speed, OLDER_CONNECTION_SPEED);
        assert_eq!(o.gui.max_peers_per_torrent, OLDER_PER_TORRENT);
        assert!(o.settings.disk_slow_downloads && o.settings.disk_slow_uploads);
        assert!(o.integration.magnet && !o.integration.torrent);
        assert_eq!(unsafe { read_value(f.dlg, f.st.ctl(ROUTER).unwrap()) }, OLDER);
    }

    #[test]
    fn a_refused_answer_shows_its_page() {
        let mut f = Fixture::new(defaults(), ALL);
        f.check("dir.move_completed", true);
        f.click(ID_NEXT);
        f.click(ID_NEXT);
        f.click(ID_FINISH);
        assert!(f.st.result.is_none());
        assert_eq!(f.st.error.as_ref().map(|e| e.0), Some("dir.completed"));
        assert_eq!(f.st.page, 0);
        let _ = &mut f;
    }

    #[test]
    fn skip_writes_nothing() {
        let f = Fixture::new(defaults(), ALL);
        f.set("dir.download", r"D:\dl");
        f.check(SMART_DISK, true);
        f.click(ID_SKIP);
        assert!(f.st.result.is_none());
        // Finish with nothing changed gives the options as they were
        let f = Fixture::new(defaults(), ALL);
        f.click(ID_FINISH);
        assert_eq!(f.st.result, Some(defaults()));
    }
}
