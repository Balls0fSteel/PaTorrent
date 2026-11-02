//! The window shown while PaTorrent closes (D47). The engine stops on its own thread
//! ([`Session::begin_shutdown`](patorrent_core::Session::begin_shutdown)); when that takes more
//! than a second (a move to a network drive, a slow disk), or at once when Windows is signing
//! out, this window says what it is waiting for, lists the moves still to finish (however many:
//! the list scrolls) with their progress and destination, and for how long, so that PaTorrent
//! does not look hung. Quit Now first lists what quitting early gives up (the moves, the
//! trackers' "stopped", the downloaded data not written yet) and asks; it is not offered while
//! the torrent list and progress are being saved, which always finishes. The texts and the
//! rules are `patorrent_frontend::closing`'s, shared with the GTK GUI.

use std::ptr::null;
use std::time::{Duration, Instant};

use patorrent_core::{Exit, ExitStatus};
use patorrent_frontend::closing::{self, SHOW_AFTER};
use patorrent_frontend::tr;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows_sys::Win32::UI::Controls::{
    LVIF_TEXT, LVM_SETEXTENDEDLISTVIEWSTYLE, LVN_GETDISPINFOW, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_EX_LABELTIP, LVS_NOSORTHEADER, LVS_OWNERDATA, LVS_REPORT, LVS_SINGLESEL, NMHDR, NMLVDISPINFOW,
    PBM_SETMARQUEE, PBM_SETPOS, PBM_SETRANGE32, PBS_MARQUEE, PBS_SMOOTH,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_PUSHBUTTON, DLGTEMPLATE, DialogBoxIndirectParamW, EndDialog, GWL_STYLE, GetClientRect, GetDlgItem,
    GetSystemMetrics, GetWindowLongPtrW, ICON_BIG, ICON_SMALL, IsWindowVisible, KillTimer, SM_CXVSCROLL,
    SW_HIDE, SW_SHOW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_INITDIALOG, WM_NOTIFY, WM_SETICON, WM_TIMER, WS_BORDER, WS_CHILD, WS_DISABLED,
    WS_TABSTOP, WS_VISIBLE,
};

use crate::dialogs::{self, BUTTON, Class, DWLP_USER, STATIC, Template};
use crate::util::{copy_to, loword, send, wide};

/// `SS_NOPREFIX`: an & in a torrent name or folder is shown as it is.
const SS_NOPREFIX: u32 = 0x80;
const TIMER: usize = 1;
const ID_PHASE: u16 = 100;
const ID_LIST: u16 = 101;
const ID_PROGRESS: u16 = 102;
const ID_ELAPSED: u16 = 103;
const ID_QUIT: u16 = 104;

struct State {
    exit: Exit,
    start: Instant,
    /// Windows is signing out: the window holds it up with a reason.
    end_session: bool,
    /// What the window shows, to set only what changed.
    shown: Option<(ExitStatus, u64)>,
    marquee: Option<bool>,
    /// The list's rows: name, progress, destination.
    rows: Vec<[String; 3]>,
}

/// Waits for `exit` to end, behind the window when it takes long. Returns once the engine
/// thread has ended, or once Quit Now has got it past saving (the rest is left to end with
/// the process).
pub fn wait(exit: Exit, end_session: bool) {
    let start = Instant::now();
    if !end_session {
        while !exit.is_finished() && start.elapsed() < SHOW_AFTER {
            std::thread::sleep(Duration::from_millis(20));
        }
        if exit.is_finished() {
            return exit.wait();
        }
    }
    let mut st = State { exit, start, end_session, shown: None, marquee: None, rows: Vec::new() };
    let template = template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr() as *const DLGTEMPLATE,
            std::ptr::null_mut(),
            Some(proc_),
            &mut st as *mut State as LPARAM,
        );
    }
    if st.exit.is_finished() { st.exit.wait() } else { st.exit.abandon() }
}

fn template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    let list =
        WS_CHILD | WS_TABSTOP | WS_BORDER | LVS_REPORT | LVS_OWNERDATA | LVS_NOSORTHEADER | LVS_SINGLESEL;
    let mut t = Template::new(300, 160);
    t.item(STATIC, ID_PHASE, C | SS_NOPREFIX, [7, 7, 286, 26], "");
    t.item(Class::Name(WC_LISTVIEW), ID_LIST, list, [7, 36, 286, 82], "");
    t.item(Class::Name("msctls_progress32"), ID_PROGRESS, C | PBS_SMOOTH, [7, 123, 286, 10], "");
    t.item(STATIC, ID_ELAPSED, C, [7, 142, 200, 9], "");
    t.item(BUTTON, ID_QUIT, C | WS_TABSTOP | BS_PUSHBUTTON as u32, [223, 139, 70, 14], tr!(ExitQuitNow));
    t.finish()
}

