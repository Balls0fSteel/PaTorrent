//! Options > Auto Shutdown: the countdown dialog and the power actions.
//!
//! The decision lives in `frontend::auto_shutdown`; the main window runs it on its timer and
//! drives the countdown dialog from there ([`tick`], [`close`]), so the dialog only shows the
//! seconds and offers Cancel.
//!
//! The OS calls go through [`perform`]. Test builds record the call instead of making it, and
//! with the environment variable `PATORRENT_POWER_DRY_RUN` set the release build reports what
//! it would have done instead of doing it (for trying the whole path by hand).

use std::cell::{Cell, RefCell};
use std::ptr::{null, null_mut};

use patorrent_frontend::auto_shutdown::{Action, Mode, When};
use patorrent_frontend::tr;
use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Power::{IsPwrHibernateAllowed, IsPwrSuspendAllowed};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, DLGTEMPLATE, DialogBoxIndirectParamW, EndDialog, GetDlgItem, GetWindowLongPtrW,
    HWND_TOPMOST, IDCANCEL, SWP_NOMOVE, SWP_NOSIZE, SendMessageW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, WM_APP, WM_COMMAND, WM_DESTROY, WM_INITDIALOG, WS_CHILD, WS_TABSTOP,
    WS_VISIBLE,
};

use crate::dialogs::{BUTTON, DWLP_USER, STATIC, Template};
use crate::util::{loword, wide};

/// The OS action after a countdown (Quit is the app's own exit).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Power {
    Standby,
    Hibernate,
    ShutDown,
}

impl Power {
    pub fn of(action: Action) -> Option<Power> {
        match action {
            Action::Quit => None,
            Action::Standby => Some(Power::Standby),
            Action::Hibernate => Some(Power::Hibernate),
            Action::ShutDown => Some(Power::ShutDown),
        }
    }

    fn what(self) -> &'static str {
        match self {
            Power::Standby => tr!(PowerStandby),
            Power::Hibernate => tr!(PowerHibernate),
            Power::ShutDown => tr!(PowerShutDown),
        }
    }
}

/// Whether the computer can do this (Standby and Hibernate can be switched off or missing);
/// the menu greys the items it cannot.
pub fn available(action: Action) -> bool {
    unsafe {
        match action {
            Action::Standby => IsPwrSuspendAllowed(),
            Action::Hibernate => IsPwrHibernateAllowed(),
            Action::Quit | Action::ShutDown => true,
        }
    }
}

const DRY_RUN: &str = "PATORRENT_POWER_DRY_RUN";

/// Puts the computer on standby or into hibernation (returning after it wakes), or starts the
/// shutdown. The caller has saved everything first.
pub fn perform(p: Power) -> Result<(), String> {
    if std::env::var_os(DRY_RUN).is_some() {
        return Err(tr!(ShutdownDryRun, name = DRY_RUN, what = p.what()));
    }
    os::perform(p).map_err(|e| match p {
        Power::Standby => tr!(ShutdownErrStandby, error = e),
        Power::Hibernate => tr!(ShutdownErrHibernate, error = e),
        Power::ShutDown => tr!(ShutdownErrShutDown, error = e),
    })
}

