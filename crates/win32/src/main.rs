//! PaTorrent for Windows: a native Win32 client in the spirit of µTorrent 2.2.1.
//!
//! Phase 3: the µTorrent 2.2.1 main window on the real engine. One instance runs per user;
//! starting another forwards its arguments (`.torrent` paths, `magnet:` links, URLs) to the
//! running one, as µTorrent does. `--minimized` starts in the tray (the autostart entry's
//! "Start minimized"). `--demo N` runs the simulated engine with N synthetic torrents instead
//! (scale testing). `--remote` drives a patorrent-nox elsewhere (`remote_ui`). `--associate` and
//! `--unregister` are the installer's (`setup`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod add;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod controls;
#[cfg(windows)]
mod create;
#[cfg(windows)]
mod details;
#[cfg(windows)]
mod dialogs;
#[cfg(windows)]
mod fit;
#[cfg(windows)]
mod icons;
#[cfg(windows)]
mod integration;
#[cfg(windows)]
mod notices;
#[cfg(windows)]
mod preferences;
#[cfg(windows)]
mod properties;
#[cfg(windows)]
mod remote_ui;
#[cfg(windows)]
mod rss;
#[cfg(windows)]
mod sched_grid;
#[cfg(windows)]
mod search;
#[cfg(windows)]
mod shutdown;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod util;
#[cfg(windows)]
mod webui_host;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(task) = setup::parse(&args) {
        std::process::exit(setup::run(task));
    }
    let parsed = app::parse_args(&args);
    // a remote window is not the single local instance, and takes no forwarded torrents
    if parsed.demo.is_none() && !parsed.remote && instance::forward(&parsed.sources) {
        return;
    }
    std::process::exit(app::run(parsed));
}

#[cfg(windows)]
mod instance {
    //! Single instance: a named mutex marks the first one; later ones send their arguments to
    //! its window with `WM_COPYDATA` and exit.

    use std::path::Path;
    use std::ptr::{null, null_mut};

    use patorrent_frontend::lang;
    use patorrent_frontend::prefs::Prefs;
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, SMTO_ABORTIFHUNG,
        SendMessageTimeoutW, WM_COPYDATA,
    };

    use crate::app::{COPYDATA_ARGS, MAIN_CLASS, data_dir};
    use crate::util::wide;

    /// Returns true if another instance took the arguments.
    pub fn forward(sources: &[String]) -> bool {
        // One instance per settings directory, so a portable copy can run beside an installed one.
        let key: String = data_dir()
            .to_string_lossy()
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        let name = wide(&format!(r"Local\PaTorrent-{key}"));
        unsafe {
            // Leaked on purpose: the mutex lives as long as the process.
            let m = CreateMutexW(null(), 0, name.as_ptr());
            if m.is_null() || GetLastError() != ERROR_ALREADY_EXISTS {
                return false;
            }
        }
        // Paths are relative to this process's directory; the other instance has its own.
        let mut payload: Vec<u16> = Vec::new();
        for s in sources {
            let abs = if s.contains("://") || s.starts_with("magnet:") {
                s.clone()
            } else {
                std::path::absolute(Path::new(s))
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| s.clone())
            };
            payload.extend(abs.encode_utf16());
            payload.push(0);
        }
        payload.push(0);
        for _ in 0..50 {
            let hwnd = unsafe { FindWindowW(MAIN_CLASS, null()) };
            if !hwnd.is_null() {
                // This process was started by the user's click and may take the foreground;
                // the running one may not, so hand it the right to raise its window.
                unsafe {
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(hwnd, &mut pid);
                    if pid != 0 {
                        AllowSetForegroundWindow(pid);
                    }
                }
                let cds = COPYDATASTRUCT {
                    dwData: COPYDATA_ARGS,
                    cbData: (payload.len() * 2) as u32,
                    lpData: payload.as_ptr() as *mut _,
                };
                let mut result = 0usize;
                let sent = unsafe {
                    SendMessageTimeoutW(
                        hwnd,
                        WM_COPYDATA,
                        0,
                        &cds as *const _ as isize,
                        SMTO_ABORTIFHUNG,
                        5000,
                        &mut result,
                    )
                };
                // the first instance runs as administrator: Windows keeps this one's message
                // from it (SEC-25); say so rather than drop the torrent silently
                if sent == 0 && unsafe { GetLastError() } == ERROR_ACCESS_DENIED && !sources.is_empty() {
                    let prefs = Prefs::load(data_dir().join("patorrent.ini"));
                    lang::init(prefs.get("ui.language").unwrap_or_default());
                    crate::dialogs::error(null_mut(), lang::text(lang::S::ForwardRefused));
                }
                return true;
            }
            // The first instance is still starting up.
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        true
    }
}