/// The list view's class, by name for the template.
const WC_LISTVIEW: &str = "SysListView32";

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            let st = &mut *(lp as *mut State);
            SetWindowTextW(dlg, wide(tr!(ExitTitle)).as_ptr());
            send(dlg, WM_SETICON, ICON_BIG as usize, crate::icons::app_icon(32) as isize);
            send(dlg, WM_SETICON, ICON_SMALL as usize, crate::icons::app_icon(16) as isize);
            init_list(GetDlgItem(dlg, ID_LIST as i32));
            if st.end_session {
                ShutdownBlockReasonCreate(dlg, wide(tr!(EndSessionSaving)).as_ptr());
            }
            refresh(dlg, st);
            SetTimer(dlg, TIMER, closing::REFRESH.as_millis() as u32, None);
            SetForegroundWindow(dlg);
            return 1;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut State;
        if st.is_null() {
            return 0;
        }
        let st = &mut *st;
        match msg {
            WM_TIMER if wp == TIMER => {
                if closing::done(&st.exit, &st.exit.status()) {
                    KillTimer(dlg, TIMER);
                    EndDialog(dlg, 1);
                } else {
                    refresh(dlg, st);
                }
                1
            }
            WM_NOTIFY if (*(lp as *const NMHDR)).code == LVN_GETDISPINFOW => {
                let di = &mut *(lp as *mut NMLVDISPINFOW);
                if di.item.mask & LVIF_TEXT != 0 {
                    let cell =
                        st.rows.get(di.item.iItem as usize).and_then(|r| r.get(di.item.iSubItem as usize));
                    copy_to(di.item.pszText, di.item.cchTextMax, cell.map_or("", |c| c));
                }
                1
            }
            WM_COMMAND if loword(wp) == ID_QUIT => {
                quit_now(dlg, st);
                1
            }
            // the close box (and Esc) mean Quit Now, with its question
            WM_CLOSE => {
                quit_now(dlg, st);
                1
            }
            WM_DESTROY => {
                if st.end_session {
                    ShutdownBlockReasonDestroy(dlg);
                }
                0
            }
            _ => 0,
        }
    }
}

/// The list's columns, sized to its width: name, progress, destination.
unsafe fn init_list(lv: HWND) {
    unsafe {
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(lv, &mut rc);
        let w = rc.right - rc.left - GetSystemMetrics(SM_CXVSCROLL);
        let (name, done) = (w * 42 / 100, w * 24 / 100);
        let [c0, c1, c2] = closing::columns();
        crate::controls::set_columns(
            lv,
            &[(c0, name, false), (c1, done, false), (c2, w - name - done, false)],
        );
    }
}

/// Shows what the engine is doing now.
unsafe fn refresh(dlg: HWND, st: &mut State) {
    unsafe {
        let s = st.exit.status();
        let secs = st.start.elapsed().as_secs();
        if st.shown.as_ref().is_some_and(|(shown, t)| *shown == s && *t == secs) {
            return;
        }
        let set = |id: u16, text: &str| SetWindowTextW(GetDlgItem(dlg, id as i32), wide(text).as_ptr());
        set(ID_PHASE, &closing::phase_text(&s));
        set(ID_ELAPSED, &closing::elapsed_text(secs));

        // the moves, however many: the list scrolls
        let lv = GetDlgItem(dlg, ID_LIST as i32);
        let rows = closing::rows(&s);
        if rows != st.rows {
            st.rows = rows;
            crate::controls::set_count(lv, st.rows.len());
            InvalidateRect(lv, null(), 0);
        }
        let visible = !st.rows.is_empty();
        if (IsWindowVisible(lv) != 0) != visible {
            ShowWindow(lv, if visible { SW_SHOW } else { SW_HIDE });
        }

        // all the moves' progress together; a bar running back and forth for the rest
        let bar = GetDlgItem(dlg, ID_PROGRESS as i32);
        let fraction = closing::fraction(&s);
        let marquee = fraction.is_none();
        if st.marquee != Some(marquee) {
            let style = GetWindowLongPtrW(bar, GWL_STYLE) as u32;
            let style = if marquee { style | PBS_MARQUEE } else { style & !PBS_MARQUEE };
            SetWindowLongPtrW(bar, GWL_STYLE, style as isize);
            send(bar, PBM_SETMARQUEE, marquee as usize, 0);
            st.marquee = Some(marquee);
        }
        if let Some(f) = fraction {
            send(bar, PBM_SETRANGE32, 0, 1000);
            send(bar, PBM_SETPOS, (f * 1000.0) as usize, 0);
        }

        // nothing to give up while saving, nor once asked
        let quit = GetDlgItem(dlg, ID_QUIT as i32);
        let enabled = closing::quit_offered(&s);
        let style = GetWindowLongPtrW(quit, GWL_STYLE) as u32;
        if (style & WS_DISABLED == 0) != enabled {
            EnableWindow(quit, enabled as i32);
        }
        st.shown = Some((s, secs));
    }
}

/// Quit Now: says what it gives up and asks, then cuts the waits short (the window ends once
/// the engine is past saving).
unsafe fn quit_now(dlg: HWND, st: &mut State) {
    let s = st.exit.status();
    if !closing::quit_offered(&s) {
        return;
    }
    if let Some(q) = closing::question(&closing::consequences(&s)) {
        if !dialogs::confirm(dlg, &q) {
            return;
        }
    }
    st.exit.hurry();
    unsafe { refresh(dlg, st) };
}
