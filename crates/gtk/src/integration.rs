//! Desktop integration (Preferences > General) on freedesktop.org desktops: PaTorrent as the
//! program for `.torrent` files and `magnet:` links, and the autostart entry.
//!
//! What is written, all in the user's own folders (no root needed):
//! - the application's desktop entry, `$XDG_DATA_HOME/applications/<app id>.desktop` (its menu
//!   item; `MimeType` names both types), when an association is first asked for, and kept;
//! - the defaults for `application/x-bittorrent` and `x-scheme-handler/magnet`, through GIO,
//!   which keeps them in `$XDG_CONFIG_HOME/mimeapps.list` (what `xdg-mime default` writes);
//! - `$XDG_CONFIG_HOME/autostart/<app id>.desktop` for "Start PaTorrent on system startup",
//!   with `--minimized` for "Start minimized";
//! - the icon both entries name (`Icon=<app id>`), as
//!   `$XDG_DATA_HOME/icons/hicolor/<n>x<n>/apps/<app id>.png`, whenever an entry is written
//!   (`icons::install_theme_icons`), and kept, like the application's entry. A Flatpak exports
//!   its own entry and icon and writes nothing (see below); an AppImage's entries point at the
//!   AppImage and the icon is written as here.
//!
//! The state is read from those places each time, never stored in `patorrent.ini`: a box is
//! checked when the entry starts this very program ([`command`]), so a moved AppImage shows
//! unchecked until it registers again. Turning an association off resets the user's choices
//! for that type (GIO's `reset_type_associations`), which gives it back to the system's
//! default; it does not restore a program chosen before PaTorrent.
//!
//! A Flatpak manages all of it ([`packaged`]): its exported desktop entry declares both types,
//! so the desktop offers PaTorrent for them, and the user picks the default program and the
//! startup applications in the desktop's own settings. Inside the sandbox GIO's
//! `set_as_default_for_type` would write the sandbox's own `mimeapps.list`, which the host never
//! reads, and an autostart entry needs the Background portal (not used yet). So there nothing
//! reads as set, nothing is written, Preferences shows the group's boxes greyed under a note,
//! and "Check association on startup" never asks.
//!
//! macOS (`crate::macos`, PLAN.md D23) keeps the same boxes in its own places:
//! - "Start PaTorrent on system startup" is a LaunchAgent, `~/Library/LaunchAgents/<app id>.plist`
//!   with `RunAtLoad`, whose `ProgramArguments` are the bundle's executable (PaTorrent.app's
//!   launcher, which sets up GTK's paths; this executable outside a bundle), plus `--minimized`
//!   for "Start minimized". It is checked when the plist starts this very program, as above.
//!   macOS 13 and later list it in System Settings > General > Login Items ("Allow in the
//!   Background") and announce it in a notification when it is written.
//! - The defaults for `.torrent` files (the UTI LaunchServices has for the extension,
//!   `org.bittorrent.torrent`, which the bundle's `Info.plist` imports) and `magnet:` links are
//!   LaunchServices', set and read by bundle id. Only PaTorrent.app has one: run from a bare
//!   executable, the two association boxes and "Check association on startup" are greyed
//!   ([`associations_settable`]). macOS has no way to give a type back to "no default", so
//!   turning one off is refused with a pointer to Finder's Get Info > Open with.

/// Whether this desktop has the integration (every one the GUI runs on now; see above).
pub const AVAILABLE: bool = true;

/// Whether the `.torrent` and `magnet:` associations can be changed from here: not in a
/// Flatpak, and on macOS only from PaTorrent.app (LaunchServices names programs by bundle id).
pub fn associations_settable() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::macos::bundle_id().is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        !packaged()
    }
}

/// Whether the integration is the package's, not ours: running in a Flatpak (see above).
pub fn packaged() -> bool {
    std::env::var_os("FLATPAK_ID").is_some()
}

