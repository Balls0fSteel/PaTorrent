//! Where a GUI keeps its files, generalised from GTK's `paths.rs`:
//!
//! | | settings (`rookparcel.ini`, `rss.dat`) | library (`resume.db`, ...) |
//! |---|---|---|
//! | Linux and the BSDs | `$XDG_CONFIG_HOME/rookparcel/<gui>`, else `~/.config/rookparcel/<gui>` | `$XDG_DATA_HOME/rookparcel/<gui>`, else `~/.local/share/rookparcel/<gui>` |
//! | macOS | `~/Library/Application Support/Rookparcel/<gui>` | the same |
//! | portable | the portable folder | the same |
//!
//! `<gui>` is the GUI's own folder (`gtk`, `mac`). Relative or empty XDG variables are ignored,
//! as the spec says; macOS reads none. A named profile (D42) puts `profiles/<name>` before
//! `<gui>` (in a portable folder: `profiles/<name>` in it).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{profile, tr};

/// A window's folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirs {
    /// The profile's settings folder: `rookparcel.ini` (the language and `remote.ini` stay
    /// here for every window).
    pub config: PathBuf,
    /// This window's settings folder: [`Dirs::config`], or a remote window's own folder in it
    /// ([`Dirs::for_remote`]).
    pub settings: PathBuf,
    /// The torrent library.
    pub data: PathBuf,
    /// The folder whose `profiles/` holds the named profiles' settings (File > Profiles).
    pub root: PathBuf,
}

impl Dirs {
    /// A remote window's (`remote/<host>_<port>` in the settings folder).
    pub fn for_remote(mut self, label: &str) -> Dirs {
        self.settings = crate::remote::data_dir(&self.config, label);
        self
    }
}

/// The two layouts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    /// `~/Library/Application Support`.
    MacOs,
    /// XDG's base directories.
    Unix,
}

impl Os {
    /// This build's.
    pub fn current() -> Os {
        if cfg!(target_os = "macos") { Os::MacOs } else { Os::Unix }
    }
}

/// This process's folders (its profile, portable mode) for the GUI folder `gui`, with the
/// environment read through `env` (`|k| std::env::var_os(k)`).
pub fn dirs(gui: &str, env: impl Fn(&str) -> Option<OsString>) -> Dirs {
    let p = profile::current();
    dirs_for(gui, Os::current(), p.name.as_deref(), p.portable_root.as_deref(), env)
}

