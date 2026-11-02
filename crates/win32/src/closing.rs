//! The window shown while PaTorrent closes (D47). The engine stops on its own thread
//! ([`Session::begin_shutdown`](patorrent_core::Session::begin_shutdown)); when that takes more
//! than a second (a move to a network drive, a slow disk), or at once when Windows is signing
//! out, this window says what it is waiting for, lists the moves still to finish (however many:
//! the list scrolls) with their progress and destination, and for how long, so that PaTorrent
//! does not look hung. Quit Now first lists what quitting early gives up (the moves, the
//! trackers' "stopped", the downloaded data not written yet) and asks; it is not offered while
//! the torrent list and progress are being saved, which always finishes.

use std::ptr::null;
use std::time::{Duration, Instant};

use patorrent_core::{Exit, ExitPhase, ExitStatus};
use patorrent_frontend::{fmt, tr};
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
/// How long a shutdown may take before the window shows.
const SHOW_AFTER: Duration = Duration::from_secs(1);
/// The question names this many moves; more are counted in one line.
const ASK_MOVES: usize = 2;
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
            SetTimer(dlg, TIMER, 200, None);
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
                let s = st.exit.status();
                if st.exit.is_finished() || (s.hurried && s.phase >= ExitPhase::Closing) {
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
        crate::controls::set_columns(
            lv,
            &[
                (tr!(ColName), name, false),
                (tr!(ColDone), done, false),
                (tr!(ExitColMoveTo), w - name - done, false),
            ],
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
        set(ID_PHASE, &phase_text(&s));
        set(ID_ELAPSED, &tr!(ExitElapsed, time = format!("{}:{:02}", secs / 60, secs % 60)));

        // the moves, however many: the list scrolls
        let lv = GetDlgItem(dlg, ID_LIST as i32);
        let rows = rows(&s);
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
        let (done, total) = s.moves.iter().fold((0u64, 0u64), |(d, t), m| (d + m.done, t + m.total));
        let marquee = !(s.phase == ExitPhase::Moving && total > 0);
        if st.marquee != Some(marquee) {
            let style = GetWindowLongPtrW(bar, GWL_STYLE) as u32;
            let style = if marquee { style | PBS_MARQUEE } else { style & !PBS_MARQUEE };
            SetWindowLongPtrW(bar, GWL_STYLE, style as isize);
            send(bar, PBM_SETMARQUEE, marquee as usize, 0);
            st.marquee = Some(marquee);
        }
        if !marquee {
            send(bar, PBM_SETRANGE32, 0, 1000);
            send(bar, PBM_SETPOS, (done * 1000 / total.max(1)) as usize, 0);
        }

        // nothing to give up while saving, nor once asked
        let quit = GetDlgItem(dlg, ID_QUIT as i32);
        let enabled = !s.hurried && s.phase != ExitPhase::Saving;
        let style = GetWindowLongPtrW(quit, GWL_STYLE) as u32;
        if (style & WS_DISABLED == 0) != enabled {
            EnableWindow(quit, enabled as i32);
        }
        st.shown = Some((s, secs));
    }
}

/// The phase's sentence.
fn phase_text(s: &ExitStatus) -> String {
    match s.phase {
        ExitPhase::Stopping => tr!(ExitStopping).to_owned(),
        ExitPhase::Moving => tr!(ExitMoving).to_owned(),
        ExitPhase::Saving => tr!(ExitSaving).to_owned(),
        ExitPhase::Trackers => tr!(ExitTrackers).to_owned(),
        ExitPhase::Closing => tr!(ExitWriting, size = fmt::size(s.unwritten)),
    }
}

/// One row per move still to finish: name, progress (or waiting), destination.
fn rows(s: &ExitStatus) -> Vec<[String; 3]> {
    s.moves
        .iter()
        .map(|m| {
            let done = if m.started {
                tr!(ExitMoveProgress, done = fmt::size(m.done), total = fmt::size(m.total))
            } else {
                tr!(ExitMoveWaiting).to_owned()
            };
            [m.name.clone(), done, m.to.clone()]
        })
        .collect()
}

