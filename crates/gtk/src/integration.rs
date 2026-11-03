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
//! Profiles (D42): a named profile has a desktop entry of its own, `<app id>.profile_<name>.desktop`
//! (its application id's, so a Wayland desktop finds the profile's windows' icon), hidden from
//! the menu (`NoDisplay=true`) and written at its first start ([`profile_entry`]), in the host's
//! `applications/` in a Flatpak too. Its associations and autostart entry name it, so one
//! profile at a time opens the types and each starts at login apart. In a Flatpak only the
//! default profile can start at login (the portal keeps one autostart per application).
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
/// Flatpak, and on macOS only from PaTorrent.app (LaunchServices names programs by bundle id)
/// in its default profile (LaunchServices opens the bundle, never a profile).
pub fn associations_settable() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::macos::bundle_id().is_some() && patorrent_frontend::profile::current().is_default()
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

pub use imp::{profile_entry, read, show_profile_entry, write};

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::{Path, PathBuf};

    use gtk::prelude::*;
    use gtk::{gio, glib};
    use patorrent_frontend::integration::{
        self, EntryKind, Integration, MIME_MAGNET, MIME_TORRENT, MINIMIZED,
    };
    use patorrent_frontend::profile;

    use crate::app::APP_ID;

    /// The desktop entry's id: the application id, so a named profile's (D42) is its own
    /// (`<APP_ID>.profile_work.desktop`), which the desktop matches to the profile's windows.
    fn desktop_id() -> String {
        format!("{}.desktop", profile::current().app_id(APP_ID))
    }

    /// The command that starts this program in this profile: the AppImage file when run from
    /// one, else this executable; in a Flatpak (only a named profile's entry asks) `flatpak run`.
    pub fn command() -> Vec<String> {
        let mut c = if super::packaged() {
            ["flatpak", "run", "--command=patorrent-gtk", APP_ID].map(str::to_owned).to_vec()
        } else if let Some(image) = std::env::var_os("APPIMAGE") {
            vec![image.to_string_lossy().into_owned()]
        } else {
            match std::env::current_exe() {
                Ok(p) => vec![p.to_string_lossy().into_owned()],
                Err(_) => return Vec::new(),
            }
        };
        c.extend(profile::current().args());
        c
    }

    /// `applications/` in the user's data folder: the host's in a Flatpak (its home is
    /// visible, `--filesystem=home`), where the sandbox's own would be seen by nobody.
    fn applications_dir() -> PathBuf {
        if super::packaged() {
            return glib::home_dir().join(".local").join("share").join("applications");
        }
        glib::user_data_dir().join("applications")
    }

    fn app_entry_path() -> PathBuf {
        applications_dir().join(desktop_id())
    }

    /// A named profile's application entry, written at its start when missing or not starting
    /// this program (a moved AppImage), hidden from the menu unless File > Profiles showed it:
    /// without it a Wayland desktop finds no entry for the profile's windows and shows a
    /// generic icon. Nothing for the default profile, whose entry the package installs. True
    /// when it wrote the entry just now.
    pub fn profile_entry() -> Result<bool, String> {
        if profile::current().is_default() {
            return Ok(false);
        }
        let (path, command) = (app_entry_path(), command());
        let old = std::fs::read_to_string(&path).ok();
        if command.is_empty() || old.as_deref().is_some_and(|t| integration::starts(t, &command)) {
            return Ok(false);
        }
        let shown = old.as_deref().is_some_and(integration::shown);
        write_file(&path, &integration::entry_shown(&command, APP_ID, EntryKind::App, shown))?;
        // a Flatpak exports its icon itself
        if !super::packaged() {
            install_icon()?;
        }
        Ok(true)
    }

    /// File > Profiles > Create Shortcut: the named profile's entry in the applications menu
    /// (`NoDisplay` dropped), as `PaTorrent (name)`.
    pub fn show_profile_entry() -> Result<(), String> {
        let command = command();
        if profile::current().is_default() || command.is_empty() {
            return Err("only a named profile has an entry of its own".into());
        }
        write_file(&app_entry_path(), &integration::entry_shown(&command, APP_ID, EntryKind::App, true))?;
        if !super::packaged() {
            install_icon()?;
        }
        Ok(())
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
            // written again, so it starts this program (a moved AppImage, a new build); a
            // profile's entry stays in or out of the menu as it was
            let shown = std::fs::read_to_string(app_entry_path()).map_or(true, |t| integration::shown(&t));
            write_file(
                &app_entry_path(),
                &integration::entry_shown(&command, APP_ID, EntryKind::App, shown),
            )?;
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

        /// Autostart as the portal last granted it; never an association. The portal keeps one
        /// autostart per application: the default profile's (D42).
        pub fn read() -> Integration {
            let s = state();
            let autostart =
                s.get_bool("autostart", false) && patorrent_frontend::profile::current().is_default();
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
                if !patorrent_frontend::profile::current().is_default() {
                    return Err("in the Flatpak only the default profile can start at login: the \
                         Background portal keeps one autostart per application"
                        .into());
                }
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
    use std::path::PathBuf;

    use gtk::glib;
    use patorrent_frontend::integration::{Integration, MINIMIZED};

    use crate::app::APP_ID;
    use crate::macos;

    /// What the LaunchAgent starts: the bundle's own executable inside PaTorrent.app (starting
    /// `patorrent-gtk` there directly would skip the launcher's paths), else this executable;
    /// in a named profile with `--profile <name>` (D42).
    pub fn command() -> Vec<String> {
        let Some(program) = macos::bundle_executable().or_else(|| std::env::current_exe().ok()) else {
            return Vec::new();
        };
        let mut c = vec![program.to_string_lossy().into_owned()];
        c.extend(patorrent_frontend::profile::current().args());
        c
    }

    /// The LaunchAgent's label: the application id, `<APP_ID>.<name>` for a named profile, so
    /// each profile starts at login apart from the others.
    fn agent_label() -> String {
        match &patorrent_frontend::profile::current().name {
            Some(n) => format!("{APP_ID}.{n}"),
            None => APP_ID.to_owned(),
        }
    }

    fn agent_path() -> PathBuf {
        glib::home_dir().join("Library").join("LaunchAgents").join(format!("{}.plist", agent_label()))
    }

    /// Nothing on macOS: LaunchServices names the bundle, never a profile's entry.
    pub fn profile_entry() -> Result<bool, String> {
        Ok(false)
    }

    /// Not on macOS (its menu item is greyed): the Dock keeps the bundle.
    pub fn show_profile_entry() -> Result<(), String> {
        Err("macOS starts profiles from the command line or a script".into())
    }

    /// The LaunchAgent's arguments after the program and profile, when it starts this program.
    fn agent_args(command: &[String]) -> Option<Vec<String>> {
        let text = std::fs::read_to_string(agent_path()).ok()?;
        let args = macos::launch_agent_arguments(&text)?;
        (!command.is_empty() && args.starts_with(command)).then(|| args[command.len()..].to_vec())
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
                std::fs::write(&path, macos::launch_agent(&agent_label(), &program)).map_err(fail)?;
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