/// The folders of profile `name` (`None`: the default one) on `os`, in the portable folder
/// `portable` if there is one.
pub fn dirs_for(
    gui: &str,
    os: Os,
    name: Option<&str>,
    portable: Option<&Path>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Dirs {
    if let Some(root) = portable {
        let d = profile::within(root, name);
        return Dirs { config: d.clone(), settings: d.clone(), data: d, root: root.to_path_buf() };
    }
    let home = env("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    match os {
        Os::MacOs => {
            let root = ["Library", "Application Support", "Rookparcel"].iter().fold(home, |p, c| p.join(c));
            let d = profile::within(&root, name).join(gui);
            Dirs { config: d.clone(), settings: d.clone(), data: d, root }
        }
        Os::Unix => {
            let base = |var: &str, fallback: &[&str]| {
                match env(var).map(PathBuf::from) {
                    Some(p) if p.is_absolute() => p,
                    _ => fallback.iter().fold(home.clone(), |p, c| p.join(c)),
                }
                .join("rookparcel")
            };
            let config_root = base("XDG_CONFIG_HOME", &[".config"]);
            let config = profile::within(&config_root, name).join(gui);
            let data = profile::within(&base("XDG_DATA_HOME", &[".local", "share"]), name).join(gui);
            Dirs { settings: config.clone(), config, data, root: config_root }
        }
    }
}

/// Takes `rookparcel.lock` in the library's folder (one process per library, D42), creating
/// the folder. The lock is held until it is dropped; `Ok(None)` when it could not be made for
/// another reason (an unwritable folder fails at the engine's start, which says why). `Err` is
/// the message for the user (heading, text), as GTK shows it.
pub fn lock_library(dir: &Path) -> Result<Option<rookparcel_core::util::lockfile::Lock>, (String, String)> {
    use rookparcel_core::util::lockfile;
    let made = rookparcel_core::util::private_file::create_dir_all(dir);
    let text = match made.and_then(|()| lockfile::Lock::take(&dir.join(lockfile::FILE_NAME))) {
        Ok(l) => return Ok(Some(l)),
        Err(e) if lockfile::is_taken(&e) => tr!(FolderInUse, folder = dir.display()),
        // a portable folder never falls back to the user's own (D42)
        Err(e) if profile::current().portable_root.is_some() => {
            tr!(PortableReadOnly, folder = dir.display(), error = e)
        }
        Err(_) => return Ok(None),
    };
    // its first paragraph is the dialog's heading, the rest the text below it
    Err(match text.split_once("\n\n") {
        Some((head, rest)) => (head.to_owned(), rest.to_owned()),
        None => (text, String::new()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute folder on the machine the tests run on.
    const CFG: &str = if cfg!(windows) { r"C:\cfg" } else { "/cfg" };

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn linux_default_profile_follows_xdg() {
        let d = dirs_for("mac", Os::Unix, None, None, env(&[("HOME", "/home/u")]));
        assert_eq!(d.config, PathBuf::from("/home/u/.config/rookparcel/mac"));
        assert_eq!(d.settings, d.config);
        assert_eq!(d.data, PathBuf::from("/home/u/.local/share/rookparcel/mac"));
        assert_eq!(d.root, PathBuf::from("/home/u/.config/rookparcel"));
        // absolute variables win; relative and empty ones are ignored
        let d = dirs_for(
            "gtk",
            Os::Unix,
            None,
            None,
            env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", CFG), ("XDG_DATA_HOME", "rel")]),
        );
        assert_eq!(d.config, Path::new(CFG).join("rookparcel").join("gtk"));
        assert_eq!(d.data, PathBuf::from("/home/u/.local/share/rookparcel/gtk"));
        let d = dirs_for("gtk", Os::Unix, None, None, env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "")]));
        assert_eq!(d.config, PathBuf::from("/home/u/.config/rookparcel/gtk"));
    }

    #[test]
    fn a_profile_goes_before_the_gui_folder() {
        let d = dirs_for("mac", Os::Unix, Some("work"), None, env(&[("HOME", "/home/u")]));
        assert_eq!(d.config, PathBuf::from("/home/u/.config/rookparcel/profiles/work/mac"));
        assert_eq!(d.data, PathBuf::from("/home/u/.local/share/rookparcel/profiles/work/mac"));
        assert_eq!(d.root, PathBuf::from("/home/u/.config/rookparcel"));
        let d = dirs_for("mac", Os::MacOs, Some("vpn"), None, env(&[("HOME", "/Users/u")]));
        let lib = PathBuf::from("/Users/u/Library/Application Support/Rookparcel/profiles/vpn/mac");
        assert_eq!((&d.config, &d.settings, &d.data), (&lib, &lib, &lib));
        assert_eq!(d.root, PathBuf::from("/Users/u/Library/Application Support/Rookparcel"));
    }

    #[test]
    fn macos_reads_no_xdg_variable() {
        let d =
            dirs_for("mac", Os::MacOs, None, None, env(&[("HOME", "/Users/u"), ("XDG_CONFIG_HOME", CFG)]));
        let lib = PathBuf::from("/Users/u/Library/Application Support/Rookparcel/mac");
        assert_eq!((&d.config, &d.data), (&lib, &lib));
    }

    #[test]
    fn portable_mode_keeps_everything_in_its_folder() {
        let root = Path::new("/media/stick/Rookparcel");
        for os in [Os::MacOs, Os::Unix] {
            let d =
                dirs_for("mac", os, None, Some(root), env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/d")]));
            assert_eq!(
                (&d.config, &d.settings, &d.data, &d.root),
                (&root.into(), &root.into(), &root.into(), &root.into())
            );
            let d = dirs_for("mac", os, Some("work"), Some(root), env(&[("HOME", "/home/u")]));
            let w = root.join("profiles").join("work");
            assert_eq!((&d.config, &d.data, d.root.as_path()), (&w, &w, root));
        }
    }

    #[test]
    fn a_remote_window_has_its_own_settings() {
        let d = dirs_for("mac", Os::MacOs, None, None, env(&[("HOME", "/Users/u")])).for_remote("nas_8080");
        assert_eq!(d.config, PathBuf::from("/Users/u/Library/Application Support/Rookparcel/mac"));
        assert_eq!(d.settings, crate::remote::data_dir(&d.config, "nas_8080"));
        assert_ne!(d.settings, d.config);
    }
}
