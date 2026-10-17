//! PaTorrent for Windows: a native Win32 client in the spirit of µTorrent 2.2.1.
//!
//! Phase 3: the µTorrent 2.2.1 main window on the real engine. One instance runs per user;
//! starting another forwards its arguments (`.torrent` paths, `magnet:` links, URLs) to the
//! running one, as µTorrent does. `--demo N` runs the simulated engine with N synthetic torrents
//! instead (scale testing).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
mod icons;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod util;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = app::parse_args(&args);
    if parsed.demo.is_none() && instance::forward(&parsed.sources) {
        return;
    }
    std::process::exit(app::run(parsed));
}

#[cfg(windows)]
mod instance {
    //! Single instance: a named mutex marks the first one; later ones send their arguments to
    //! its window with `WM_COPYDATA` and exit.

    use std::path::Path;
    use std::ptr::null;

    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA,
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
                let cds = COPYDATASTRUCT {
                    dwData: COPYDATA_ARGS,
                    cbData: (payload.len() * 2) as u32,
                    lpData: payload.as_ptr() as *mut _,
                };
                let mut result = 0usize;
                unsafe {
                    SendMessageTimeoutW(
                        hwnd,
                        WM_COPYDATA,
                        0,
                        &cds as *const _ as isize,
                        SMTO_ABORTIFHUNG,
                        5000,
                        &mut result,
                    );
                }
                return true;
            }
            // The first instance is still starting up.
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        true
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("patorrent is the Windows GUI; use patorrent-nox (or patorrent-gtk) on this platform.");
    std::process::exit(1);
}
