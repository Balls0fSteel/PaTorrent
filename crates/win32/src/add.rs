//! The Add New Torrent dialog (µTorrent 2.2.1): Save In with the recent folders, Name, the
//! torrent's contents with a file tree of check boxes and priorities, the options and Label.
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
use patorrent_frontend::add::{self, AddChoice, Check, FileTree};
use patorrent_frontend::detail::{PRIORITIES, priority_text};
use patorrent_frontend::fmt;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_WINDOW, CreateCompatibleBitmap, CreateCompatibleDC, DFC_BUTTON, DFCS_BUTTON3STATE,
    DFCS_BUTTONCHECK, DFCS_CHECKED, DeleteDC, DeleteObject, DrawFrameControl, FillRect, GetDC,
    GetSysColorBrush, InvalidateRect, ReleaseDC, SelectObject,
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
    LVIF_TEXT, LVIS_STATEIMAGEMASK, LVM_ENSUREVISIBLE, LVM_INSERTCOLUMNW, LVM_SETCALLBACKMASK,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST, LVM_SUBITEMHITTEST, LVN_GETDISPINFOW, LVN_KEYDOWN,
    LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT,
    LVS_SHAREIMAGELISTS, LVS_SHOWSELALWAYS, LVSIL_SMALL, LVSIL_STATE, NM_CLICK, NM_DBLCLK, NM_RETURN, NMHDR,
    NMITEMACTIVATE, NMLVDISPINFOW, NMLVKEYDOWN,
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
    CB_ADDSTRING, CB_GETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN, CBN_EDITCHANGE, CBN_SELCHANGE,
    CBS_AUTOHSCROLL, CBS_DROPDOWN, CBS_SORT, CreatePopupMenu, DLGTEMPLATE, DestroyMenu,
    DialogBoxIndirectParamW, ES_AUTOHSCROLL, EndDialog, GetCursorPos, GetDlgItem, GetWindowLongPtrW,
    GetWindowRect, IDCANCEL, IDOK, MF_SEPARATOR, MF_STRING, MapDialogRect, SetWindowLongPtrW, SetWindowTextW,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_INITDIALOG,
    WM_NOTIFY, WS_BORDER, WS_CHILD, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows_sys::core::w;

use crate::controls;
use crate::dialogs::{self, BUTTON, COMBOBOX, Class, DWLP_USER, EDIT, STATIC, Template};
use crate::util::{copy_to, from_wide, hiword, loword, point_from_lparam, scale, send, wide, window_text};

/// Static control styles (in `System::SystemServices`, a feature we do not otherwise need).
const SS_NOPREFIX: u32 = 0x80;
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

/// Context menu commands: the priorities, then check and uncheck.
const MENU_PRIO: usize = 1;
const MENU_CHECK: usize = 10;
const MENU_UNCHECK: usize = 11;

/// What the dialog starts with (the caller reads it from the preferences and the list).
#[derive(Clone, Debug, Default)]
pub struct Setup {
    /// The Save In list; the first is selected.
    pub save_dirs: Vec<String>,
    /// The labels already in use.
    pub labels: Vec<String>,
    /// "Start torrent" (off with Preferences' "Don't start the download automatically").
    pub start: bool,
    /// The torrent is in the session already (a magnet that fetched its metadata): its files
    /// were checked when the metadata arrived, so "Skip hash check" is greyed out.
    pub in_session: bool,
}

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
        }
    }
}

/// Shows the dialog for `ti`; `None` on Cancel.
pub fn show(owner: HWND, ti: &TorrentInfo, setup: Setup) -> Option<AddChoice> {
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
    st.choice
}

