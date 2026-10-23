//! The Add New Torrent dialog, laid out as µTorrent 2.2.1's: one column, with Save In (the
//! recent folders), Name and the options in a box at the top and the torrent's contents under
//! it, a file tree of check boxes and priorities that takes the whole width, OK and Cancel at
//! the bottom. The dialog resizes (the list grows) and keeps its size; "Don't show this again"
//! turns it off in the preferences.
//!
//! The file tree is an owner-data list view over [`FileTree`]: only visible rows are asked for,
//! so torrents with tens of thousands of files open at once. Folders are indented rows with the
//! shell's folder icon; double-click, Enter, Left / Right or a click on the icon opens and closes
//! them. The check boxes are state images drawn with the theme at the list's DPI (on, off and
//! mixed for folders whose files differ); a click on one, or Space, toggles the rows.

use std::collections::HashMap;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use patorrent_core::metainfo::TorrentInfo;
use patorrent_frontend::add::{self, AddChoice, Check, FileTree, Setup};
use patorrent_frontend::detail::{PRIORITIES, priority_text};
use patorrent_frontend::prefs::Prefs;
use patorrent_frontend::{fmt, lang, tr};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_WINDOW, CreateCompatibleBitmap, CreateCompatibleDC, DFC_BUTTON, DFCS_BUTTON3STATE,
    DFCS_BUTTONCHECK, DFCS_CHECKED, DT_CALCRECT, DT_NOPREFIX, DT_SINGLELINE, DeleteDC, DeleteObject,
    DrawFrameControl, DrawTextW, FillRect, GetDC, GetMonitorInfoW, GetSysColorBrush, HFONT, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MapWindowPoints, MonitorFromWindow, ReleaseDC, SelectObject,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, GetDiskFreeSpaceExW,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    BP_CHECKBOX, BST_CHECKED, BST_UNCHECKED, CBS_CHECKEDNORMAL, CBS_MIXEDNORMAL, CBS_UNCHECKEDNORMAL,
    CloseThemeData, DrawThemeBackground, HIMAGELIST, ILC_COLOR24, ILC_MASK, ImageList_AddMasked,
    ImageList_Create, ImageList_Destroy, LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT,
    LVCOLUMNW, LVHITTESTINFO, LVHT_ONITEMICON, LVHT_ONITEMSTATEICON, LVIF_IMAGE, LVIF_INDENT, LVIF_STATE,
    LVIF_TEXT, LVIS_STATEIMAGEMASK, LVM_ENSUREVISIBLE, LVM_GETCOLUMNWIDTH, LVM_INSERTCOLUMNW,
    LVM_SETCALLBACKMASK, LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST,
    LVM_SUBITEMHITTEST, LVN_GETDISPINFOW, LVN_KEYDOWN, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT, LVS_SHAREIMAGELISTS, LVS_SHOWSELALWAYS, LVSIL_SMALL,
    LVSIL_STATE, NM_CLICK, NM_CUSTOMDRAW, NM_DBLCLK, NM_RETURN, NMHDR, NMITEMACTIVATE, NMLVCUSTOMDRAW,
    NMLVDISPINFOW, NMLVKEYDOWN, TOOLTIPS_CLASSW, TTF_IDISHWND, TTF_SUBCLASS, TTM_ADDTOOLW,
    TTM_SETMAXTIPWIDTH, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, OpenThemeDataForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, SetFocus, VK_ADD, VK_LEFT, VK_RIGHT, VK_SPACE, VK_SUBTRACT,
};
use windows_sys::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_OPENICON, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_GROUPBOX, BS_PUSHBUTTON,
    BeginDeferWindowPos, CB_ADDSTRING, CB_GETCURSEL, CB_GETDROPPEDCONTROLRECT, CB_GETLBTEXT, CB_GETLBTEXTLEN,
    CBN_EDITCHANGE, CBN_SELCHANGE, CBS_AUTOHSCROLL, CBS_DROPDOWN, CBS_SORT, CreatePopupMenu, CreateWindowExW,
    DLGTEMPLATE, DeferWindowPos, DestroyMenu, DialogBoxIndirectParamW, ES_AUTOHSCROLL, EndDeferWindowPos,
    EndDialog, GWL_STYLE, GetClientRect, GetCursorPos, GetDlgItem, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowRect, IDCANCEL, IDOK, MF_SEPARATOR, MF_STRING, MINMAXINFO, MapDialogRect, SIZE_MINIMIZED,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos,
    SetWindowTextW, TPM_LAYOUTRTL, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WINDOWPLACEMENT,
    WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_GETFONT, WM_GETMINMAXINFO, WM_INITDIALOG, WM_NOTIFY, WM_SIZE,
    WS_BORDER, WS_CHILD, WS_MAXIMIZEBOX, WS_POPUP, WS_TABSTOP, WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL,
};
use windows_sys::core::w;

use crate::controls;
use crate::dialogs::{self, BUTTON, COMBOBOX, Class, DWLP_MSGRESULT, DWLP_USER, EDIT, STATIC, Template};
use crate::util::{
    copy_to, from_wide, hiword, loword, point_from_lparam, rtl, scale, send, unscale, wide, window_text,
};

/// Static control styles (in `System::SystemServices`, a feature we do not otherwise need).
const SS_NOPREFIX: u32 = 0x80;
/// The static takes the mouse (so a tooltip over it sees it) instead of passing it to the dialog.
const SS_NOTIFY: u32 = 0x100;
const SS_ENDELLIPSIS: u32 = 0x4000;

const ID_SAVE: u16 = 100;
const ID_BROWSE: u16 = 101;
const ID_NAME: u16 = 102;
const ID_SKIP: u16 = 103;
const ID_START: u16 = 104;
const ID_TOP: u16 = 105;
const ID_LABEL: u16 = 106;
const ID_T_NAME: u16 = 107;
const ID_T_COMMENT: u16 = 108;
const ID_T_SIZE: u16 = 109;
const ID_T_DATE: u16 = 110;
const ID_FILES: u16 = 111;
const ID_ALL: u16 = 112;
const ID_NONE: u16 = 113;
const ID_AGAIN: u16 = 114;
const ID_BOX_SAVE: u16 = 115;
const ID_BOX_CONTENTS: u16 = 116;

/// How a control follows the window's size: moved or grown with the width or the height.
const MOVE_X: u8 = 1;
const GROW_X: u8 = 2;
const MOVE_Y: u8 = 4;
const GROW_Y: u8 = 8;
/// A drop-down: its height is the open one (`GetWindowRect` gives the closed one).
const DROP: u8 = 16;
const ANCHORS: [(u16, u8); 16] = [
    (ID_BOX_SAVE, GROW_X),
    (ID_SAVE, GROW_X | DROP),
    (ID_BROWSE, MOVE_X),
    (ID_NAME, GROW_X),
    (ID_LABEL, GROW_X | DROP),
    (ID_BOX_CONTENTS, GROW_X | GROW_Y),
    (ID_T_NAME, GROW_X),
    (ID_T_COMMENT, GROW_X),
    (ID_T_SIZE, GROW_X),
    (ID_T_DATE, GROW_X),
    (ID_ALL, MOVE_X),
    (ID_NONE, MOVE_X),
    (ID_FILES, GROW_X | GROW_Y),
    (ID_AGAIN, MOVE_Y),
    (IDOK as u16, MOVE_X | MOVE_Y),
    (IDCANCEL as u16, MOVE_X | MOVE_Y),
];

