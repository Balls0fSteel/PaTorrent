//! Where the GTK client keeps its files, after the XDG Base Directory spec (as nox does):
//!
//! | | default |
//! |---|---|
//! | settings (`patorrent.ini`, `rss.dat`, `remote.ini`) | `$XDG_CONFIG_HOME/patorrent/gtk`, else `~/.config/patorrent/gtk` |
//! | a remote window's settings | `remote/<host>_<port>` in the settings folder |
//! | inside a Flatpak, what the Background portal granted for autostart (`background.ini`) | the settings folder |
//! | torrent library (`resume.db`, `metadata.pack`, `dht_state.dat`) | `$XDG_DATA_HOME/patorrent/gtk`, else `~/.local/share/patorrent/gtk` |
//!
//! Relative or empty XDG variables are ignored, as the spec says.
//!
//! On macOS both are `~/Library/Application Support/PaTorrent/gtk` (their file names differ),
//! where Apple puts an application's own files; the XDG variables are not read there.
//!
//! A named profile (D42, `--profile work`) puts `profiles/work` before `gtk` in both:
//! `~/.config/patorrent/profiles/work/gtk`, `~/.local/share/patorrent/profiles/work/gtk`.
//!
//! Portable mode (`patorrent.ini` beside the program or the `.AppImage`,
//! `patorrent_frontend::profile::portable_root`) keeps both in that folder (a named profile's in
//! its `profiles/<name>`), as the Windows GUI does, and reads no XDG variable.

use std::ffi::OsString;
use std::path::PathBuf;

use patorrent_frontend::profile;

/// The application's folder below the base directories, and the program's below that.
const APP: [&str; 2] = ["patorrent", "gtk"];

/// The settings folder.
pub fn config_dir() -> PathBuf {
    config_dir_of(profile_name())
}

/// The portable folder, when there is one (D42): settings and library both live in it (in
/// `profiles/<name>` for a named profile), as the Windows GUI's do, and nothing in XDG's.
fn portable() -> Option<&'static std::path::Path> {
    profile::current().portable_root.as_deref()
}

/// [`config_dir`] of the profile `name` (`None`: the default one).
pub fn config_dir_of(name: Option<&str>) -> PathBuf {
    if let Some(root) = portable() {
        return profile::within(root, name);
    }
    if cfg!(target_os = "macos") {
        return macos_dir(&|k| std::env::var_os(k), name);
    }
    dir(&|k| std::env::var_os(k), "XDG_CONFIG_HOME", &[".config"], name)
}

/// [`data_dir`] of the profile `name`.
pub fn data_dir_of(name: Option<&str>) -> PathBuf {
    if let Some(root) = portable() {
        return profile::within(root, name);
    }
    if cfg!(target_os = "macos") {
        return macos_dir(&|k| std::env::var_os(k), name);
    }
    dir(&|k| std::env::var_os(k), "XDG_DATA_HOME", &[".local", "share"], name)
}

/// The folder whose `profiles/` holds the named profiles' settings (File > Profiles lists them).
pub fn config_root() -> PathBuf {
    if let Some(root) = portable() {
        return root.to_path_buf();
    }
    config_dir_of(None).parent().map(PathBuf::from).unwrap_or_default()
}

fn profile_name() -> Option<&'static str> {
    profile::current().name.as_deref()
}

/// This window's settings folder: [`config_dir`], or a remote window's own folder in it
/// (`remote/<host>_<port>`, `crate::remote`). The language and `remote.ini` stay in
/// [`config_dir`] for every window.
pub fn settings_dir() -> PathBuf {
    match crate::remote::get() {
        Some(r) => patorrent_frontend::remote::data_dir(&config_dir(), &r.label),
        None => config_dir(),
    }
}

/// Inside a Flatpak, what the Background portal last granted for "Start PaTorrent on system
/// startup" (`crate::integration`): the portal cannot be asked afterwards, so the answer is kept
/// here. A file of its own beside `patorrent.ini`, which the main window keeps in memory and
/// writes back whole, so it would lose a value written behind its back.
#[cfg(not(target_os = "macos"))]
pub fn background_file() -> PathBuf {
    config_dir().join("background.ini")
}

/// The torrent library's folder.
pub fn data_dir() -> PathBuf {
    data_dir_of(profile_name())
}

/// `<base>/patorrent[/profiles/<name>]/gtk`.
fn dir(env: &dyn Fn(&str) -> Option<OsString>, var: &str, fallback: &[&str], name: Option<&str>) -> PathBuf {
    let base = match env(var).map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => {
            let home = env("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            fallback.iter().fold(home, |p, c| p.join(c))
        }
    };
    profile::within(&base.join(APP[0]), name).join(APP[1])
}

/// `$HOME/Library/Application Support/PaTorrent[/profiles/<name>]/gtk`.
fn macos_dir(env: &dyn Fn(&str) -> Option<OsString>, name: Option<&str>) -> PathBuf {
    let home = env("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let root = ["Library", "Application Support", "PaTorrent"].iter().fold(home, |p, c| p.join(c));
    profile::within(&root, name).join(APP[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_variables_win_when_absolute() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
        };
        let e = env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/cfg")]);
        assert_eq!(dir(&e, "XDG_CONFIG_HOME", &[".config"], None), PathBuf::from("/cfg/patorrent/gtk"));
        let e = env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "rel")]);
        assert_eq!(
            dir(&e, "XDG_DATA_HOME", &[".local", "share"], None),
            PathBuf::from("/home/u/.local/share/patorrent/gtk")
        );
        let e = env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "")]);
        assert_eq!(
            dir(&e, "XDG_CONFIG_HOME", &[".config"], None),
            PathBuf::from("/home/u/.config/patorrent/gtk")
        );
        let e = env(&[("HOME", "/Users/u"), ("XDG_CONFIG_HOME", "/cfg")]);
        assert_eq!(macos_dir(&e, None), PathBuf::from("/Users/u/Library/Application Support/PaTorrent/gtk"));
    }

    #[test]
    fn a_profile_goes_before_gtk() {
        let e = |k: &str| (k == "HOME").then(|| OsString::from("/home/u"));
        assert_eq!(
            dir(&e, "XDG_CONFIG_HOME", &[".config"], Some("work")),
            PathBuf::from("/home/u/.config/patorrent/profiles/work/gtk")
        );
        assert_eq!(
            dir(&e, "XDG_DATA_HOME", &[".local", "share"], Some("work")),
            PathBuf::from("/home/u/.local/share/patorrent/profiles/work/gtk")
        );
        assert_eq!(
            macos_dir(&e, Some("vpn")),
            PathBuf::from("/home/u/Library/Application Support/PaTorrent/profiles/vpn/gtk")
        );
    }
}