pub use imp::{read, write};

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::{Path, PathBuf};

    use gtk::prelude::*;
    use gtk::{gio, glib};
    use patorrent_frontend::integration::{
        self, EntryKind, Integration, MIME_MAGNET, MIME_TORRENT, MINIMIZED,
    };

    use crate::app::APP_ID;

    fn desktop_id() -> String {
        format!("{APP_ID}.desktop")
    }

    /// The command that starts this program: the AppImage file when run from one, else this
    /// executable (never asked in a Flatpak).
    pub fn command() -> Vec<String> {
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
        gio::AppInfo::default_for_type(content_type, false)
            .and_then(|a| a.id())
            .is_some_and(|i| i == id.as_str())
    }

    /// What is set for this program now: nothing in a Flatpak, where the desktop's settings
    /// hold it and the sandbox cannot see them.
    pub fn read() -> Integration {
        if super::packaged() {
            return Integration::default();
        }
        let command = command();
        let app = ours(&app_entry_path(), &command).is_some();
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

    /// The icon the entries name, in the user's hicolor theme.
    fn install_icon() -> Result<(), String> {
        crate::icons::install_theme_icons(&glib::user_data_dir().join("icons"), false)
    }

    /// Changes what differs between `old` (as [`read`] gave it) and `want`; what is set afterwards.
    pub fn write(old: &Integration, want: &Integration) -> Result<Integration, String> {
        let (old, want) = (old.normalized(), want.normalized());
        if super::packaged() {
            // the boxes are greyed there, so nothing asks; nothing would reach the host
            return Err("managed by the Flatpak: use the desktop's settings".into());
        }
        let command = command();
        if command.is_empty() {
            return Err("the path of this program is not known".into());
        }
        if (want.torrent && !old.torrent) || (want.magnet && !old.magnet) {
            // written again, so it starts this program (a moved AppImage, a new build)
            write_file(&app_entry_path(), &integration::entry(&command, APP_ID, EntryKind::App))?;
            install_icon()?;
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
                install_icon()?;
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
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;

    use gtk::glib;
    use patorrent_frontend::integration::{Integration, MINIMIZED};

    use crate::app::APP_ID;
    use crate::macos;

    /// What the LaunchAgent starts: the bundle's own executable inside PaTorrent.app (starting
    /// `patorrent-gtk` there directly would skip the launcher's paths), else this executable.
    pub fn command() -> Vec<String> {
        macos::bundle_executable()
            .or_else(|| std::env::current_exe().ok())
            .map(|p| vec![p.to_string_lossy().into_owned()])
            .unwrap_or_default()
    }

    fn agent_path() -> PathBuf {
        glib::home_dir().join("Library").join("LaunchAgents").join(format!("{APP_ID}.plist"))
    }

    /// The LaunchAgent's arguments after the program, when it starts this program.
    fn agent_args(command: &[String]) -> Option<Vec<String>> {
        let text = std::fs::read_to_string(agent_path()).ok()?;
        let args = macos::launch_agent_arguments(&text)?;
        let program = command.first()?;
        (args.first() == Some(program)).then(|| args[1..].to_vec())
    }

    /// LaunchServices may hand bundle ids back lowercased.
    fn is(handler: Option<String>, id: &str) -> bool {
        handler.is_some_and(|h| h.eq_ignore_ascii_case(id))
    }

    /// What is set for this program now (the associations only for PaTorrent.app).
    pub fn read() -> Integration {
        let args = agent_args(&command());
        let id = macos::bundle_id();
        let id = id.as_deref();
        Integration {
            torrent: id.is_some_and(|id| is(macos::default_for_type(&macos::torrent_uti()), id)),
            magnet: id.is_some_and(|id| is(macos::default_for_scheme(macos::MAGNET_SCHEME), id)),
            minimized: args.as_ref().is_some_and(|a| a.iter().any(|x| x == MINIMIZED)),
            autostart: args.is_some(),
        }
    }

    /// Changes what differs between `old` (as [`read`] gave it) and `want`; what is set afterwards.
    pub fn write(old: &Integration, want: &Integration) -> Result<Integration, String> {
        let (old, want) = (old.normalized(), want.normalized());
        if (want.torrent && !old.torrent) || (want.magnet && !old.magnet) {
            // the boxes are greyed outside the bundle; "Check association" does not ask there
            let id = macos::bundle_id().ok_or("only PaTorrent.app can be the default program")?;
            if want.torrent && !old.torrent {
                macos::set_default_for_type(&macos::torrent_uti(), &id)?;
            }
            if want.magnet && !old.magnet {
                macos::set_default_for_scheme(macos::MAGNET_SCHEME, &id)?;
            }
        }
        if (want.autostart, want.minimized) != (old.autostart, old.minimized) {
            let path = agent_path();
            if want.autostart {
                let mut program = command();
                if program.is_empty() {
                    return Err("the path of this program is not known".into());
                }
                if want.minimized {
                    program.push(MINIMIZED.to_owned());
                }
                let fail = |e: std::io::Error| format!("{}: {e}", path.display());
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(fail)?;
                }
                std::fs::write(&path, macos::launch_agent(APP_ID, &program)).map_err(fail)?;
            } else {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("{}: {e}", path.display())),
                }
            }
        }
        // LaunchServices keeps a default until another program takes it; there is no reset
        if (!want.torrent && old.torrent) || (!want.magnet && old.magnet) {
            return Err("macOS keeps PaTorrent as the default program until another one is chosen \
                 (Finder's Get Info > Open with > Change All for .torrent files, the other \
                 program's own setting for magnet: links)"
                .into());
        }
        Ok(read())
    }
}