/// Context menu commands: the priorities, then check and uncheck.
const MENU_PRIO: usize = 1;
const MENU_CHECK: usize = 10;
const MENU_UNCHECK: usize = 11;

struct State {
    name: String,
    comment: String,
    date: Option<i64>,
    tree: FileTree,
    setup: Setup,
    choice: Option<AddChoice>,
    /// The shell's small icon index per lower-case extension, filled as rows are drawn.
    icons: HashMap<String, i32>,
    /// Closed and open folder.
    folder_icons: (i32, i32),
    checks: HIMAGELIST,
    /// The tooltip with the whole name and comment, when one of them shows "...".
    tip: HWND,
    /// The client size the template gave (the rects in `rects` belong to it).
    base: (i32, i32),
    /// The anchored controls with their rects at the template's size.
    rects: Vec<(HWND, RECT, u8)>,
    /// The window's smallest size: the template's.
    min: (i32, i32),
    /// The window's size when it closed (96-DPI pixels).
    window: (i32, i32),
    /// "Don't show this again" was checked when OK was pressed.
    again: bool,
}

impl State {
    fn new(ti: &TorrentInfo, setup: Setup) -> State {
        State {
            name: ti.name().to_owned(),
            comment: ti.comment().to_owned(),
            date: ti.creation_date(),
            tree: FileTree::from_torrent(ti),
            setup,
            choice: None,
            icons: HashMap::new(),
            folder_icons: (-1, -1),
            checks: 0,
            tip: null_mut(),
            base: (0, 0),
            rects: Vec::new(),
            min: (0, 0),
            window: (0, 0),
            again: false,
        }
    }
}

/// What came of the dialog.
pub struct Shown {
    /// The choices on OK; `None` on Cancel.
    pub choice: Option<AddChoice>,
    /// The size the dialog closed with (96-DPI pixels), for [`remember`].
    pub window: (i32, i32),
    /// "Don't show this again" was checked when OK was pressed.
    pub dont_show_again: bool,
}

/// Keeps the dialog's size for next time and, after "Don't show this again", turns the dialog
/// off in the preferences (`ui.show_add_dialog`; the caller saves them).
pub fn remember(prefs: &mut Prefs, shown: &Shown) {
    add::remember_window(prefs, shown.window);
    if shown.dont_show_again {
        prefs.set_bool("ui.show_add_dialog", false);
    }
}

/// Shows the dialog for `ti`.
pub fn show(owner: HWND, ti: &TorrentInfo, setup: Setup) -> Shown {
    let mut st = State::new(ti, setup);
    let template = template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(proc_),
            &mut st as *mut State as LPARAM,
        );
    }
    Shown { choice: st.choice, window: st.window, dont_show_again: st.again }
}

fn template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    const TAB: u32 = C | WS_TABSTOP;
    const LINE: u32 = TAB | WS_BORDER | ES_AUTOHSCROLL as u32;
    const PUSH: u32 = TAB | BS_PUSHBUTTON as u32;
    const CHECK: u32 = TAB | BS_AUTOCHECKBOX as u32;
    const GROUP: u32 = C | BS_GROUPBOX as u32;
    const COMBO: u32 = TAB | WS_VSCROLL | (CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32;
    // a value shows "..." for what does not fit; the whole text is in its tooltip (`tips`)
    const VALUE: u32 = C | SS_NOPREFIX | SS_NOTIFY | SS_ENDELLIPSIS;
    let list = TAB | WS_BORDER | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS;
    // 2.2.1's layout: one column, the contents' list the whole width. Everything is 7 units
    // in from the box around it; the two boxes and OK / Cancel keep 7 to the dialog's edge.
    let mut t = Template::new(420, 300);
    t.item(BUTTON, ID_BOX_SAVE, GROUP, [7, 4, 406, 78], tr!(AddSaveIn));
    t.item(COMBOBOX, ID_SAVE, COMBO, [14, 16, 370, 150], "");
    t.item(BUTTON, ID_BROWSE, PUSH, [388, 15, 18, 14], "...");
    t.item(STATIC, 0xffff, C, [14, 36, 40, 9], tr!(AddName));
    t.item(EDIT, ID_NAME, LINE, [58, 34, 348, 13], "");
    // the options in two columns, as 2.2.1 had them
    t.item(BUTTON, ID_SKIP, CHECK, [14, 53, 100, 10], tr!(AddSkipCheck));
    t.item(STATIC, 0xffff, C, [122, 54, 40, 9], tr!(AddLabel));
    t.item(COMBOBOX, ID_LABEL, COMBO | CBS_SORT as u32, [165, 52, 241, 120], "");
    t.item(BUTTON, ID_START, CHECK, [14, 67, 100, 10], tr!(AddStart));
    t.item(BUTTON, ID_TOP, CHECK, [122, 67, 160, 10], tr!(AddTopOfQueue));
    t.item(BUTTON, ID_BOX_CONTENTS, GROUP, [7, 88, 406, 186], tr!(AddContents));
    t.item(STATIC, 0xffff, C, [14, 100, 36, 9], tr!(AddTorrentName));
    t.item(STATIC, ID_T_NAME, VALUE, [54, 100, 352, 9], "");
    t.item(STATIC, 0xffff, C, [14, 111, 36, 9], tr!(AddComment));
    t.item(STATIC, ID_T_COMMENT, VALUE, [54, 111, 352, 9], "");
    t.item(STATIC, 0xffff, C, [14, 122, 36, 9], tr!(AddSize));
    t.item(STATIC, ID_T_SIZE, VALUE, [54, 122, 352, 9], "");
    t.item(STATIC, 0xffff, C, [14, 133, 36, 9], tr!(AddDate));
    // Select All / None beside the date, as in 2.2.1
    t.item(STATIC, ID_T_DATE, VALUE, [54, 133, 244, 9], "");
    t.item(BUTTON, ID_ALL, PUSH, [302, 130, 50, 14], tr!(MenuSelectAll));
    t.item(BUTTON, ID_NONE, PUSH, [356, 130, 50, 14], tr!(MenuSelectNone));
    t.item(Class::Name("SysListView32"), ID_FILES, list, [14, 148, 392, 119], "");
    t.item(BUTTON, ID_AGAIN, CHECK, [7, 281, 220, 10], tr!(AddDontShowAgain));
    t.item(BUTTON, IDOK as u16, TAB | BS_DEFPUSHBUTTON as u32, [309, 279, 50, 14], tr!(BtnOk));
    t.item(BUTTON, IDCANCEL as u16, PUSH, [363, 279, 50, 14], tr!(BtnCancel));
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

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            init(dlg, &mut *(lp as *mut State));
            crate::fit::fit_texts(dlg, |_| Some(0));
            tips(dlg, &mut *(lp as *mut State));
            resizable(dlg, &mut *(lp as *mut State));
            layout(dlg, &*(lp as *mut State));
            // focus is on OK (set by `init`)
            return 0;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut State;
        if st.is_null() {
            return 0;
        }
        let st = &mut *st;
        match msg {
            WM_COMMAND => command(dlg, st, loword(wp), hiword(wp) as u32) as isize,
            WM_NOTIFY => {
                let nm = lp as *mut NMHDR;
                if (*nm).idFrom != ID_FILES as usize {
                    return 0;
                }
                if (*nm).code == NM_CUSTOMDRAW {
                    let r = controls::selection_custom_draw(&mut *(nm as *mut NMLVCUSTOMDRAW));
                    SetWindowLongPtrW(dlg, DWLP_MSGRESULT, r);
                    return 1;
                }
                notify(dlg, st, nm);
                0
            }
            WM_CONTEXTMENU if wp as HWND == item(dlg, ID_FILES) => {
                context_menu(dlg, st, lp);
                1
            }
            WM_SIZE => {
                if wp as u32 != SIZE_MINIMIZED {
                    layout(dlg, st);
                }
                0
            }
            WM_GETMINMAXINFO => {
                if st.min != (0, 0) {
                    (*(lp as *mut MINMAXINFO)).ptMinTrackSize = POINT { x: st.min.0, y: st.min.1 };
                }
                1
            }
            WM_DESTROY => {
                // the size the dialog closed with (its normal one when maximized)
                let mut wp: WINDOWPLACEMENT = zeroed();
                wp.length = size_of::<WINDOWPLACEMENT>() as u32;
                if GetWindowPlacement(dlg, &mut wp) != 0 {
                    let (r, dpi) = (wp.rcNormalPosition, GetDpiForWindow(dlg).max(96));
                    st.window = (unscale(r.right - r.left, dpi), unscale(r.bottom - r.top, dpi));
                }
                if st.checks != 0 {
                    ImageList_Destroy(st.checks);
                    st.checks = 0;
                }
                0
            }
            _ => 0,
        }
    }
}

