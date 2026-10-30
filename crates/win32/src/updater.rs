//! The GUI updating itself (D45): a PaTorrent that the setup installed (D31) downloads a newer
//! version's setup from GitHub, checks it against the release's `SHA256SUMS`
//! (`patorrent_frontend::update::download_setup`, behind a small progress dialog) and starts it
//! silently with `/UPDATE=<its own pid>`, then exits the normal way; the setup waits for it,
//! installs over the same folder and starts the new version. A portable copy (no Uninstall key
//! naming this folder) is sent to the release's page as before.

use std::io;
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use patorrent_core::fetch::FetchProxy;
use patorrent_frontend::tr;
use patorrent_frontend::update::{self, Version};
use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use windows_sys::Win32::UI::Controls::{PBM_SETMARQUEE, PBM_SETPOS, PBM_SETRANGE32, PBS_MARQUEE, PBS_SMOOTH};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_PUSHBUTTON, DLGTEMPLATE, DialogBoxIndirectParamW, EndDialog, GetDlgItem, GetWindowLongPtrW, IDCANCEL,
    PostMessageW, SetWindowLongPtrW, SetWindowTextW, WM_APP, WM_CLOSE, WM_COMMAND, WM_INITDIALOG, WS_CHILD,
    WS_TABSTOP, WS_VISIBLE,
};

use crate::dialogs::{self, BUTTON, Class, DWLP_USER, STATIC, Template};
use crate::util::{exe_path, loword, send, wide};

/// The setup's Add/Remove Programs key (`packaging/windows/patorrent.nsi`): under HKCU for one
/// user, HKLM for all users; its `InstallLocation` is the folder.
const UNINST_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\PaTorrent";

/// How the setup installed this PaTorrent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallMode {
    CurrentUser,
    AllUsers,
}

impl InstallMode {
    /// The setup's switch for the mode, so an upgrade installs over the same installation.
    pub fn switch(self) -> &'static str {
        match self {
            InstallMode::CurrentUser => "/CURRENTUSER",
            InstallMode::AllUsers => "/ALLUSERS",
        }
    }
}

/// Whether this executable is one the setup installed: the Uninstall key (one user's first,
/// then all users') whose `InstallLocation` is its folder.
pub fn installed() -> Option<InstallMode> {
    let exe = exe_path();
    let dir = exe.parent()?.to_string_lossy().into_owned();
    let roots: [(HKEY, InstallMode); 2] =
        [(HKEY_CURRENT_USER, InstallMode::CurrentUser), (HKEY_LOCAL_MACHINE, InstallMode::AllUsers)];
    roots.into_iter().find_map(|(root, mode)| {
        let location = crate::integration::get_in(root, UNINST_KEY, Some("InstallLocation"))?;
        same_folder(&location, &dir).then_some(mode)
    })
}

/// Whether two folder paths name the same folder, as Windows compares them: without regard to
/// case or a trailing separator.
pub fn same_folder(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    !a.trim().is_empty() && norm(a) == norm(b)
}

/// The setup's arguments for the update: silent, the installation's mode, and this process to
/// wait for.
pub fn setup_args(mode: InstallMode, pid: u32) -> [String; 3] {
    ["/S".to_owned(), mode.switch().to_owned(), format!("/UPDATE={pid}")]
}

/// Where the setup is downloaded to (`%TEMP%\PaTorrent-update`); what an earlier update left
/// there goes first.
pub fn download_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("PaTorrent-update");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }
    dir
}

/// Starts the setup for the update; it runs on while PaTorrent exits.
pub fn run_setup(path: &Path, mode: InstallMode) -> io::Result<()> {
    std::process::Command::new(path).args(setup_args(mode, std::process::id())).spawn().map(drop)
}

// ---- The download dialog

/// `wParam` = KiB so far, `lParam` = KiB in all (0 while unknown).
const WM_APP_PROGRESS: u32 = WM_APP + 22;
/// `lParam` = `Box<Result<PathBuf, String>>`.
const WM_APP_DONE: u32 = WM_APP + 23;
const ID_NAME: u16 = 100;
const ID_PROGRESS: u16 = 101;
const ID_PERCENT: u16 = 102;

struct State {
    version: Version,
    proxy: Option<FetchProxy>,
    cancel: Arc<AtomicBool>,
    /// Cancelled: end once the worker has stopped.
    closing: bool,
    result: Option<Result<PathBuf, String>>,
}

/// Downloads `v`'s setup behind a modal progress dialog: its path once checked, `None` when
/// cancelled or failed (a failure is shown first).
pub fn download(owner: HWND, v: &Version, proxy: Option<FetchProxy>) -> Option<PathBuf> {
    let mut st = State {
        version: v.clone(),
        proxy,
        cancel: Arc::new(AtomicBool::new(false)),
        closing: false,
        result: None,
    };
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
    match st.result? {
        Ok(path) => Some(path),
        Err(e) => {
            dialogs::error(owner, &tr!(UpdateDownloadFailed, version = v, error = e));
            None
        }
    }
}

