//! Help for settings and controls (D55) in the Win32 GUI: the help tooltips of a dialog (one
//! tooltip control per dialog, `TTF_SUBCLASS` tools, several lines, 0.7 s before and 20 s
//! shown), the tracking tip F1 shows beside the focused control, and the main window's menu
//! items and toolbar buttons described in the status bar while highlighted (`WM_MENUSELECT`,
//! the convention of the era). The texts are `frontend::lang`'s help table.

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use patorrent_frontend::lang::{self, H, S};
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    SB_SETTEXTW, SB_SIMPLE, SB_SIMPLEID, TOOLTIPS_CLASSW, TTDT_AUTOPOP, TTDT_INITIAL, TTF_ABSOLUTE,
    TTF_IDISHWND, TTF_SUBCLASS, TTF_TRACK, TTM_ADDTOOLW, TTM_NEWTOOLRECTW, TTM_SETDELAYTIME,
    TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE, TTM_TRACKPOSITION, TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP,
    TTS_NOPREFIX, TTTOOLINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetMenuStringW, GetWindowRect, MF_BYCOMMAND, MF_BYPOSITION, MF_POPUP, MF_SEPARATOR,
    MapDialogRect, WS_POPUP,
};

use crate::util::{send, wide};

/// The delay before a help tooltip shows, and how long it stays (D55).
const TIP_DELAY_MS: isize = 700;
const TIP_SHOW_MS: isize = 20_000;
/// How long the F1 tip stays, as the hover tips do.
pub const TIP_SHOW: std::time::Duration = std::time::Duration::from_millis(TIP_SHOW_MS as u64);
/// The widest a help tooltip gets before it wraps, in dialog units.
const TIP_WIDTH_DLU: i32 = 220;

/// The tracking tool's id in a dialog's F1 tip (no control has it).
const F1_TOOL: usize = 1;
/// Rectangle tools on the dialog itself, for greyed controls (which get no mouse messages, so
/// their own tool never fires): `RECT_TOOL + control id`.
const RECT_TOOL: usize = 0x1_0000;

/// A dialog's help tooltips and its F1 tip.
pub struct Tips {
    pub hover: HWND,
    pub f1: HWND,
}

impl Tips {
    /// Creates both for `dlg` (null handles if the tooltip class failed, which every call
    /// below then ignores).
    pub unsafe fn new(dlg: HWND) -> Tips {
        unsafe {
            let width = dlu(dlg, TIP_WIDTH_DLU) as LPARAM;
            let hover = tooltip(dlg);
            if !hover.is_null() {
                send(hover, TTM_SETMAXTIPWIDTH, 0, width);
                send(hover, TTM_SETDELAYTIME, TTDT_INITIAL as usize, TIP_DELAY_MS);
                send(hover, TTM_SETDELAYTIME, TTDT_AUTOPOP as usize, TIP_SHOW_MS);
            }
            let f1 = tooltip(dlg);
            if !f1.is_null() {
                send(f1, TTM_SETMAXTIPWIDTH, 0, width);
                // a tool added without a text is refused; F1 sets the real one
                let mut text = wide(" ");
                let mut ti = info(dlg, F1_TOOL, TTF_TRACK | TTF_ABSOLUTE, &mut text);
                send(f1, TTM_ADDTOOLW, 0, &mut ti as *mut _ as LPARAM);
            }
            Tips { hover, f1 }
        }
    }

    /// A help tooltip on `ctl`, child of `dlg`: on the control while it is enabled, and on its
    /// rectangle of the dialog while it is greyed (place it with [`Tips::place`]).
    pub unsafe fn add(&self, dlg: HWND, ctl: HWND, id: u16, text: &str) {
        if self.hover.is_null() || ctl.is_null() {
            return;
        }
        unsafe {
            // the tooltip copies the text
            let mut w = wide(text);
            let mut ti = info(dlg, ctl as usize, TTF_IDISHWND | TTF_SUBCLASS, &mut w);
            send(self.hover, TTM_ADDTOOLW, 0, &mut ti as *mut _ as LPARAM);
            let mut ti = info(dlg, RECT_TOOL + id as usize, TTF_SUBCLASS, &mut w);
            send(self.hover, TTM_ADDTOOLW, 0, &mut ti as *mut _ as LPARAM);
        }
    }

    /// Moves the rectangle tool of control `id` over `ctl` as it lies now, or to nothing when it
    /// is hidden (another Preferences page).
    pub unsafe fn place(&self, dlg: HWND, ctl: HWND, id: u16, shown: bool) {
        if self.hover.is_null() {
            return;
        }
        unsafe {
            let mut ti = info(dlg, RECT_TOOL + id as usize, 0, &mut []);
            if shown {
                let mut r: RECT = zeroed();
                GetWindowRect(ctl, &mut r);
                windows_sys::Win32::Graphics::Gdi::MapWindowPoints(
                    null_mut(),
                    dlg,
                    &mut r as *mut RECT as _,
                    2,
                );
                ti.rect = r;
            }
            send(self.hover, TTM_NEWTOOLRECTW, 0, &mut ti as *mut _ as LPARAM);
        }
    }

