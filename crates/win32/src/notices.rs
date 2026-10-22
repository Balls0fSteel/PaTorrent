//! Help > Third-Party Notices: THIRD-PARTY-NOTICES.md, built into the program, so the licence
//! texts of the ported code stay with an executable copied out on its own (`patorrent-nox`
//! prints them with `--license`). A resizable dialog: the text read-only with a scroll bar, and
//! Close (Escape closes too).

use std::mem::zeroed;
use std::ptr::{null, null_mut};

use patorrent_frontend::{lang, tr};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::EM_SETSEL;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, DLGTEMPLATE, DialogBoxIndirectParamW, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY,
    EndDialog, GWL_STYLE, GetClientRect, GetDlgItem, GetWindowLongPtrW, GetWindowRect, IDCANCEL, MINMAXINFO,
    MapDialogRect, MoveWindow, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, WM_COMMAND, WM_GETMINMAXINFO, WM_INITDIALOG, WM_SIZE, WS_BORDER, WS_CHILD,
    WS_MAXIMIZEBOX, WS_TABSTOP, WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL,
};

use crate::dialogs::{BUTTON, DWLP_USER, EDIT, Template};
use crate::util::{loword, send, wide};

/// The notices as shipped beside the binaries.
pub const NOTICES: &str = include_str!("../../../THIRD-PARTY-NOTICES.md");

const ID_TEXT: u16 = 100;
/// Close is the dialog's Cancel, so Escape (and a multi-line edit's Escape) closes it.
const ID_CLOSE: u16 = IDCANCEL as u16;

/// Margins and the button, in dialog units.
const MARGIN: i32 = 7;
const BUTTON_W: i32 = 60;
const BUTTON_H: i32 = 14;
const WIDTH: i16 = 400;
const HEIGHT: i16 = 280;

/// The window title: the menu item without its mnemonic.
pub fn title() -> String {
    lang::plain(tr!(MenuNotices))
}

/// The notices as an edit control shows them (CR LF line ends).
pub fn text() -> String {
    NOTICES.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Shows the notices, modal to `owner`.
pub fn show(owner: HWND) {
    let template = template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(proc_),
            0,
        );
    }
}

fn template() -> Vec<u16> {
    let (w, h) = (WIDTH as i32, HEIGHT as i32);
    let mut t = Template::new(WIDTH, HEIGHT);
    let text = WS_CHILD
        | WS_VISIBLE
        | WS_BORDER
        | WS_VSCROLL
        | WS_TABSTOP
        | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32;
    let text_h = h - 3 * MARGIN - BUTTON_H;
    t.item(EDIT, ID_TEXT, text, [MARGIN, MARGIN, w - 2 * MARGIN, text_h].map(|v| v as i16), "");
    t.item(
        BUTTON,
        ID_CLOSE,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        [w - MARGIN - BUTTON_W, h - MARGIN - BUTTON_H, BUTTON_W, BUTTON_H].map(|v| v as i16),
        tr!(BtnClose),
    );
    t.finish()
}

/// Dialog units to pixels for this dialog's font.
unsafe fn pixels(dlg: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: x, bottom: y };
    unsafe { MapDialogRect(dlg, &mut r) };
    (r.right, r.bottom)
}