fn template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    const TAB: u32 = C | WS_TABSTOP;
    const LINE: u32 = TAB | WS_BORDER | ES_AUTOHSCROLL as u32;
    const PUSH: u32 = TAB | BS_PUSHBUTTON as u32;
    const CHECK: u32 = TAB | BS_AUTOCHECKBOX as u32;
    const GROUP: u32 = C | BS_GROUPBOX as u32;
    const COMBO: u32 = TAB | WS_VSCROLL | (CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32;
    const VALUE: u32 = C | SS_NOPREFIX | SS_ENDELLIPSIS;
    let list = TAB | WS_BORDER | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS;
    let mut t = Template::new(420, 258);
    t.item(BUTTON, 0xffff, GROUP, [7, 4, 176, 34], "Save In");
    t.item(COMBOBOX, ID_SAVE, COMBO, [14, 17, 140, 150], "");
    t.item(BUTTON, ID_BROWSE, PUSH, [158, 16, 18, 14], "...");
    t.item(BUTTON, 0xffff, GROUP, [7, 42, 176, 32], "&Name");
    t.item(EDIT, ID_NAME, LINE, [14, 55, 162, 13], "");
    t.item(BUTTON, 0xffff, GROUP, [7, 78, 176, 82], "Torrent Options");
    t.item(BUTTON, ID_SKIP, CHECK, [14, 91, 162, 10], "Skip &hash check");
    t.item(BUTTON, ID_START, CHECK, [14, 105, 162, 10], "&Start torrent");
    t.item(BUTTON, ID_TOP, CHECK, [14, 119, 162, 10], "Add to &top of queue");
    t.item(STATIC, 0xffff, C, [14, 138, 28, 9], "&Label:");
    t.item(COMBOBOX, ID_LABEL, COMBO | CBS_SORT as u32, [44, 136, 132, 120], "");
    t.item(BUTTON, 0xffff, GROUP, [190, 4, 223, 226], "Torrent Contents");
    t.item(STATIC, 0xffff, C, [197, 16, 36, 9], "Name:");
    t.item(STATIC, ID_T_NAME, VALUE, [236, 16, 170, 9], "");
    t.item(STATIC, 0xffff, C, [197, 27, 36, 9], "Comment:");
    t.item(STATIC, ID_T_COMMENT, VALUE, [236, 27, 170, 9], "");
    t.item(STATIC, 0xffff, C, [197, 38, 36, 9], "Size:");
    t.item(STATIC, ID_T_SIZE, VALUE, [236, 38, 170, 9], "");
    t.item(STATIC, 0xffff, C, [197, 49, 36, 9], "Date:");
    t.item(STATIC, ID_T_DATE, VALUE, [236, 49, 170, 9], "");
    t.item(Class::Name("SysListView32"), ID_FILES, list, [197, 62, 209, 144], "");
    t.item(BUTTON, ID_ALL, PUSH, [197, 210, 56, 14], "Select &All");
    t.item(BUTTON, ID_NONE, PUSH, [257, 210, 56, 14], "Select N&one");
    t.item(BUTTON, IDOK as u16, TAB | BS_DEFPUSHBUTTON as u32, [309, 238, 50, 14], "OK");
    t.item(BUTTON, IDCANCEL as u16, PUSH, [363, 238, 50, 14], "Cancel");
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
                if (*nm).idFrom == ID_FILES as usize {
                    notify(dlg, st, nm);
                }
                0
            }
            WM_CONTEXTMENU if wp as HWND == item(dlg, ID_FILES) => {
                context_menu(dlg, st, lp);
                1
            }
            WM_DESTROY => {
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
        set_text(dlg, "Add New Torrent");
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
        let label = item(dlg, ID_LABEL);
        for l in &st.setup.labels {
            let w = wide(l);
            send(label, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }

        set_text(item(dlg, ID_T_NAME), &st.name);
        set_text(item(dlg, ID_T_COMMENT), &st.comment);
        set_text(item(dlg, ID_T_DATE), &st.date.map(fmt::date_time).unwrap_or_default());

        let lv = item(dlg, ID_FILES);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        // columns in dialog units, like the rest of the layout (the list is 209 wide)
        let cols = [("Name", 108, false), ("Size", 42, true), ("Priority", 40, false)];
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
        update_size(dlg, st);
        SetFocus(item(dlg, IDOK as u16));
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
                if let Some(p) = crate::create::pick_folder(dlg, "Select the folder to save the torrent in") {
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
        let save_path = window_text(item(dlg, ID_SAVE)).trim().trim_matches('"').to_owned();
        if save_path.is_empty() {
            dialogs::error(dlg, "Please choose the folder to save the torrent in.");
            SetFocus(item(dlg, ID_SAVE));
            return None;
        }
        if let Err(e) = std::fs::create_dir_all(&save_path) {
            dialogs::error(dlg, &format!("{save_path}:\n{e}"));
            SetFocus(item(dlg, ID_SAVE));
            return None;
        }
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
            text.push_str(&format!(" (disk space: {} free)", fmt::size(free)));
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
            _ => row.priority.map_or("Mixed", priority_text).to_owned(),
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
            let w = wide(text);
            AppendMenuW(menu, MF_STRING, MENU_PRIO + i, w.as_ptr());
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, MENU_CHECK, w!("Check"));
        AppendMenuW(menu, MF_STRING, MENU_UNCHECK, w!("Uncheck"));
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, 0, dlg, null()) as usize;
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
    use patorrent_frontend::prefs::Prefs;
    use windows_sys::Win32::UI::Controls::{
        ICC_LISTVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, LVITEMW, LVM_GETITEMCOUNT,
        LVM_GETITEMTEXTW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CB_GETCOUNT, CreateDialogIndirectParamW, DestroyWindow, SendMessageW, WM_KEYDOWN,
    };

    use super::*;

    /// A multi-file torrent: `name/` with the given (path, size) files.
    pub(crate) fn torrent(name: &str, files: &[(&str, u64)]) -> Vec<u8> {
        let total: u64 = files.iter().map(|f| f.1).sum();
        let pieces = total.div_ceil(16384).max(1);
        let mut b = b"d7:comment9:a comment13:creation datei1700000000e4:infod5:filesl".to_vec();
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
            unsafe {
                let icc = INITCOMMONCONTROLSEX {
                    dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                    dwICC: ICC_LISTVIEW_CLASSES,
                };
                InitCommonControlsEx(&icc);
            }
            let ti = TorrentInfo::from_bytes(&torrent("Pack", files)).expect("test torrent");
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
        }
    }

    #[test]
    fn shows_the_torrent_and_its_files() {
        let f = Fixture::new(&FILES, setup());
        let dlg = f.dlg;
        unsafe {
            for id in [ID_SAVE, ID_BROWSE, ID_NAME, ID_SKIP, ID_START, ID_TOP, ID_LABEL, ID_ALL, ID_NONE] {
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