#[cfg(windows)]
mod setup {
    //! The installer's hidden flags (`packaging/windows/patorrent.nsi`): they change the
    //! registration as Preferences > General does and exit without a window. `--associate
    //! torrent,magnet` (either or both) makes this executable the program for `.torrent` files
    //! and `magnet:` links; `--unregister` removes what names this executable (associations,
    //! Default apps, autostart) before the uninstaller deletes it. The exit code is 0 when done,
    //! 1 when the registry refused, 2 for a bad argument.

    use patorrent_frontend::lang;
    use patorrent_frontend::prefs::Prefs;

    use crate::app::base_data_dir;
    use crate::integration::{self, Keys};
    use crate::util::exe_path;

    #[derive(Debug, PartialEq, Eq)]
    pub enum Task {
        Associate { torrent: bool, magnet: bool },
        Unregister,
    }

    /// The task when the first argument is one of the flags (`Err`: what is wrong with it).
    pub fn parse(args: &[String]) -> Option<Result<Task, String>> {
        let (flag, rest) = args.split_first()?;
        match (flag.as_str(), rest) {
            ("--associate", [list]) => Some(associate_list(list)),
            ("--unregister", []) => Some(Ok(Task::Unregister)),
            ("--associate" | "--unregister", _) => Some(Err(format!("{flag}: wrong arguments"))),
            _ => None,
        }
    }

    fn associate_list(list: &str) -> Result<Task, String> {
        let (mut torrent, mut magnet) = (false, false);
        for item in list.split(',') {
            match item.trim().to_ascii_lowercase().as_str() {
                "torrent" => torrent = true,
                "magnet" => magnet = true,
                _ => return Err(format!("--associate {list}: expected torrent, magnet or torrent,magnet")),
            }
        }
        Ok(Task::Associate { torrent, magnet })
    }

    /// Does `task`; the exit code.
    pub fn run(task: Result<Task, String>) -> i32 {
        let (keys, exe) = (Keys::user(), exe_path());
        let done = match task {
            Ok(Task::Associate { torrent, magnet }) => {
                // the type names it writes in the language chosen in Preferences, if any
                let prefs = Prefs::load(base_data_dir().join("patorrent.ini"));
                lang::init(prefs.get("ui.language").unwrap_or_default());
                integration::associate(&keys, &exe, torrent, magnet)
            }
            Ok(Task::Unregister) => integration::unregister(&keys, &exe),
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        };
        match done {
            Ok(_) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::setup::{Task, parse};

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn setup_flags() {
        let both = Task::Associate { torrent: true, magnet: true };
        assert_eq!(parse(&args(&["--associate", "torrent,magnet"])), Some(Ok(both)));
        assert_eq!(
            parse(&args(&["--associate", " Magnet "])),
            Some(Ok(Task::Associate { torrent: false, magnet: true }))
        );
        assert_eq!(parse(&args(&["--unregister"])), Some(Ok(Task::Unregister)));
        let bad: [&[&str]; 4] =
            [&["--associate"], &["--associate", "torrent,rss"], &["--associate", ""], &["--unregister", "x"]];
        for a in bad {
            assert!(matches!(parse(&args(a)), Some(Err(_))), "{a:?}");
        }
        // anything else is the window's, the flags only first
        let gui: [&[&str]; 4] = [&[], &["a.torrent"], &["--minimized"], &["a.torrent", "--unregister"]];
        for a in gui {
            assert_eq!(parse(&args(a)), None, "{a:?}");
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("patorrent is the Windows GUI; use patorrent-nox (or patorrent-gtk) on this platform.");
    std::process::exit(1);
}
