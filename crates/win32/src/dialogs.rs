//! Common dialogs and a small text-input dialog built from an in-memory template (no resource
//! compiler).

use std::mem::{size_of, zeroed};
use std::ptr::null;

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OPENFILENAMEW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, BS_PUSHBUTTON, DLGTEMPLATE, DS_CENTER, DS_MODALFRAME, DialogBoxIndirectParamW,
    ES_AUTOHSCROLL, EndDialog, GetDlgItem, GetWindowLongPtrW, IDCANCEL, IDOK, IDYES, MB_ICONERROR,
    MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNO, MessageBoxW, SendMessageW, SetWindowLongPtrW,
    SetWindowTextW, WM_COMMAND, WM_INITDIALOG, WS_BORDER, WS_CAPTION, WS_CHILD, WS_POPUP, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE,
};

use windows_sys::Win32::UI::Controls::EM_SETSEL;

use crate::util::{from_wide_ptr, loword, wide, window_text};

/// File → Add Torrent: one or more `.torrent` files.
pub fn open_torrents(owner: HWND) -> Vec<String> {
    let filter = wide("Torrents (*.torrent)\0*.torrent\0All Files (*.*)\0*.*\0");
    let title = wide("Select a .torrent to open");
    let mut buf = vec![0u16; 64 * 1024];
    unsafe {
        let mut ofn: OPENFILENAMEW = zeroed();
        ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrTitle = title.as_ptr();
        ofn.Flags = OFN_EXPLORER | OFN_ALLOWMULTISELECT | OFN_FILEMUSTEXIST | OFN_HIDEREADONLY;
        if GetOpenFileNameW(&mut ofn) == 0 {
            return Vec::new();
        }
    }
    // One file: its full path. Several: the folder, then each name, NUL-separated.
    let parts: Vec<String> =
        buf.split(|&c| c == 0).take_while(|s| !s.is_empty()).map(String::from_utf16_lossy).collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [one] => vec![one.clone()],
        [dir, names @ ..] => names.iter().map(|n| format!("{}\\{n}", dir.trim_end_matches('\\'))).collect(),
    }
}

pub fn error(owner: HWND, text: &str) {
    message(owner, text, MB_OK | MB_ICONERROR);
}

pub fn info(owner: HWND, text: &str) {
    message(owner, text, MB_OK | MB_ICONINFORMATION);
}

pub fn confirm(owner: HWND, text: &str) -> bool {
    message(owner, text, MB_YESNO | MB_ICONWARNING) == IDYES
}

fn message(owner: HWND, text: &str, flags: u32) -> i32 {
    let (t, c) = (wide(text), wide("LibreTorrent"));
    unsafe { MessageBoxW(owner, t.as_ptr(), c.as_ptr(), flags) }
}

/// The clipboard's text, if any.
pub fn clipboard_text(owner: HWND) -> Option<String> {
    unsafe {
        if OpenClipboard(owner) == 0 {
            return None;
        }
        let h = GetClipboardData(CF_UNICODETEXT as u32);
        let text = if h.is_null() {
            None
        } else {
            let p = GlobalLock(h) as *const u16;
            let s = from_wide_ptr(p);
            GlobalUnlock(h);
            Some(s)
        };
        CloseClipboard();
        text
    }
}

/// `DS_SETFONT | DS_FIXEDSYS`: the dialog uses the shell font (Segoe UI).
const DS_SHELLFONT: u32 = 0x48;
/// `GetWindowLongPtr` index of the dialog's user data (after the message result and the
/// dialog procedure).
const DWLP_USER: i32 = (2 * size_of::<usize>()) as i32;

const ID_EDIT: u16 = 100;
const ID_PROMPT: u16 = 101;

struct InputState {
    title: String,
    prompt: String,
    initial: String,
    result: Option<String>,
}

/// A modal dialog with a prompt, one text field, OK and Cancel. Returns the trimmed text, or
/// `None` on Cancel.
pub fn input(owner: HWND, title: &str, prompt: &str, initial: &str) -> Option<String> {
    let mut state =
        InputState { title: title.into(), prompt: prompt.into(), initial: initial.into(), result: None };
    let template = input_template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(input_proc),
            &mut state as *mut InputState as LPARAM,
        );
    }
    state.result.map(|s| s.trim().to_owned())
}

unsafe extern "system" fn input_proc(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dlg, DWLP_USER, lp);
                let st = &*(lp as *const InputState);
                let (t, p, i) = (wide(&st.title), wide(&st.prompt), wide(&st.initial));
                SetWindowTextW(dlg, t.as_ptr());
                SetWindowTextW(GetDlgItem(dlg, ID_PROMPT as i32), p.as_ptr());
                let edit = GetDlgItem(dlg, ID_EDIT as i32);
                SetWindowTextW(edit, i.as_ptr());
                SendMessageW(edit, EM_SETSEL, 0, -1);
                1
            }
            WM_COMMAND => {
                let id = loword(wp) as i32;
                if id == IDOK || id == IDCANCEL {
                    let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut InputState;
                    if id == IDOK && !st.is_null() {
                        (*st).result = Some(window_text(GetDlgItem(dlg, ID_EDIT as i32)));
                    }
                    EndDialog(dlg, id as isize);
                    return 1;
                }
                0
            }
            _ => 0,
        }
    }
}

/// `DLGTEMPLATE` + items, as the dialog manager reads them (UTF-16 words, items DWORD-aligned).
fn input_template() -> Vec<u16> {
    let mut t: Vec<u16> = Vec::new();
    fn dword(t: &mut Vec<u16>, v: u32) {
        t.push(v as u16);
        t.push((v >> 16) as u16);
    }
    fn text(t: &mut Vec<u16>, s: &str) {
        t.extend(s.encode_utf16());
        t.push(0);
    }
    fn align(t: &mut Vec<u16>) {
        if t.len() % 2 == 1 {
            t.push(0);
        }
    }
    let style = WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME as u32 | DS_CENTER as u32 | DS_SHELLFONT;
    dword(&mut t, style);
    dword(&mut t, 0);
    t.push(4); // items
    t.extend([0, 0, 280, 62].map(|v: i16| v as u16)); // x, y, cx, cy in dialog units
    t.push(0); // no menu
    t.push(0); // default class
    text(&mut t, "");
    t.push(8);
    text(&mut t, "MS Shell Dlg");

    // (class atom, id, style, x, y, cx, cy, text)
    let items: [(u16, u16, u32, [i16; 4], &str); 4] = [
        (0x0082, ID_PROMPT, WS_CHILD | WS_VISIBLE, [7, 7, 266, 9], ""),
        (
            0x0081,
            ID_EDIT,
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            [7, 19, 266, 13],
            "",
        ),
        (
            0x0080,
            IDOK as u16,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            [169, 40, 50, 14],
            "OK",
        ),
        (
            0x0080,
            IDCANCEL as u16,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            [223, 40, 50, 14],
            "Cancel",
        ),
    ];
    for (class, id, style, rect, label) in items {
        align(&mut t);
        dword(&mut t, style);
        dword(&mut t, 0);
        t.extend(rect.map(|v| v as u16));
        t.push(id);
        t.push(0xffff);
        t.push(class);
        text(&mut t, label);
        t.push(0); // no creation data
    }
    t
}

/// Reads a NUL-separated list (WM_COPYDATA payloads) into strings.
pub fn split_nul(buf: &[u16]) -> Vec<String> {
    buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}
