//! Rookparcel's GTK 4 GUI (PLAN.md Phase 7, D11): the µTorrent 2.2.1 main window on the real
//! engine for Linux and the BSDs, drawn from the same toolkit-independent `rookparcel_frontend`
//! models as the Win32 client. Windows has its own GUI (`rookparcel.exe`) and macOS too
//! (`rookparcel-mac`, Rookparcel.app, D52); there this crate builds a stub that says so.
//!
//! One instance runs per user (`GtkApplication`); starting another hands its arguments
//! (`.torrent` files, `magnet:` links, URLs) to the running one. Settings live in
//! `$XDG_CONFIG_HOME/rookparcel/gtk`, the torrent library in `$XDG_DATA_HOME/rookparcel/gtk`
//! (`paths`). `--remote` drives a rookparcel-nox elsewhere instead, in a process of its own
//! (`remote`).

#[cfg(all(unix, not(target_os = "macos")))]
mod add;
#[cfg(all(unix, not(target_os = "macos")))]
mod app;
#[cfg(all(unix, not(target_os = "macos")))]
mod canvas;
#[cfg(all(unix, not(target_os = "macos")))]
mod closing;
#[cfg(all(unix, not(target_os = "macos")))]
mod create;
#[cfg(all(unix, not(target_os = "macos")))]
mod detail;
#[cfg(all(unix, not(target_os = "macos")))]
mod icons;
#[cfg(all(unix, not(target_os = "macos")))]
mod integration;
#[cfg(all(unix, not(target_os = "macos")))]
mod list;
#[cfg(all(unix, not(target_os = "macos")))]
mod notices;
#[cfg(all(unix, not(target_os = "macos")))]
mod paths;
#[cfg(all(unix, not(target_os = "macos")))]
mod portal;
#[cfg(all(unix, not(target_os = "macos")))]
mod preferences;
#[cfg(all(unix, not(target_os = "macos")))]
mod properties;
#[cfg(all(unix, not(target_os = "macos")))]
mod remote;
#[cfg(all(unix, not(target_os = "macos")))]
mod rss;
#[cfg(all(unix, not(target_os = "macos")))]
mod sched_grid;
#[cfg(all(unix, not(target_os = "macos")))]
mod search;
#[cfg(all(unix, not(target_os = "macos")))]
mod setup_guide;
#[cfg(all(unix, not(target_os = "macos")))]
mod shutdown;
#[cfg(all(unix, not(target_os = "macos")))]
mod sidebar;
#[cfg(all(unix, not(target_os = "macos")))]
mod snapshots;
#[cfg(all(unix, not(target_os = "macos")))]
mod table;
#[cfg(all(unix, not(target_os = "macos")))]
mod tray;

#[cfg(all(unix, not(target_os = "macos")))]
fn main() -> std::process::ExitCode {
    // before GTK starts, so it needs no display
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("rookparcel-gtk {}", rookparcel_frontend::BUILD);
        return std::process::ExitCode::SUCCESS;
    }
    app::run()
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn main() {
    let other = if cfg!(target_os = "macos") {
        "on macOS use Rookparcel.app (rookparcel-mac)"
    } else {
        "on Windows use rookparcel.exe"
    };
    eprintln!("rookparcel-gtk is the GUI for Linux and the BSDs; {other}.");
    std::process::exit(1);
}
