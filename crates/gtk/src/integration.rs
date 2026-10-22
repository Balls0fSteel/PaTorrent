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
//! In a Flatpak ([`packaged`]) the associations are the package's: its exported desktop entry
//! declares both types, so the desktop offers PaTorrent for them, and the user picks the
//! default program in the desktop's own settings. Inside the sandbox GIO's
//! `set_as_default_for_type` would write the sandbox's own `mimeapps.list`, which the host never
//! reads. So there the associations read as unset and are never written, Preferences shows
//! their boxes greyed under a note, and "Check association on startup" never asks.
//!
//! Autostart works in a Flatpak too, through the Background portal (`crate::portal`):
//! `RequestBackground` with `autostart` and the sandbox's command line (`patorrent-gtk`, the
//! manifest's `command`, plus `--minimized` for "Start minimized"), which makes the portal write
//! the host's autostart entry (`flatpak run --command=patorrent-gtk <app id> ...`); turning it
//! off is the same request with `autostart` false. The portal answers later, after the desktop
//! has perhaps asked the user, and has nothing to read back, so what it granted is kept in
//! `background.ini` in the settings folder (`paths::background_file`) and read from there. The
//! dialog's Apply does not wait: the boxes show the granted state the next time it opens, and a
//! refusal or failure shows the usual error.
//!
//! macOS has none of this: an application registers for file types and URL schemes through
//! its `.app` bundle's `Info.plist` and starts at login through Login Items (PLAN.md D23),
//! neither built yet. There [`AVAILABLE`] is false, Preferences leaves the group out, nothing
//! reads as set and nothing can be written.

/// Whether this desktop has the integration (not macOS, see above).
pub const AVAILABLE: bool = cfg!(not(target_os = "macos"));

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

    /// What is set for this program now. In a Flatpak: no association (the desktop's settings
    /// hold them, out of the sandbox's sight) and autostart as the portal last granted it.
    pub fn read() -> Integration {
        if super::packaged() {
            return flatpak::read();
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
            return flatpak::write(&old, &want);
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

    /// Inside a Flatpak (see the module's note): autostart through the Background portal.
    mod flatpak {
        use gtk::gio;
        use gtk::prelude::*;
        use patorrent_frontend::integration::{Integration, MINIMIZED};
        use patorrent_frontend::prefs::Prefs;
        use patorrent_frontend::tr;

        /// The manifest's `command`: what the portal's autostart entry runs in the sandbox.
        const COMMAND: &str = "patorrent-gtk";

        fn state() -> Prefs {
            Prefs::load(crate::paths::background_file())
        }

        /// Autostart as the portal last granted it; never an association.
        pub fn read() -> Integration {
            let s = state();
            let autostart = s.get_bool("autostart", false);
            Integration {
                autostart,
                minimized: autostart && s.get_bool("minimized", false),
                ..Default::default()
            }
        }

        /// The portal's command line, with or without "Start minimized".
        pub fn commandline(minimized: bool) -> Vec<String> {
            let mut c = vec![COMMAND.to_owned()];
            if minimized {
                c.push(MINIMIZED.to_owned());
            }
            c
        }

        /// Asks the portal when autostart changes and returns at once with what is set now (the
        /// associations' boxes are greyed, so they never differ). The answer is kept and shown
        /// next time; a refusal or failure is reported when it comes.
        pub fn write(old: &Integration, want: &Integration) -> Result<Integration, String> {
            if (want.autostart, want.minimized) != (old.autostart, old.minimized) {
                let (autostart, minimized) = (want.autostart, want.minimized);
                let reason = tr!(PrefAutostart).replace('&', "");
                crate::portal::request_autostart(autostart, commandline(minimized), reason, move |r| {
                    let r = r.and_then(|granted| {
                        let mut s = state();
                        s.set_bool("autostart", granted);
                        s.set_bool("minimized", granted && minimized);
                        s.save()
                            .map_err(|e| format!("{}: {e}", crate::paths::background_file().display()))?;
                        if autostart && !granted {
                            return Err("the desktop does not allow PaTorrent to start at login".into());
                        }
                        Ok(())
                    });
                    if let Err(e) = r {
                        report(&e);
                    }
                });
            }
            Ok(read())
        }

        /// An error after the dialog has gone, over whichever window is in front.
        fn report(e: &str) {
            eprintln!("autostart through the Background portal: {e}");
            let window = gio::Application::default()
                .and_downcast::<gtk::Application>()
                .and_then(|a| a.active_window());
            gtk::AlertDialog::builder()
                .message(tr!(ErrDesktopIntegration, error = e))
                .modal(true)
                .build()
                .show(window.as_ref());
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn the_portal_runs_the_manifest_command() {
                assert_eq!(commandline(false), ["patorrent-gtk"]);
                assert_eq!(commandline(true), ["patorrent-gtk", "--minimized"]);
                let manifest =
                    include_str!("../../../packaging/flatpak/io.github.balls0fsteel.PaTorrent.yml");
                assert!(manifest.lines().any(|l| l == format!("command: {COMMAND}")));
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use patorrent_frontend::integration::Integration;

    /// Nothing is set (see the module's note on macOS).
    pub fn read() -> Integration {
        Integration::default()
    }

    pub fn write(_old: &Integration, _want: &Integration) -> Result<Integration, String> {
        Err("not available on macOS yet".into())
    }
}