/// Dialog units to pixels, horizontally.
unsafe fn dlu(dlg: HWND, x: i32) -> i32 {
    let mut rc = RECT { left: 0, top: 0, right: x, bottom: 0 };
    unsafe { MapDialogRect(dlg, &mut rc) };
    rc.right
}

unsafe fn init(dlg: HWND, st: &mut State) {
    unsafe {
        set_text(dlg, tr!(AddTitle));
        let save = item(dlg, ID_SAVE);
        for d in &st.setup.save_dirs {
            let w = wide(d);
            send(save, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        set_text(save, st.setup.save_dirs.first().map_or("", String::as_str));
        set_text(item(dlg, ID_NAME), &st.name);
        set_check(dlg, ID_START, st.setup.start);
        if st.setup.in_session {
            EnableWindow(item(dlg, ID_SKIP), 0);
        }
        // a folder on this computer means nothing to a remote server: the path is typed
        if crate::remote_ui::is_remote() {
            EnableWindow(item(dlg, ID_BROWSE), 0);
        }
        let label = item(dlg, ID_LABEL);
        for l in &st.setup.labels {
            let w = wide(l);
            send(label, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }

        set_text(item(dlg, ID_T_NAME), &st.name);
        // on one line (a comment of several lines shows whole in its tooltip)
        set_text(item(dlg, ID_T_COMMENT), &st.comment.split_whitespace().collect::<Vec<_>>().join(" "));
        set_text(item(dlg, ID_T_DATE), &st.date.map(fmt::date_time).unwrap_or_default());

        let lv = item(dlg, ID_FILES);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        // Size and Priority in dialog units; Name takes the rest (`fit_columns`)
        let cols = [(tr!(ColName), 0, false), (tr!(ColSize), 50, true), (tr!(ColPriority), 48, false)];
        for (i, (title, width, right)) in cols.into_iter().enumerate() {
            let mut text = wide(title);
            let mut c: LVCOLUMNW = zeroed();
            c.mask = LVCF_TEXT | LVCF_WIDTH | LVCF_FMT;
            c.fmt = if right { LVCFMT_RIGHT } else { LVCFMT_LEFT };
            c.cx = dlu(dlg, width);
            c.pszText = text.as_mut_ptr();
            send(lv, LVM_INSERTCOLUMNW, i, &c as *const _ as LPARAM);
        }
        send(lv, LVM_SETCALLBACKMASK, LVIS_STATEIMAGEMASK as usize, 0);
        st.checks = check_images(lv);
        send(lv, LVM_SETIMAGELIST, LVSIL_STATE as usize, st.checks as LPARAM);
        let (system, closed) = shell_icon("folder", FILE_ATTRIBUTE_DIRECTORY, 0);
        let (_, open) = shell_icon("folder", FILE_ATTRIBUTE_DIRECTORY, SHGFI_OPENICON);
        st.folder_icons = (closed, open);
        if system != 0 {
            // the shell's list, shared (LVS_SHAREIMAGELISTS): never destroyed here
            send(lv, LVM_SETIMAGELIST, LVSIL_SMALL as usize, system as LPARAM);
        }
        controls::set_count(lv, st.tree.len());
        fit_columns(lv);
        update_size(dlg, st);
        SetFocus(item(dlg, IDOK as u16));
    }
}

/// A control's rect in the dialog's client coordinates (mirrored with the dialog).
unsafe fn rect_in(dlg: HWND, h: HWND) -> RECT {
    unsafe {
        let mut r: RECT = zeroed();
        GetWindowRect(h, &mut r);
        MapWindowPoints(null_mut(), dlg, &mut r as *mut RECT as *mut POINT, 2);
        r
    }
}

/// A sizing border, the template's size as the smallest, the controls' rects noted for
/// `layout`, and the remembered size (if any) around the same centre, kept on the screen.
unsafe fn resizable(dlg: HWND, st: &mut State) {
    unsafe {
        let style = GetWindowLongPtrW(dlg, GWL_STYLE);
        SetWindowLongPtrW(dlg, GWL_STYLE, style | (WS_THICKFRAME | WS_MAXIMIZEBOX) as isize);
        let keep = SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
        SetWindowPos(dlg, null_mut(), 0, 0, 0, 0, keep | SWP_FRAMECHANGED);
        let mut c: RECT = zeroed();
        GetClientRect(dlg, &mut c);
        st.base = (c.right, c.bottom);
        st.rects = ANCHORS
            .iter()
            .map(|&(id, a)| {
                let h = item(dlg, id);
                (h, rect_in(dlg, h), a)
            })
            .collect();
        let mut w: RECT = zeroed();
        GetWindowRect(dlg, &mut w);
        st.min = (w.right - w.left, w.bottom - w.top);
        let Some((rw, rh)) = st.setup.window else { return };
        let dpi = GetDpiForWindow(dlg).max(96);
        let (cx, cy) = (scale(rw, dpi).max(st.min.0), scale(rh, dpi).max(st.min.1));
        let (mut x, mut y) = (w.left - (cx - st.min.0) / 2, w.top - (cy - st.min.1) / 2);
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromWindow(dlg, MONITOR_DEFAULTTONEAREST), &mut mi) != 0 {
            let work = mi.rcWork;
            x = x.min(work.right - cx).max(work.left);
            y = y.min(work.bottom - cy).max(work.top);
        }
        SetWindowPos(dlg, null_mut(), x, y, cx, cy, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Moves and grows the anchored controls with the window (`ANCHORS`), from their rects at the
/// template's size.
unsafe fn layout(dlg: HWND, st: &State) {
    unsafe {
        if st.rects.is_empty() {
            return;
        }
        let mut c: RECT = zeroed();
        GetClientRect(dlg, &mut c);
        let (dx, dy) = (c.right - st.base.0, c.bottom - st.base.1);
        let mut dwp = BeginDeferWindowPos(st.rects.len() as i32);
        for &(h, r, a) in &st.rects {
            let x = r.left + if a & MOVE_X != 0 { dx } else { 0 };
            let right = r.right + if a & (MOVE_X | GROW_X) != 0 { dx } else { 0 };
            let y = r.top + if a & MOVE_Y != 0 { dy } else { 0 };
            let bottom = r.bottom + if a & (MOVE_Y | GROW_Y) != 0 { dy } else { 0 };
            let mut cy = bottom - y;
            if a & DROP != 0 {
                let mut d: RECT = zeroed();
                send(h, CB_GETDROPPEDCONTROLRECT, 0, &mut d as *mut _ as LPARAM);
                cy = d.bottom - d.top;
            }
            let flags = SWP_NOZORDER | SWP_NOACTIVATE;
            dwp = DeferWindowPos(dwp, h, null_mut(), x, y, (right - x).max(0), cy.max(0), flags);
        }
        EndDeferWindowPos(dwp);
        fit_columns(item(dlg, ID_FILES));
    }
}

/// The Name column takes what Size and Priority leave of the list's width (a scroll bar, when
/// there is one, is outside the client area).
unsafe fn fit_columns(lv: HWND) {
    unsafe {
        let mut c: RECT = zeroed();
        GetClientRect(lv, &mut c);
        let rest: i32 = (1..3).map(|i| send(lv, LVM_GETCOLUMNWIDTH, i, 0) as i32).sum();
        let least = scale(90, GetDpiForWindow(lv).max(96));
        send(lv, LVM_SETCOLUMNWIDTH, 0, (c.right - rest).max(least) as isize);
    }
}

/// A tooltip with the whole name and comment over the values that show "..." for part of
/// theirs (a torrent's comment is often a paragraph). Run once the texts are fitted.
unsafe fn tips(dlg: HWND, st: &mut State) {
    unsafe {
        let mut tip: HWND = null_mut();
        for (id, full) in [(ID_T_NAME, &st.name), (ID_T_COMMENT, &st.comment)] {
            let h = item(dlg, id);
            let shown = wide(&window_text(h));
            if shown.len() <= 1 {
                continue;
            }
            // the width the shown text needs on one line, against the control's
            let dc = GetDC(h);
            let old = SelectObject(dc, send(h, WM_GETFONT, 0, 0) as HFONT);
            let mut r: RECT = zeroed();
            let flags = DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX;
            DrawTextW(dc, shown.as_ptr(), shown.len() as i32 - 1, &mut r, flags);
            SelectObject(dc, old);
            ReleaseDC(h, dc);
            let mut own: RECT = zeroed();
            GetClientRect(h, &mut own);
            if r.right <= own.right {
                continue;
            }
            if tip.is_null() {
                tip = CreateWindowExW(
                    0,
                    TOOLTIPS_CLASSW,
                    null(),
                    WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
                    0,
                    0,
                    0,
                    0,
                    dlg,
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                if tip.is_null() {
                    return;
                }
                // a long comment wraps rather than running off the screen
                send(tip, TTM_SETMAXTIPWIDTH, 0, dlu(dlg, 240) as LPARAM);
            }
            // the tooltip copies the text, with its line breaks
            let mut w = wide(full);
            let mut ti: TTTOOLINFOW = zeroed();
            ti.cbSize = size_of::<TTTOOLINFOW>() as u32;
            ti.uFlags = TTF_IDISHWND | TTF_SUBCLASS;
            ti.hwnd = dlg;
            ti.uId = h as usize;
            ti.lpszText = w.as_mut_ptr();
            send(tip, TTM_ADDTOOLW, 0, &ti as *const _ as LPARAM);
        }
        st.tip = tip;
    }
}

/// The shell's small icon for a file name with `attrs`: (system image list, index).
unsafe fn shell_icon(name: &str, attrs: u32, extra: u32) -> (usize, i32) {
    let w = wide(name);
    unsafe {
        let mut sfi: SHFILEINFOW = zeroed();
        let list = SHGetFileInfoW(
            w.as_ptr(),
            attrs,
            &mut sfi,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES | extra,
        );
        (list, if list == 0 { -1 } else { sfi.iIcon })
    }
}

/// State images 1..=3: unchecked, checked and mixed check boxes, themed, at the list's DPI.
unsafe fn check_images(lv: HWND) -> HIMAGELIST {
    unsafe {
        let dpi = GetDpiForWindow(lv).max(96);
        let (cx, cy) = (scale(16, dpi), scale(16, dpi));
        let box_ = scale(13, dpi);
        let list = ImageList_Create(cx, cy, ILC_COLOR24 | ILC_MASK, 3, 0);
        let screen = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, cx * 3, cy);
        ReleaseDC(null_mut(), screen);
        let old = SelectObject(dc, bmp);
        let all = RECT { left: 0, top: 0, right: cx * 3, bottom: cy };
        FillRect(dc, &all, GetSysColorBrush(COLOR_WINDOW));
        let theme = OpenThemeDataForDpi(lv, w!("BUTTON"), dpi);
        let states = [CBS_UNCHECKEDNORMAL, CBS_CHECKEDNORMAL, CBS_MIXEDNORMAL];
        let classic = [DFCS_BUTTONCHECK, DFCS_BUTTONCHECK | DFCS_CHECKED, DFCS_BUTTON3STATE | DFCS_CHECKED];
        for i in 0..3 {
            let (x, y) = (i as i32 * cx + (cx - box_) / 2, (cy - box_) / 2);
            let mut rc = RECT { left: x, top: y, right: x + box_, bottom: y + box_ };
            if theme == 0 {
                DrawFrameControl(dc, &mut rc, DFC_BUTTON, classic[i]);
            } else {
                DrawThemeBackground(theme, dc, BP_CHECKBOX, states[i], &rc, null());
            }
        }
        if theme != 0 {
            CloseThemeData(theme);
        }
        SelectObject(dc, old);
        let window = windows_sys::Win32::Graphics::Gdi::GetSysColor(COLOR_WINDOW);
        ImageList_AddMasked(list, bmp, window);
        DeleteObject(bmp);
        DeleteDC(dc);
        list
    }
}

unsafe fn command(dlg: HWND, st: &mut State, id: u16, code: u32) -> bool {
    unsafe {
        match id {
            ID_BROWSE => {
                if let Some(p) = crate::create::pick_folder(dlg, tr!(AddPickFolder)) {
                    set_text(item(dlg, ID_SAVE), &p);
                    update_size(dlg, st);
                }
            }
            ID_SAVE if code == CBN_SELCHANGE => {
                // the edit still shows the old text while this is sent
                let combo = item(dlg, ID_SAVE);
                let i = send(combo, CB_GETCURSEL, 0, 0);
                if i >= 0 {
                    let len = send(combo, CB_GETLBTEXTLEN, i as usize, 0).max(0) as usize;
                    let mut buf = vec![0u16; len + 1];
                    send(combo, CB_GETLBTEXT, i as usize, buf.as_mut_ptr() as LPARAM);
                    set_text(combo, &from_wide(&buf));
                }
                update_size(dlg, st);
            }
            ID_SAVE if code == CBN_EDITCHANGE => update_size(dlg, st),
            ID_ALL | ID_NONE => {
                st.tree.set_all(id == ID_ALL);
                changed(dlg, st);
            }
            _ if id == IDOK as u16 => {
                if let Some(c) = collect(dlg, st) {
                    st.choice = Some(c);
                    st.again = checked(dlg, ID_AGAIN);
                    EndDialog(dlg, IDOK as isize);
                }
            }
            _ if id == IDCANCEL as u16 => {
                EndDialog(dlg, IDCANCEL as isize);
            }
            _ => return false,
        }
        true
    }
}

/// Reads the fields; `None` after telling the user what is wrong.
unsafe fn collect(dlg: HWND, st: &State) -> Option<AddChoice> {
    unsafe {
        let save_path = match add::save_folder(&window_text(item(dlg, ID_SAVE))) {
            Ok(dir) => dir,
            Err(e) => {
                dialogs::error(dlg, &e);
                SetFocus(item(dlg, ID_SAVE));
                return None;
            }
        };
        let name = window_text(item(dlg, ID_NAME)).trim().to_owned();
        Some(AddChoice {
            save_path,
            name: (!name.is_empty() && name != st.name).then_some(name),
            file_priorities: st.tree.file_priorities(),
            start: checked(dlg, ID_START),
            skip_hash_check: !st.setup.in_session && checked(dlg, ID_SKIP),
            top_of_queue: checked(dlg, ID_TOP),
            label: window_text(item(dlg, ID_LABEL)).trim().to_owned(),
        })
    }
}

/// "Size: 1.20 GB of 3.40 GB selected (disk space: 120 GB free)".
unsafe fn update_size(dlg: HWND, st: &State) {
    unsafe {
        let (sel, all) = st.tree.sizes();
        let mut text = add::size_text(sel, all);
        if let Some(free) = free_space(&window_text(item(dlg, ID_SAVE))) {
            text.push(' ');
            text.push_str(&tr!(AddDiskSpace, free = fmt::size(free)));
        }
        set_text(item(dlg, ID_T_SIZE), &text);
    }
}

/// Free bytes on the volume of `dir` (or of its nearest existing parent).
fn free_space(dir: &str) -> Option<u64> {
    let mut p = std::path::Path::new(dir.trim());
    loop {
        if p.as_os_str().is_empty() {
            return None;
        }
        if p.is_dir() {
            let w = wide(&p.to_string_lossy());
            let mut free = 0u64;
            let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, null_mut(), null_mut()) } != 0;
            return ok.then_some(free);
        }
        p = p.parent()?;
    }
}

/// After check or priority changes: repaint the rows and the size.
unsafe fn changed(dlg: HWND, st: &State) {
    unsafe {
        InvalidateRect(item(dlg, ID_FILES), null(), 0);
        update_size(dlg, st);
    }
}

/// After a folder opened or closed: new row count, the folder's row selected and focused.
unsafe fn reshaped(dlg: HWND, st: &State, row: usize) {
    unsafe {
        let lv = item(dlg, ID_FILES);
        controls::set_count(lv, st.tree.len());
        controls::set_state(lv, None, false, false);
        controls::set_state(lv, Some(row), true, true);
        send(lv, LVM_ENSUREVISIBLE, row, 0);
        // a scroll bar may have come or gone
        fit_columns(lv);
        InvalidateRect(lv, null(), 0);
    }
}

unsafe fn notify(dlg: HWND, st: &mut State, nm: *mut NMHDR) {
    unsafe {
        let lv = item(dlg, ID_FILES);
        let code = (*nm).code;
        match code {
            LVN_GETDISPINFOW => {
                let di = &mut *(nm as *mut NMLVDISPINFOW);
                display(st, &mut di.item);
            }
            NM_CLICK | NM_DBLCLK => {
                let ia = &*(nm as *const NMITEMACTIVATE);
                let mut hit: LVHITTESTINFO = zeroed();
                hit.pt = ia.ptAction;
                if send(lv, LVM_SUBITEMHITTEST, 0, &mut hit as *mut _ as LPARAM) < 0 || hit.iItem < 0 {
                    return;
                }
                let row = hit.iItem as usize;
                if hit.flags & LVHT_ONITEMSTATEICON != 0 {
                    st.tree.toggle_checks(&[row]);
                    changed(dlg, st);
                } else if (code == NM_DBLCLK || hit.flags & LVHT_ONITEMICON != 0)
                    && st.tree.toggle_expanded(row)
                {
                    reshaped(dlg, st, row);
                }
            }
            NM_RETURN => {
                if let Some(row) = controls::focused(lv) {
                    if st.tree.toggle_expanded(row) {
                        reshaped(dlg, st, row);
                    }
                }
            }
            LVN_KEYDOWN => {
                let kd = &*(nm as *const NMLVKEYDOWN);
                let Some(row) = controls::focused(lv) else { return };
                match kd.wVKey {
                    VK_SPACE => {
                        let mut rows = controls::selected(lv);
                        if rows.is_empty() {
                            rows.push(row);
                        }
                        st.tree.toggle_checks(&rows);
                        changed(dlg, st);
                    }
                    VK_RIGHT | VK_ADD => {
                        if st.tree.set_expanded(row, Some(true)) {
                            reshaped(dlg, st, row);
                        }
                    }
                    VK_LEFT | VK_SUBTRACT => {
                        if st.tree.set_expanded(row, Some(false)) {
                            reshaped(dlg, st, row);
                        } else if let Some(parent) = st.tree.parent_row(row).filter(|_| kd.wVKey == VK_LEFT) {
                            controls::set_state(lv, None, false, false);
                            controls::set_state(lv, Some(parent), true, true);
                            send(lv, LVM_ENSUREVISIBLE, parent, 0);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// One cell for the list view.
unsafe fn display(st: &mut State, it: &mut windows_sys::Win32::UI::Controls::LVITEMW) {
    let Some(row) = st.tree.row(it.iItem.max(0) as usize) else { return };
    let (depth, folder, check, file_name) = (row.depth, row.folder, row.check, row.name.to_owned());
    if it.mask & LVIF_TEXT != 0 {
        let text = match it.iSubItem {
            0 => row.name.to_owned(),
            1 => fmt::size(row.size),
            _ => row.priority.map_or(tr!(AddMixed), priority_text).to_owned(),
        };
        unsafe { copy_to(it.pszText, it.cchTextMax, &text) };
    }
    if it.iSubItem == 0 {
        if it.mask & LVIF_INDENT != 0 {
            it.iIndent = depth as i32;
        }
        if it.mask & LVIF_IMAGE != 0 {
            it.iImage = match folder {
                Some(true) => st.folder_icons.1,
                Some(false) => st.folder_icons.0,
                None => file_icon(st, &file_name),
            };
        }
        if it.mask & LVIF_STATE != 0 {
            let image = match check {
                Check::Off => 1,
                Check::On => 2,
                Check::Mixed => 3,
            };
            it.state = (it.state & !LVIS_STATEIMAGEMASK) | (image << 12);
            it.stateMask |= LVIS_STATEIMAGEMASK;
        }
    }
}

fn file_icon(st: &mut State, name: &str) -> i32 {
    let ext = name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_lowercase());
    *st.icons
        .entry(ext)
        .or_insert_with_key(|ext| unsafe { shell_icon(&format!("file.{ext}"), FILE_ATTRIBUTE_NORMAL, 0).1 })
}

/// Right-click on the files: priorities and check / uncheck for the selected rows.
unsafe fn context_menu(dlg: HWND, st: &mut State, lp: LPARAM) {
    unsafe {
        let lv = item(dlg, ID_FILES);
        let rows = controls::selected(lv);
        if rows.is_empty() {
            return;
        }
        let mut p = POINT { x: 0, y: 0 };
        if lp == -1 {
            // the menu key: under the focused row
            let mut rc: RECT = zeroed();
            GetWindowRect(lv, &mut rc);
            let cell = controls::cell_rect(lv, controls::focused(lv).unwrap_or(rows[0]), 0);
            p = POINT { x: rc.left + cell.left, y: rc.top + cell.bottom };
        } else if lp != 0 {
            (p.x, p.y) = point_from_lparam(lp);
        } else {
            GetCursorPos(&mut p);
        }
        let menu = CreatePopupMenu();
        for (i, (text, _)) in PRIORITIES.iter().enumerate() {
            let w = wide(lang::text(*text));
            AppendMenuW(menu, MF_STRING, MENU_PRIO + i, w.as_ptr());
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, MENU_CHECK, wide(tr!(MenuCheck)).as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_UNCHECK, wide(tr!(MenuUncheck)).as_ptr());
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | rtl(TPM_LAYOUTRTL),
            p.x,
            p.y,
            0,
            dlg,
            null(),
        ) as usize;
        DestroyMenu(menu);
        match cmd {
            MENU_CHECK | MENU_UNCHECK => st.tree.set_checks(&rows, cmd == MENU_CHECK),
            c if (MENU_PRIO..MENU_PRIO + PRIORITIES.len()).contains(&c) => {
                st.tree.set_priority(&rows, PRIORITIES[c - MENU_PRIO].1);
            }
            _ => return,
        }
        changed(dlg, st);
    }
}

#[cfg(test)]
mod tests {
    //! The dialog built from its template, hidden and modeless, driven through its messages
    //! and the list view's own keyboard handling.

    use patorrent_frontend::options::{GuiOptions, Options};
    use windows_sys::Win32::UI::Controls::{
        ICC_LISTVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, LVITEMW, LVM_GETITEMCOUNT,
        LVM_GETITEMTEXTW, TTM_GETTOOLCOUNT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CB_GETCOUNT, CreateDialogIndirectParamW, DestroyWindow, SendMessageW, WM_KEYDOWN,
    };

    use super::*;

    /// A multi-file torrent: `name/` with the given (path, size) files and a comment.
    fn torrent(name: &str, files: &[(&str, u64)], comment: &str) -> Vec<u8> {
        let total: u64 = files.iter().map(|f| f.1).sum();
        let pieces = total.div_ceil(16384).max(1);
        let mut b =
            format!("d7:comment{}:{comment}13:creation datei1700000000e4:infod5:filesl", comment.len())
                .into_bytes();
        for (path, size) in files {
            b.extend(format!("d6:lengthi{size}e4:pathl").as_bytes());
            for part in path.split('/') {
                b.extend(format!("{}:{part}", part.len()).as_bytes());
            }
            b.extend(b"ee");
        }
        b.extend(
            format!("e4:name{}:{name}12:piece lengthi16384e6:pieces{}:", name.len(), pieces * 20).as_bytes(),
        );
        b.extend(std::iter::repeat_n(b'a', pieces as usize * 20));
        b.extend(b"ee");
        b
    }

    struct Fixture {
        dlg: HWND,
        st: Box<State>,
    }

    impl Fixture {
        fn new(files: &[(&str, u64)], setup: Setup) -> Fixture {
            Fixture::with_comment(files, setup, "a comment")
        }

        fn with_comment(files: &[(&str, u64)], setup: Setup, comment: &str) -> Fixture {
            unsafe {
                let icc = INITCOMMONCONTROLSEX {
                    dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                    dwICC: ICC_LISTVIEW_CLASSES,
                };
                InitCommonControlsEx(&icc);
            }
            let ti = TorrentInfo::from_bytes(&torrent("Pack", files, comment)).expect("test torrent");
            let mut st = Box::new(State::new(&ti, setup));
            let template = template();
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

        fn lv(&self) -> HWND {
            unsafe { item(self.dlg, ID_FILES) }
        }

        fn click(&self, id: u16) {
            unsafe { SendMessageW(self.dlg, WM_COMMAND, id as usize, 0) };
        }

        fn text(&self, id: u16) -> String {
            unsafe { window_text(item(self.dlg, id)) }
        }

        fn rows(&self) -> usize {
            unsafe { send(self.lv(), LVM_GETITEMCOUNT, 0, 0) as usize }
        }

        fn cell(&self, row: usize, sub: i32) -> String {
            let mut buf = [0u16; 260];
            unsafe {
                let mut it: LVITEMW = zeroed();
                it.iSubItem = sub;
                it.pszText = buf.as_mut_ptr();
                it.cchTextMax = buf.len() as i32;
                send(self.lv(), LVM_GETITEMTEXTW, row, &mut it as *mut _ as LPARAM);
            }
            from_wide(&buf)
        }

        /// Selects and focuses `rows` (the first focused), as the user would.
        fn select(&self, rows: &[usize]) {
            unsafe {
                controls::set_state(self.lv(), None, false, false);
                for (i, &r) in rows.iter().enumerate() {
                    controls::set_state(self.lv(), Some(r), true, i == 0);
                }
            }
        }

        /// A key pressed in the list view (it notifies the dialog with `LVN_KEYDOWN`).
        fn key(&self, vk: u16) {
            unsafe { SendMessageW(self.lv(), WM_KEYDOWN, vk as usize, 0) };
        }

        fn prios(&self) -> Vec<u8> {
            self.st.tree.file_priorities().iter().map(|p| p.0).collect()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.dlg) };
        }
    }

    const FILES: [(&str, u64); 4] =
        [("readme.txt", 100), ("cd1/a.bin", 20_000), ("cd1/b.bin", 30_000), ("cd2/c.bin", 40_000)];

    fn setup() -> Setup {
        Setup {
            save_dirs: vec![r"C:\Downloads".into(), r"D:\Recent".into()],
            labels: vec!["Linux".into(), "Music".into()],
            start: true,
            in_session: false,
            window: None,
        }
    }

    #[test]
    fn shows_the_torrent_and_its_files() {
        let f = Fixture::new(&FILES, setup());
        let dlg = f.dlg;
        unsafe {
            let ids =
                [ID_SAVE, ID_BROWSE, ID_NAME, ID_SKIP, ID_START, ID_TOP, ID_LABEL, ID_ALL, ID_NONE, ID_AGAIN];
            for id in ids {
                assert!(!item(dlg, id).is_null(), "control {id}");
            }
            assert_eq!(send(item(dlg, ID_SAVE), CB_GETCOUNT, 0, 0), 2);
            assert_eq!(send(item(dlg, ID_LABEL), CB_GETCOUNT, 0, 0), 2);
            assert!(checked(dlg, ID_START) && !checked(dlg, ID_SKIP) && !checked(dlg, ID_TOP));
        }
        assert_eq!(f.text(ID_SAVE), r"C:\Downloads");
        assert_eq!(f.text(ID_NAME), "Pack");
        assert_eq!(f.text(ID_T_NAME), "Pack");
        assert_eq!(f.text(ID_T_COMMENT), "a comment");
        assert!(f.text(ID_T_DATE).starts_with("2023-11-1"), "{}", f.text(ID_T_DATE));
        assert!(f.text(ID_T_SIZE).starts_with("88.0 kB (disk space: "), "{}", f.text(ID_T_SIZE));
        // readme.txt, cd1, a.bin, b.bin, cd2, c.bin
        assert_eq!(f.rows(), 6);
        assert_eq!(f.cell(1, 0), "cd1");
        assert_eq!(f.cell(1, 1), "48.8 kB");
        assert_eq!(f.cell(2, 0), "a.bin");
        assert_eq!(f.cell(2, 2), "Normal");
    }

    #[test]
    fn a_long_comment_stays_in_its_box_with_a_tooltip() {
        let short = Fixture::new(&FILES, setup());
        assert!(short.st.tip.is_null(), "a comment that fits needs no tooltip");
        let comment = "Comment: CachyOS ISO - Edition: Desktop, Version: 260809, Note: Torrent Created on a \
                       build box with a long comment\nand a second line";
        let f = Fixture::with_comment(&FILES, setup(), comment);
        unsafe {
            // never widened past the contents box: it ends where the list does
            let (mut value, mut list) = (zeroed::<RECT>(), zeroed::<RECT>());
            GetWindowRect(item(f.dlg, ID_T_COMMENT), &mut value);
            GetWindowRect(item(f.dlg, ID_FILES), &mut list);
            assert!(
                value.right <= list.right,
                "the value ends at {}, the list at {}",
                value.right,
                list.right
            );
            assert!(!f.st.tip.is_null(), "the whole comment is in a tooltip");
            assert_eq!(send(f.st.tip, TTM_GETTOOLCOUNT, 0, 0), 1);
        }
        assert_eq!(f.text(ID_T_COMMENT), comment.replace('\n', " "));
    }

    #[test]
    fn the_list_and_the_buttons_follow_the_window_size() {
        let f = Fixture::new(&FILES, setup());
        let dlg = f.dlg;
        unsafe {
            assert_ne!(GetWindowLongPtrW(dlg, GWL_STYLE) as u32 & WS_THICKFRAME, 0, "a sizing border");
            let mut w: RECT = zeroed();
            GetWindowRect(dlg, &mut w);
            let (cx, cy) = (w.right - w.left, w.bottom - w.top);
            let mut mmi: MINMAXINFO = zeroed();
            SendMessageW(dlg, WM_GETMINMAXINFO, 0, &mut mmi as *mut _ as LPARAM);
            let min = (mmi.ptMinTrackSize.x, mmi.ptMinTrackSize.y);
            assert_eq!(min, (cx, cy), "the template's size is the smallest");
            let at = |id: u16| rect_in(dlg, item(dlg, id));
            let (list, ok, none, save) = (at(ID_FILES), at(IDOK as u16), at(ID_NONE), at(ID_SAVE));
            let (again, box_, drop) = (at(ID_AGAIN), at(ID_BOX_CONTENTS), at(ID_LABEL));
            let mut dropped: RECT = zeroed();
            send(item(dlg, ID_LABEL), CB_GETDROPPEDCONTROLRECT, 0, &mut dropped as *mut _ as LPARAM);
            let open_height = dropped.bottom - dropped.top;

            SetWindowPos(
                dlg,
                null_mut(),
                0,
                0,
                cx + 100,
                cy + 80,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let l = at(ID_FILES);
            assert_eq!(
                (l.left, l.top, l.right, l.bottom),
                (list.left, list.top, list.right + 100, list.bottom + 80)
            );
            let b = at(ID_BOX_CONTENTS);
            assert_eq!((b.right, b.bottom), (box_.right + 100, box_.bottom + 80));
            let o = at(IDOK as u16);
            assert_eq!((o.left, o.top, o.right - o.left), (ok.left + 100, ok.top + 80, ok.right - ok.left));
            let n = at(ID_NONE);
            assert_eq!((n.left, n.top), (none.left + 100, none.top));
            let s = at(ID_SAVE);
            assert_eq!((s.left, s.right), (save.left, save.right + 100));
            let a = at(ID_AGAIN);
            assert_eq!((a.left, a.top), (again.left, again.top + 80));
            // a grown drop-down still opens as far
            let d = at(ID_LABEL);
            assert_eq!(
                (d.left, d.right, d.bottom - d.top),
                (drop.left, drop.right + 100, drop.bottom - drop.top)
            );
            send(item(dlg, ID_LABEL), CB_GETDROPPEDCONTROLRECT, 0, &mut dropped as *mut _ as LPARAM);
            assert_eq!(dropped.bottom - dropped.top, open_height);
            // the Name column fills what Size and Priority leave
            let lv = item(dlg, ID_FILES);
            let mut c: RECT = zeroed();
            GetClientRect(lv, &mut c);
            let widths: Vec<i32> = (0..3).map(|i| send(lv, LVM_GETCOLUMNWIDTH, i, 0) as i32).collect();
            assert_eq!(widths.iter().sum::<i32>(), c.right, "{widths:?}");
        }
    }

    #[test]
    fn the_size_is_remembered() {
        let f = Fixture::new(&FILES, Setup { window: Some((900, 700)), ..setup() });
        let (mut w, mut c) = unsafe { (zeroed::<RECT>(), zeroed::<RECT>()) };
        unsafe {
            GetWindowRect(f.dlg, &mut w);
            let dpi = GetDpiForWindow(f.dlg).max(96);
            assert_eq!((w.right - w.left, w.bottom - w.top), (scale(900, dpi), scale(700, dpi)));
            // the list grew with it
            GetClientRect(item(f.dlg, ID_FILES), &mut c);
            assert!(c.right > scale(600, dpi), "list {} wide", c.right);
        }
        // a size below the template's is lifted to it
        let small = Fixture::new(&FILES, Setup { window: Some((100, 100)), ..setup() });
        unsafe { GetWindowRect(small.dlg, &mut w) };
        assert_eq!((w.right - w.left, w.bottom - w.top), small.st.min);

        // what the caller keeps: the size, and the preference after "Don't show this again"
        let mut prefs = Prefs::default();
        remember(&mut prefs, &Shown { choice: None, window: (700, 500), dont_show_again: false });
        assert!(GuiOptions::load(&prefs, "x").show_add_dialog);
        assert_eq!(Setup::new(&GuiOptions::load(&prefs, "x"), &prefs, vec![]).window, Some((700, 500)));
        remember(&mut prefs, &Shown { choice: None, window: (0, 0), dont_show_again: true });
        assert!(!GuiOptions::load(&prefs, "x").show_add_dialog);
        assert_eq!(add::window_size(&prefs), Some((700, 500)), "a zero size is not kept");
    }

    #[test]
    fn ok_notes_dont_show_again() {
        let mut f = Fixture::new(&FILES, setup());
        unsafe { set_check(f.dlg, ID_AGAIN, true) };
        f.click(IDOK as u16);
        assert!(f.st.choice.take().is_some() && f.st.again);
    }

    #[test]
    fn checkboxes_set_the_file_priorities() {
        let f = Fixture::new(&FILES, setup());
        // Space on a file unchecks it
        f.select(&[0]);
        f.key(VK_SPACE);
        assert_eq!(f.prios(), [0, 4, 4, 4]);
        assert_eq!(f.cell(0, 2), "Don't Download");
        assert!(f.text(ID_T_SIZE).starts_with("87.9 kB of 88.0 kB selected"), "{}", f.text(ID_T_SIZE));
        // and on again
        f.key(VK_SPACE);
        assert_eq!(f.prios(), [4, 4, 4, 4]);

        // the context menu's priority for several rows
        let mut f = f;
        f.st.tree.set_priority(&[2, 4], PRIORITIES[0].1);
        assert_eq!(f.prios(), [4, 7, 4, 7]);
        assert_eq!(f.cell(1, 2), "Mixed");

        f.click(ID_NONE);
        assert_eq!(f.prios(), [0, 0, 0, 0]);
        f.click(ID_ALL);
        assert_eq!(f.prios(), [4, 7, 4, 7], "priorities come back with the check marks");
    }

    #[test]
    fn a_folder_toggles_its_files() {
        let f = Fixture::new(&FILES, setup());
        f.select(&[1]);
        f.key(VK_SPACE);
        assert_eq!(f.prios(), [4, 0, 0, 4]);
        assert_eq!(f.st.tree.row(1).unwrap().check, Check::Off);
        // one file back: the folder is mixed, Space turns all of it on
        f.select(&[3]);
        f.key(VK_SPACE);
        assert_eq!(f.st.tree.row(1).unwrap().check, Check::Mixed);
        f.select(&[1]);
        f.key(VK_SPACE);
        assert_eq!(f.prios(), [4, 4, 4, 4]);

        // Left closes the folder, Right opens it
        f.key(VK_LEFT);
        assert_eq!(f.rows(), 4);
        assert_eq!(f.cell(2, 0), "cd2");
        f.key(VK_SPACE);
        assert_eq!(f.prios(), [4, 0, 0, 4], "a closed folder still toggles its files");
        f.key(VK_RIGHT);
        assert_eq!(f.rows(), 6);
        // Left on a file goes to its folder
        f.select(&[3]);
        f.key(VK_LEFT);
        unsafe { assert_eq!(controls::focused(f.lv()), Some(1)) };
    }

    #[test]
    fn a_torrent_in_the_session_cannot_skip_the_check() {
        let dir = std::env::temp_dir().join(format!("patorrent-win32-add-session-{}", std::process::id()));
        let save = dir.to_string_lossy().into_owned();
        let mut f =
            Fixture::new(&FILES, Setup { in_session: true, save_dirs: vec![save.clone()], ..setup() });
        unsafe {
            assert_eq!(
                windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(item(f.dlg, ID_SKIP)),
                0
            );
            set_check(f.dlg, ID_SKIP, true);
        }
        f.click(IDOK as u16);
        let c = f.st.choice.take().expect("OK gives a choice");
        assert!(!c.skip_hash_check);
        assert_eq!(c.save_path, save);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ok_hands_over_the_choice() {
        let dir = std::env::temp_dir().join(format!("patorrent-win32-add-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut f = Fixture::new(&FILES, setup());
        let save = dir.join("new folder");
        unsafe {
            set_text(item(f.dlg, ID_SAVE), &save.to_string_lossy());
            set_text(item(f.dlg, ID_NAME), "Renamed");
            set_text(item(f.dlg, ID_LABEL), " Linux ");
            set_check(f.dlg, ID_START, false);
            set_check(f.dlg, ID_SKIP, true);
            set_check(f.dlg, ID_TOP, true);
        }
        f.select(&[4]);
        f.key(VK_SPACE);
        f.click(IDOK as u16);
        let c = f.st.choice.take().expect("OK gives a choice");
        assert_eq!(c.save_path, save.to_string_lossy());
        assert!(save.is_dir(), "the save folder is created");
        assert_eq!(c.name.as_deref(), Some("Renamed"));
        assert_eq!(c.label, "Linux");
        assert!(!c.start && c.skip_hash_check && c.top_of_queue);
        assert_eq!(c.file_priorities.iter().map(|p| p.0).collect::<Vec<_>>(), [4, 4, 4, 0]);
        // the free space of the new folder's volume is shown
        assert!(f.text(ID_T_SIZE).contains("free"), "{}", f.text(ID_T_SIZE));

        // the unchanged name is not a rename; an empty save folder is refused
        unsafe { set_text(item(f.dlg, ID_NAME), "Pack") };
        let c = unsafe { collect(f.dlg, &f.st) }.unwrap();
        assert_eq!(c.name, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn thousands_of_files_fill_lazily() {
        let names: Vec<String> = (0..5000).map(|i| format!("d{}/f{i}.bin", i % 50)).collect();
        let files: Vec<(&str, u64)> = names.iter().map(|n| (n.as_str(), 1000)).collect();
        let start = std::time::Instant::now();
        let f = Fixture::new(&files, setup());
        assert_eq!(f.rows(), 5050);
        assert_eq!(f.cell(5049, 0), "f4999.bin");
        f.select(&[0]);
        f.key(VK_SPACE);
        assert_eq!(f.prios().iter().filter(|&&p| p == 0).count(), 100);
        assert!(start.elapsed().as_secs() < 10);
    }

    #[test]
    fn the_preference_decides_whether_it_is_shown() {
        // the switch lives in patorrent.ini as ui.show_add_dialog, on by default
        let mut prefs = Prefs::default();
        assert!(GuiOptions::load(&prefs, "x").show_add_dialog);
        let mut o = Options::load(&prefs, "x");
        o.gui.show_add_dialog = false;
        o.store(&mut prefs);
        assert!(prefs.to_text().contains("ui.show_add_dialog=0"));
        assert!(!GuiOptions::load(&prefs, "x").show_add_dialog);
    }
}