/// What Quit Now gives up now, one "• " line each; empty when nothing. Past [`ASK_MOVES`]
/// moves the rest are counted, so the question stays readable with dozens.
fn consequences(s: &ExitStatus) -> Vec<String> {
    let named = if s.moves.len() > ASK_MOVES + 1 { ASK_MOVES } else { s.moves.len() };
    let mut out: Vec<String> = s.moves[..named]
        .iter()
        .map(|m| tr!(ExitAbortMove, name = m.name, folder = m.from, dest = m.to))
        .collect();
    if named < s.moves.len() {
        out.push(tr!(ExitAbortMoreMoves, count = s.moves.len() - named));
    }
    if s.phase <= ExitPhase::Trackers {
        out.push(tr!(ExitAbortTrackers).to_owned());
    }
    if s.unwritten > 0 {
        out.push(tr!(ExitAbortWrites, size = fmt::size(s.unwritten)));
    }
    out
}

/// Quit Now: says what it gives up and asks, then cuts the waits short (the window ends once
/// the engine is past saving).
unsafe fn quit_now(dlg: HWND, st: &mut State) {
    let s = st.exit.status();
    if s.hurried || s.phase == ExitPhase::Saving {
        return;
    }
    let lines = consequences(&s);
    if !lines.is_empty() && !dialogs::confirm(dlg, &tr!(ExitAskQuit, list = lines.join("\n"))) {
        return;
    }
    st.exit.hurry();
    unsafe { refresh(dlg, st) };
}

#[cfg(test)]
mod tests {
    use patorrent_core::ExitMove;

    use super::*;

    fn moving(n: usize) -> ExitStatus {
        let moves = (0..n)
            .map(|i| ExitMove {
                name: format!("Debian {i}"),
                from: r"C:\Downloads".into(),
                to: r"\\nas\done".into(),
                done: 3 << 30,
                total: 4 << 30,
                started: i == 0,
            })
            .collect();
        ExitStatus { phase: ExitPhase::Moving, moves, unwritten: 1 << 20, hurried: false }
    }

    #[test]
    fn the_window_lists_every_move() {
        let s = moving(20);
        assert_eq!(phase_text(&s), tr!(ExitMoving));
        let rows = rows(&s);
        assert_eq!(rows.len(), 20);
        assert_eq!(
            rows[0],
            ["Debian 0".to_owned(), "3.00 GB of 4.00 GB".to_owned(), r"\\nas\done".to_owned()]
        );
        assert_eq!(rows[19][1], tr!(ExitMoveWaiting), "not started yet");
        let closing = ExitStatus { phase: ExitPhase::Closing, moves: Vec::new(), ..moving(0) };
        assert!(phase_text(&closing).contains("1.00 MB"), "{}", phase_text(&closing));
    }

    #[test]
    fn quit_now_lists_what_it_gives_up() {
        let all = consequences(&moving(1));
        assert_eq!(all.len(), 3, "{all:?}");
        assert!(
            all[0].contains("Debian 0") && all[0].contains(r"C:\Downloads") && all[0].contains(r"\\nas\done")
        );
        assert_eq!(all[1], tr!(ExitAbortTrackers));
        assert!(all[2].contains("1.00 MB"));
        // three moves are named; past that two are and the rest counted
        assert_eq!(consequences(&moving(3)).len(), 5);
        let many = consequences(&moving(20));
        assert_eq!(many.len(), 5, "{many:?}");
        assert!(many[1].contains("Debian 1"));
        assert_eq!(many[2], tr!(ExitAbortMoreMoves, count = 18));
        // writing the last data: only that; nothing left: no question at all
        let closing = ExitStatus { phase: ExitPhase::Closing, moves: Vec::new(), ..moving(0) };
        assert_eq!(consequences(&closing).len(), 1);
        assert!(consequences(&ExitStatus { phase: ExitPhase::Closing, ..Default::default() }).is_empty());
    }
}