/// The text fills the client area above Close, which keeps to the bottom right corner.
unsafe fn layout(dlg: HWND) {
    unsafe {
        let mut c: RECT = zeroed();
        GetClientRect(dlg, &mut c);
        let (m, _) = pixels(dlg, MARGIN, 0);
        let (bw, bh) = pixels(dlg, BUTTON_W, BUTTON_H);
        let (cw, ch) = (c.right - c.left, c.bottom - c.top);
        let text = GetDlgItem(dlg, ID_TEXT as i32);
        MoveWindow(text, m, m, (cw - 2 * m).max(0), (ch - 3 * m - bh).max(0), 1);
        MoveWindow(GetDlgItem(dlg, ID_CLOSE as i32), cw - m - bw, ch - m - bh, bw, bh, 1);
    }
}

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowTextW(dlg, wide(&title()).as_ptr());
                let edit = GetDlgItem(dlg, ID_TEXT as i32);
                SetWindowTextW(edit, wide(&text()).as_ptr());
                // a sizing border; the size the template gives is the smallest
                let style = GetWindowLongPtrW(dlg, GWL_STYLE);
                SetWindowLongPtrW(dlg, GWL_STYLE, style | (WS_THICKFRAME | WS_MAXIMIZEBOX) as isize);
                SetWindowPos(
                    dlg,
                    null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
                );
                let mut r: RECT = zeroed();
                GetWindowRect(dlg, &mut r);
                // width and height in 16 bits each (`isize` is 32 bits on x86)
                let min = ((r.right - r.left) as u32 & 0xffff) | ((r.bottom - r.top) as u32 & 0xffff) << 16;
                SetWindowLongPtrW(dlg, DWLP_USER, min as isize);
                layout(dlg);
                // the caret at the top, nothing selected
                SetFocus(edit);
                send(edit, EM_SETSEL, 0, 0);
                0
            }
            WM_SIZE => {
                layout(dlg);
                1
            }
            WM_GETMINMAXINFO => {
                let min = GetWindowLongPtrW(dlg, DWLP_USER) as u32;
                if min != 0 {
                    let mmi = &mut *(lp as *mut MINMAXINFO);
                    mmi.ptMinTrackSize = POINT { x: (min & 0xffff) as i32, y: (min >> 16) as i32 };
                }
                1
            }
            WM_COMMAND if loword(wp) == ID_CLOSE => {
                EndDialog(dlg, IDCANCEL as isize);
                1
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    //! The dialog built from its template, hidden and modeless, driven through its messages.

    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateDialogIndirectParamW, DestroyWindow};

    use super::*;
    use crate::util::window_text;

    struct Dialog(HWND);

    impl Drop for Dialog {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) };
        }
    }

    fn open() -> Dialog {
        let template = template();
        let dlg = unsafe {
            CreateDialogIndirectParamW(
                GetModuleHandleW(null()),
                template.as_ptr() as *const DLGTEMPLATE,
                null_mut(),
                Some(proc_),
                0,
            )
        };
        assert!(!dlg.is_null(), "the template does not load");
        Dialog(dlg)
    }

    #[test]
    fn shows_the_notices_read_only_and_resizable() {
        let d = open();
        unsafe {
            assert_eq!(window_text(d.0), title());
            assert_eq!(title(), "Third-Party Notices");
            let edit = GetDlgItem(d.0, ID_TEXT as i32);
            let shown = window_text(edit);
            assert_eq!(shown, text());
            assert!(shown.starts_with("# Third-party notices\r\n") && shown.contains("Arvid Norberg"));
            assert!(!shown.replace("\r\n", "").contains('\n'), "bare LF");
            let edit_style = GetWindowLongPtrW(edit, GWL_STYLE) as u32;
            assert_ne!(edit_style & ES_READONLY as u32, 0);
            assert_ne!(edit_style & WS_VSCROLL, 0);
            assert_ne!(GetWindowLongPtrW(d.0, GWL_STYLE) as u32 & WS_THICKFRAME, 0);
            assert_eq!(window_text(GetDlgItem(d.0, ID_CLOSE as i32)), tr!(BtnClose));
        }
    }

    #[test]
    fn the_text_follows_the_window_size() {
        let d = open();
        unsafe {
            let size = |id: u16| {
                let mut r: RECT = zeroed();
                GetWindowRect(GetDlgItem(d.0, id as i32), &mut r);
                (r.right - r.left, r.bottom - r.top, r.left, r.top)
            };
            let mut w: RECT = zeroed();
            GetWindowRect(d.0, &mut w);
            let (tw, th, ..) = size(ID_TEXT);
            let (bw, bh, bx, by) = size(ID_CLOSE);
            MoveWindow(d.0, w.left, w.top, w.right - w.left + 200, w.bottom - w.top + 100, 0);
            let (tw2, th2, ..) = size(ID_TEXT);
            let (bw2, bh2, bx2, by2) = size(ID_CLOSE);
            assert_eq!((tw2 - tw, th2 - th), (200, 100), "the text grows with the window");
            assert_eq!((bw2, bh2, bx2 - bx, by2 - by), (bw, bh, 200, 100), "Close keeps to the corner");
            // never smaller than the template's size
            let mut mmi: MINMAXINFO = zeroed();
            send(d.0, WM_GETMINMAXINFO, 0, &mut mmi as *mut _ as LPARAM);
            assert_eq!((mmi.ptMinTrackSize.x, mmi.ptMinTrackSize.y), (w.right - w.left, w.bottom - w.top));
        }
    }
}
