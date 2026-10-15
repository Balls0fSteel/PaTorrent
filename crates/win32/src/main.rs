//! LibreTorrent for Windows: a native Win32 client in the spirit of µTorrent 2.2.1.
//!
//! Phase 0: a skeleton window wired to the engine API (simulated engine). It proves the
//! cross-thread alert path (engine thread → `PostMessageW` → GUI thread), the embedded manifest
//! (Common Controls v6, per-monitor DPI v2, long paths, UTF-8) and the static, single-file build.
//! The µTorrent 2.2.1 main window arrives in Phase 3.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "libretorrent is the Windows GUI; use libretorrent-nox (or libretorrent-gtk) on this platform."
    );
    std::process::exit(1);
}