#[cfg(not(test))]
mod os {
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_NOT_ALL_ASSIGNED, GetLastError};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Power::SetSuspendState;
    use windows_sys::Win32::System::Shutdown::{
        EWX_POWEROFF, EWX_SHUTDOWN, ExitWindowsEx, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_APPLICATION,
        SHTDN_REASON_MINOR_OTHER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use super::Power;

    pub fn perform(p: Power) -> Result<(), std::io::Error> {
        // both SetSuspendState and ExitWindowsEx need the shutdown privilege enabled
        enable_shutdown_privilege()?;
        let ok = unsafe {
            match p {
                Power::Standby => SetSuspendState(false, false, false),
                Power::Hibernate => SetSuspendState(true, false, false),
                // planned, so the event log does not ask why; no EWX_FORCE: programs with
                // unsaved work may still hold the shutdown back
                Power::ShutDown => {
                    ExitWindowsEx(
                        EWX_SHUTDOWN | EWX_POWEROFF,
                        SHTDN_REASON_MAJOR_APPLICATION | SHTDN_REASON_MINOR_OTHER | SHTDN_REASON_FLAG_PLANNED,
                    ) != 0
                }
            }
        };
        if ok { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    fn enable_shutdown_privilege() -> Result<(), std::io::Error> {
        unsafe {
            let mut token = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut tp: TOKEN_PRIVILEGES = zeroed();
            tp.PrivilegeCount = 1;
            tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
            let mut ok = LookupPrivilegeValueW(null(), SE_SHUTDOWN_NAME, &mut tp.Privileges[0].Luid) != 0;
            ok = ok && AdjustTokenPrivileges(token, 0, &tp, 0, null_mut(), null_mut()) != 0;
            // it succeeds without enabling anything when the account does not hold the privilege
            let e = GetLastError();
            CloseHandle(token);
            match (ok, e) {
                (true, ERROR_NOT_ALL_ASSIGNED) => Err(std::io::Error::from_raw_os_error(e as i32)),
                (true, _) => Ok(()),
                (false, _) => Err(std::io::Error::from_raw_os_error(e as i32)),
            }
        }
    }
}

#[cfg(test)]
mod os {
    //! Tests never touch the power state: the calls are recorded.

    use std::cell::RefCell;

    use super::Power;

    thread_local! {
        pub static CALLS: RefCell<Vec<Power>> = const { RefCell::new(Vec::new()) };
    }

    pub fn perform(p: Power) -> Result<(), std::io::Error> {
        CALLS.with(|c| c.borrow_mut().push(p));
        Ok(())
    }
}

thread_local! {
    /// The countdown dialog while it is open.
    static OPEN: Cell<HWND> = const { Cell::new(null_mut()) };
    /// Shut down once the app has exited (its state saved on the way out).
    static AFTER_EXIT: RefCell<Option<Power>> = const { RefCell::new(None) };
}

/// Asks for `p` after the main window is gone and the session has shut down.
pub fn after_exit(p: Power) {
    AFTER_EXIT.with(|a| *a.borrow_mut() = Some(p));
}

pub fn take_after_exit() -> Option<Power> {
    AFTER_EXIT.with(|a| a.borrow_mut().take())
}

/// How a countdown ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The user's Cancel (or Escape, or the close box).
    Cancelled,
    /// Time is up: do it.
    TimeUp,
    /// New work came: the countdown is off, the mode stays.
    Aborted,
}

/// `wParam`: the seconds left.
const WM_COUNTDOWN: u32 = WM_APP + 60;
/// `wParam`: 1 time up, 2 aborted.
const WM_COUNTDOWN_END: u32 = WM_APP + 61;
const ID_TEXT: u16 = 100;

struct State {
    mode: Mode,
    secs: u32,
    outcome: Outcome,
}

/// The countdown's text.
pub fn message(mode: Mode, secs: u32) -> String {
    let done = match mode.when {
        When::Downloads => tr!(ShutdownDownloadsDone),
        When::Everything => tr!(ShutdownEverythingDone),
    };
    let time = format!("{}:{:02}", secs / 60, secs % 60);
    let what = match mode.action {
        Action::Quit => tr!(ShutdownWillQuit, time),
        Action::Standby => tr!(ShutdownWillStandby, time),
        Action::Hibernate => tr!(ShutdownWillHibernate, time),
        Action::ShutDown => tr!(ShutdownWillShutDown, time),
    };
    format!("{done}\n{what}")
}

/// Shows the countdown (modal, on top of every window, the owner possibly hidden in the tray)
/// until Cancel, [`close`] or the window's end.
pub fn countdown(owner: HWND, mode: Mode, secs: u32) -> Outcome {
    let mut st = State { mode, secs, outcome: Outcome::Cancelled };
    let t = template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(proc_),
            &mut st as *mut State as LPARAM,
        );
    }
    st.outcome
}

pub fn is_open() -> bool {
    !OPEN.with(Cell::get).is_null()
}

/// The seconds left, if the countdown is showing.
pub fn tick(secs: u32) {
    let dlg = OPEN.with(Cell::get);
    if !dlg.is_null() {
        unsafe { SendMessageW(dlg, WM_COUNTDOWN, secs as usize, 0) };
    }
}

/// Ends the countdown from outside (time up, or aborted by new work).
pub fn close(outcome: Outcome) {
    let dlg = OPEN.with(Cell::get);
    let code = match outcome {
        Outcome::Cancelled => IDCANCEL as usize,
        Outcome::TimeUp => 1,
        Outcome::Aborted => 2,
    };
    if !dlg.is_null() {
        unsafe { SendMessageW(dlg, WM_COUNTDOWN_END, code, 0) };
    }
}

fn template() -> Vec<u16> {
    let mut t = Template::new(220, 58);
    t.item(STATIC, ID_TEXT, WS_CHILD | WS_VISIBLE, [10, 8, 200, 20], "");
    t.item(
        BUTTON,
        IDCANCEL as u16,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        [85, 36, 50, 14],
        tr!(BtnCancel),
    );
    t.finish()
}

