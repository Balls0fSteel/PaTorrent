//! Desktop integration (Preferences > General) on freedesktop.org desktops: PaTorrent as the
//! program for `.torrent` files and `magnet:` links, and the autostart entry.
//!
//! What is written, all in the user's own folders (no root needed):
//! - the application's desktop entry, `$XDG_DATA_HOME/applications/<app id>.desktop` (its menu
//!   item; `MimeType` names both types), when an association is first asked for, and kept;
//! - the defaults for `application/x-bittorrent` and `x-scheme-handler/magnet`, through GIO,
//!   which keeps them in `$XDG_CONFIG_HOME/mimeapps.list` (what `xdg-mime default` writes);
//! - `$XDG_CONFIG_HOME/autostart/<app id>.desktop` for "Start PaTorrent on system startup",
//!   with `--minimized` for "Start minimized".
//!
//! The state is read from those places each time, never stored in `patorrent.ini`: a box is
//! checked when the entry starts this very program ([`command`]), so a moved AppImage shows
//! unchecked until it registers again. Turning an association off resets the user's choices
//! for that type (GIO's `reset_type_associations`), which gives it back to the system's
//! default; it does not restore a program chosen before PaTorrent.

use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_frontend::integration::{self, EntryKind, Integration, MIME_MAGNET, MIME_TORRENT, MINIMIZED};

use crate::app::APP_ID;

fn desktop_id() -> String {
    format!("{APP_ID}.desktop")
}

/// In a Flatpak the application's entry is the one the Flatpak exports.
fn in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some()
}

/// The command that starts this program: `flatpak run <id>` in a Flatpak, the AppImage file
/// when run from one, else this executable.
pub fn command() -> Vec<String> {
    if let Some(id) = std::env::var_os("FLATPAK_ID") {
        return vec!["flatpak".into(), "run".into(), id.to_string_lossy().into_owned()];
    }
    if let Some(image) = std::env::var_os("APPIMAGE") {
        return vec![image.to_string_lossy().into_owned()];
    }
    std::env::current_exe().map(|p| vec![p.to_string_lossy().into_owned()]).unwrap_or_default()
}

fn app_entry_path() -> PathBuf {
    glib::user_data_dir().join("applications").join(desktop_id())
}

fn autostart_path() -> PathBuf {
    glib::user_config_dir().join("autostart").join(desktop_id())
}

/// The entry at `path` when it starts this program.
fn ours(path: &Path, command: &[String]) -> Option<String> {
    std::fs::read_to_string(path).ok().filter(|t| integration::starts(t, command))
}

fn default_is_ours(content_type: &str) -> bool {
    let id = desktop_id();
    gio::AppInfo::default_for_type(content_type, false).and_then(|a| a.id()).is_some_and(|i| i == id.as_str())
}

/// What is set for this program now.
pub fn read() -> Integration {
    let command = command();
    let app = in_flatpak() || ours(&app_entry_path(), &command).is_some();
    let autostart = ours(&autostart_path(), &command);
    Integration {
        torrent: app && default_is_ours(MIME_TORRENT),
        magnet: app && default_is_ours(MIME_MAGNET),
        minimized: autostart
            .as_deref()
            .and_then(integration::exec_args)
            .is_some_and(|a| a.iter().any(|x| x == MINIMIZED)),
        autostart: autostart.is_some(),
    }
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    std::fs::write(path, text).map_err(fail)
}

/// Changes what differs between `old` (as [`read`] gave it) and `want`; what is set afterwards.
pub fn write(old: &Integration, want: &Integration) -> Result<Integration, String> {
    let (old, want) = (old.normalized(), want.normalized());
    let command = command();
    if command.is_empty() {
        return Err("the path of this program is not known".into());
    }
    if (want.torrent && !old.torrent) || (want.magnet && !old.magnet) {
        if !in_flatpak() {
            // written again, so it starts this program (a moved AppImage, a new build)
            write_file(&app_entry_path(), &integration::entry(&command, APP_ID, EntryKind::App))?;
        }
        let id = desktop_id();
        let info = gio::DesktopAppInfo::new(&id)
            .or_else(|| gio::DesktopAppInfo::from_filename(app_entry_path()))
            .ok_or_else(|| format!("{}: not a desktop entry GIO can read", app_entry_path().display()))?;
        for (on, was, content_type) in
            [(want.torrent, old.torrent, MIME_TORRENT), (want.magnet, old.magnet, MIME_MAGNET)]
        {
            if on && !was {
                info.set_as_default_for_type(content_type).map_err(|e| e.to_string())?;
            }
        }
    }
    for (on, was, content_type) in
        [(want.torrent, old.torrent, MIME_TORRENT), (want.magnet, old.magnet, MIME_MAGNET)]
    {
        if !on && was {
            gio::AppInfo::reset_type_associations(content_type);
        }
    }
    if (want.autostart, want.minimized) != (old.autostart, old.minimized) {
        let path = autostart_path();
        if want.autostart {
            let kind = EntryKind::Autostart { minimized: want.minimized };
            write_file(&path, &integration::entry(&command, APP_ID, kind))?;
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
    }
    Ok(read())
}
