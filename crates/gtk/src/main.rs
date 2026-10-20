//! PaTorrent's GTK 4 GUI (PLAN.md Phase 7, D11, D23): the µTorrent 2.2.1 main window on the real
//! engine for Linux, the BSDs and macOS, drawn from the same toolkit-independent
//! `patorrent_frontend` models as the Win32 client.
//!
//! One instance runs per user (`GtkApplication`); starting another hands its arguments
//! (`.torrent` files, `magnet:` links, URLs) to the running one. Settings live in
//! `$XDG_CONFIG_HOME/patorrent/gtk`, the torrent library in `$XDG_DATA_HOME/patorrent/gtk`
//! (`paths`).

#[cfg(unix)]
mod app;
#[cfg(unix)]
mod list;
#[cfg(unix)]
mod paths;
#[cfg(unix)]
mod preferences;
#[cfg(unix)]
mod sidebar;

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    app::run()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("patorrent-gtk is the GUI for Linux, the BSDs and macOS; on Windows use patorrent.exe.");
    std::process::exit(1);
}