    /// F1: shows `text` under `ctl` until [`Tips::hide_f1`].
    pub unsafe fn show_f1(&self, dlg: HWND, ctl: HWND, text: &str) {
        if self.f1.is_null() {
            return;
        }
        unsafe {
            let mut w = wide(text);
            let mut ti = info(dlg, F1_TOOL, 0, &mut w);
            send(self.f1, TTM_UPDATETIPTEXTW, 0, &mut ti as *mut _ as LPARAM);
            let mut r: RECT = zeroed();
            GetWindowRect(ctl, &mut r);
            let (x, y) = (r.left as u16 as u32, (r.bottom + 2) as u16 as u32);
            send(self.f1, TTM_TRACKPOSITION, 0, (x | (y << 16)) as LPARAM);
            send(self.f1, TTM_TRACKACTIVATE, 1, &mut ti as *mut _ as LPARAM);
        }
    }

    pub unsafe fn hide_f1(&self, dlg: HWND) {
        if self.f1.is_null() {
            return;
        }
        unsafe {
            let mut ti = info(dlg, F1_TOOL, 0, &mut []);
            send(self.f1, TTM_TRACKACTIVATE, 0, &mut ti as *mut _ as LPARAM);
        }
    }
}

unsafe fn tooltip(dlg: HWND) -> HWND {
    unsafe {
        CreateWindowExW(
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
        )
    }
}

fn info(dlg: HWND, id: usize, flags: u32, text: &mut [u16]) -> TTTOOLINFOW {
    let mut ti: TTTOOLINFOW = unsafe { zeroed() };
    ti.cbSize = size_of::<TTTOOLINFOW>() as u32;
    ti.uFlags = flags;
    ti.hwnd = dlg;
    ti.uId = id;
    ti.lpszText = if text.is_empty() { null_mut() } else { text.as_mut_ptr() };
    ti
}

unsafe fn dlu(dlg: HWND, x: i32) -> i32 {
    let mut rc = RECT { left: 0, top: 0, right: x, bottom: 0 };
    unsafe { MapDialogRect(dlg, &mut rc) };
    rc.right
}

/// The help for a menu item or toolbar button by the text it shows (`&Add Torrent...\tCtrl+O`):
/// the help key named like the string key whose current text it is, among the menus', the
/// toolbar's and Auto Shutdown's. Matching the text rather than the command covers the items
/// that share a command (Open Containing Folder for one torrent or several, Hide / Show) and
/// the submenus, which have none.
pub fn for_menu_text(text: &str) -> Option<H> {
    let text = text.split('\t').next().unwrap_or(text);
    if text.is_empty() {
        return None;
    }
    H::ALL.iter().copied().find(|h| {
        let k = h.key();
        (k.starts_with("Menu") || k.starts_with("Tool") || k.starts_with("Shutdown"))
            && S::from_key(k).is_some_and(|s| lang::text(s) == text)
    })
}

/// `WM_MENUSELECT` on the main window: the highlighted item's help in the status bar (in its
/// simple mode, so the parts keep their texts), back to the parts when the menu closes.
pub unsafe fn menu_select(status: HWND, wparam: usize, lparam: LPARAM) {
    if status.is_null() {
        return;
    }
    let (item, flags) = (crate::util::loword(wparam) as u32, crate::util::hiword(wparam) as u32);
    let menu = lparam as windows_sys::Win32::UI::WindowsAndMessaging::HMENU;
    unsafe {
        if flags == 0xffff && menu.is_null() {
            send(status, SB_SIMPLE, 0, 0);
            return;
        }
        let mut text = String::new();
        if flags & MF_SEPARATOR == 0 && !menu.is_null() {
            let by = if flags & MF_POPUP != 0 { MF_BYPOSITION } else { MF_BYCOMMAND };
            let mut buf = [0u16; 260];
            let n = GetMenuStringW(menu, item, buf.as_mut_ptr(), buf.len() as i32, by);
            let shown = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            if let Some(h) = for_menu_text(&shown) {
                text = lang::help(h).to_owned();
            }
        }
        show_simple(status, &text);
    }
}

/// The toolbar's hot button changed (`TBN_HOTITEMCHANGE`): its help in the status bar, or the
/// parts back when the pointer leaves the toolbar (`button` `None`).
pub unsafe fn toolbar_hot(status: HWND, button: Option<S>) {
    if status.is_null() {
        return;
    }
    unsafe {
        match button.and_then(H::for_label) {
            Some(h) => show_simple(status, lang::help(h)),
            None => {
                send(status, SB_SIMPLE, 0, 0);
            }
        }
    }
}

unsafe fn show_simple(status: HWND, text: &str) {
    unsafe {
        send(status, SB_SIMPLE, 1, 0);
        let w = wide(text);
        send(status, SB_SETTEXTW, SB_SIMPLEID as usize, w.as_ptr() as LPARAM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_texts_find_their_help() {
        let add = format!("{}\tCtrl+O", lang::text(S::MenuAddTorrent));
        assert_eq!(for_menu_text(&add), Some(H::MenuAddTorrent));
        assert_eq!(for_menu_text(lang::text(S::MenuOpenFolderMany)), Some(H::MenuOpenFolderMany));
        assert_eq!(for_menu_text(lang::text(S::ShutdownQuitDownloads)), Some(H::ShutdownQuitDownloads));
        assert_eq!(for_menu_text(lang::text(S::MenuProfiles)), Some(H::MenuProfiles));
        // a label or profile name is not a menu text
        assert_eq!(for_menu_text("Linux ISOs"), None);
        assert_eq!(for_menu_text(""), None);
    }
}