unsafe fn show(dlg: HWND, st: &State) {
    let text = wide(&message(st.mode, st.secs));
    unsafe { SetWindowTextW(GetDlgItem(dlg, ID_TEXT as i32), text.as_ptr()) };
}

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut State;
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dlg, DWLP_USER, lp);
                let st = &*(lp as *const State);
                let title = wide(tr!(ShutdownTitle));
                SetWindowTextW(dlg, title.as_ptr());
                show(dlg, st);
                OPEN.with(|o| o.set(dlg));
                SetWindowPos(dlg, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                SetForegroundWindow(dlg);
                1
            }
            WM_COUNTDOWN if !st.is_null() => {
                (*st).secs = wp as u32;
                show(dlg, &*st);
                1
            }
            WM_COUNTDOWN_END if !st.is_null() => {
                (*st).outcome = if wp == 1 { Outcome::TimeUp } else { Outcome::Aborted };
                EndDialog(dlg, wp as isize);
                1
            }
            WM_COMMAND if loword(wp) as i32 == IDCANCEL && !st.is_null() => {
                (*st).outcome = Outcome::Cancelled;
                EndDialog(dlg, IDCANCEL as isize);
                1
            }
            WM_DESTROY => {
                OPEN.with(|o| {
                    if o.get() == dlg {
                        o.set(null_mut());
                    }
                });
                0
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

    struct Fixture {
        dlg: HWND,
        st: Box<State>,
    }

    impl Fixture {
        fn new(mode: Mode, secs: u32) -> Fixture {
            let mut st = Box::new(State { mode, secs, outcome: Outcome::Cancelled });
            let t = template();
            let dlg = unsafe {
                CreateDialogIndirectParamW(
                    GetModuleHandleW(null()),
                    t.as_ptr() as *const DLGTEMPLATE,
                    null_mut(),
                    Some(proc_),
                    &mut *st as *mut State as LPARAM,
                )
            };
            assert!(!dlg.is_null(), "the template does not load");
            Fixture { dlg, st }
        }

        fn text(&self) -> String {
            unsafe { window_text(GetDlgItem(self.dlg, ID_TEXT as i32)) }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.dlg) };
        }
    }

    const SHUT_DOWN: Mode = Mode::new(Action::ShutDown, When::Downloads);

    #[test]
    fn the_countdown_shows_the_seconds_and_cancel_ends_it() {
        let f = Fixture::new(SHUT_DOWN, 30);
        assert!(is_open());
        assert_eq!(unsafe { window_text(f.dlg) }, "Auto Shutdown");
        assert_eq!(f.text(), "All downloads have completed.\nThe computer will shut down in 0:30.");
        tick(1);
        assert_eq!(f.text(), "All downloads have completed.\nThe computer will shut down in 0:01.");
        unsafe { SendMessageW(f.dlg, WM_COMMAND, IDCANCEL as usize, 0) };
        assert_eq!(f.st.outcome, Outcome::Cancelled);
        drop(f);
        assert!(!is_open(), "closed with its window");
        tick(5); // nothing open: no effect
    }

    #[test]
    fn the_timer_ends_the_countdown_when_time_is_up_or_new_work_comes() {
        let f = Fixture::new(Mode::new(Action::Quit, When::Everything), 30);
        assert_eq!(f.text(), "All torrents have completed.\nPaTorrent will quit in 0:30.");
        close(Outcome::TimeUp);
        assert_eq!(f.st.outcome, Outcome::TimeUp);
        drop(f);

        let f = Fixture::new(Mode::new(Action::Hibernate, When::Downloads), 12);
        assert!(f.text().ends_with("The computer will hibernate in 0:12."));
        close(Outcome::Aborted);
        assert_eq!(f.st.outcome, Outcome::Aborted);
    }

    #[test]
    fn power_actions_are_recorded_not_made_in_tests() {
        assert_eq!(Power::of(Action::Quit), None);
        assert_eq!(Power::of(Action::Standby), Some(Power::Standby));
        for p in [Power::Standby, Power::Hibernate, Power::ShutDown] {
            perform(p).unwrap();
        }
        let calls = os::CALLS.with(|c| c.borrow().clone());
        assert_eq!(calls, [Power::Standby, Power::Hibernate, Power::ShutDown]);
        assert!(available(Action::Quit) && available(Action::ShutDown));

        assert_eq!(take_after_exit(), None);
        after_exit(Power::ShutDown);
        assert_eq!(take_after_exit(), Some(Power::ShutDown));
        assert_eq!(take_after_exit(), None);
    }
}