fn template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    let mut t = Template::new(240, 58);
    t.item(STATIC, ID_NAME, C, [7, 7, 226, 9], "");
    t.item(Class::Name("msctls_progress32"), ID_PROGRESS, C | PBS_SMOOTH | PBS_MARQUEE, [7, 19, 226, 11], "");
    t.item(STATIC, ID_PERCENT, C, [7, 37, 120, 9], "");
    t.item(BUTTON, IDCANCEL as u16, C | WS_TABSTOP | BS_PUSHBUTTON as u32, [183, 35, 50, 14], tr!(BtnCancel));
    t.finish()
}

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            let st = &mut *(lp as *mut State);
            let title = wide(&tr!(UpdateDownloading, version = st.version));
            SetWindowTextW(dlg, title.as_ptr());
            let name = wide(&st.version.setup_name());
            SetWindowTextW(GetDlgItem(dlg, ID_NAME as i32), name.as_ptr());
            send(GetDlgItem(dlg, ID_PROGRESS as i32), PBM_SETMARQUEE, 1, 0);
            crate::fit::fit_texts(dlg, |_| Some(0));
            start(dlg, st);
            return 1;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut State;
        if st.is_null() {
            return 0;
        }
        let st = &mut *st;
        match msg {
            WM_COMMAND if loword(wp) == IDCANCEL as u16 => {
                cancel(dlg, st);
                1
            }
            WM_CLOSE => {
                cancel(dlg, st);
                1
            }
            WM_APP_PROGRESS => {
                let (done, total) = (wp as u32, lp as u32);
                let bar = GetDlgItem(dlg, ID_PROGRESS as i32);
                if total > 0 {
                    send(bar, PBM_SETMARQUEE, 0, 0);
                    send(bar, PBM_SETRANGE32, 0, total as isize);
                    send(bar, PBM_SETPOS, done as usize, 0);
                }
                let text = match total {
                    0 => format!("{} MB", done / 1024),
                    _ => format!(
                        "{:.1} % ({} / {} MB)",
                        done as f64 * 100.0 / total as f64,
                        done / 1024,
                        total / 1024
                    ),
                };
                let w = wide(&text);
                SetWindowTextW(GetDlgItem(dlg, ID_PERCENT as i32), w.as_ptr());
                1
            }
            WM_APP_DONE => {
                let result = *Box::from_raw(lp as *mut Result<PathBuf, String>);
                // a cancelled download is no failure to report
                st.result = if st.closing { None } else { Some(result) };
                EndDialog(dlg, if st.closing { IDCANCEL as isize } else { 1 });
                1
            }
            _ => 0,
        }
    }
}

/// Fetches on a worker thread; ends in `WM_APP_DONE`.
unsafe fn start(dlg: HWND, st: &mut State) {
    let (v, proxy, cancel) = (st.version.clone(), st.proxy.take(), st.cancel.clone());
    let target = dlg as isize;
    std::thread::spawn(move || {
        let dir = download_dir();
        let mut shown = u64::MAX;
        let result = update::download_setup(&v, proxy, &dir, &mut |got, total| {
            // about 500 updates at most
            let step = total.map_or(got >> 16, |t| got * 500 / t.max(1));
            if step != shown {
                shown = step;
                let kib = |n: u64| (n >> 10).min(u32::MAX as u64);
                unsafe {
                    PostMessageW(
                        target as HWND,
                        WM_APP_PROGRESS,
                        kib(got) as usize,
                        kib(total.unwrap_or(0)) as isize,
                    )
                };
            }
            !cancel.load(Ordering::Relaxed)
        });
        let boxed = Box::into_raw(Box::new(result));
        unsafe {
            if PostMessageW(target as HWND, WM_APP_DONE, 0, boxed as LPARAM) == 0 {
                drop(Box::from_raw(boxed));
            }
        }
    });
}

/// Cancel, Esc or the close box: the worker stops at its next piece, then the dialog ends.
fn cancel(_dlg: HWND, st: &mut State) {
    st.cancel.store(true, Ordering::Relaxed);
    st.closing = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_compare_as_windows_does() {
        assert!(same_folder(
            "C:\\Users\\x\\AppData\\Local\\Programs\\PaTorrent",
            "c:/users/X/appdata/local/programs/patorrent\\"
        ));
        assert!(same_folder("C:\\Program Files\\PaTorrent\\", "C:\\Program Files\\PaTorrent"));
        assert!(!same_folder("C:\\Program Files\\PaTorrent", "C:\\Program Files\\PaTorrent2"));
        assert!(!same_folder("", ""), "no InstallLocation is no installation");
    }

    #[test]
    fn the_setup_is_started_silently_for_the_same_installation() {
        assert_eq!(setup_args(InstallMode::CurrentUser, 4242), ["/S", "/CURRENTUSER", "/UPDATE=4242"]);
        assert_eq!(setup_args(InstallMode::AllUsers, 1)[1], "/ALLUSERS");
        // whatever this machine has: the lookup itself must not fail
        let _ = installed();
        assert!(download_dir().ends_with("PaTorrent-update"));
    }
}
