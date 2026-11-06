//! Where the Mac GUI keeps its files (D61): `~/Library/Application Support/Rookparcel/mac`, both
//! the settings (`rookparcel.ini`, `rss.dat`) and the torrent library (`state/`); a named profile
//! (D42) in `Rookparcel/profiles/<name>/mac`. Portable mode (`rookparcel.ini` beside
//! `Rookparcel.app`, `profile::portable_root`) keeps everything in that folder (a named profile's
//! in its `profiles/<name>`), as the other GUIs do.
//!
//! A stand-in until slice A's `frontend::app::dirs` replaces it (docs/MACOS_GUI.md); pure Rust,
//! tested on every system.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rookparcel_frontend::profile;

/// The program's folder below `Rookparcel` (the GTK bundle's is `gtk`).
pub const GUI: &str = "mac";

/// The settings and library folder of this process's profile.
pub fn data_dir() -> PathBuf {
    let p = profile::current();
    dir_of(&|k| std::env::var_os(k), p.portable_root.as_deref(), p.name.as_deref())
}

/// The folder whose `profiles/` holds the named profiles (File > Profiles lists them).
pub fn profiles_root() -> PathBuf {
    let p = profile::current();
    match &p.portable_root {
        Some(root) => root.clone(),
        None => app_root(&|k| std::env::var_os(k)),
    }
}

/// `~/Library/Application Support/Rookparcel`.
fn app_root(env: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    let home = env("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    ["Library", "Application Support", "Rookparcel"].iter().fold(home, |p, c| p.join(c))
}

/// The folder of profile `name` (`None`: the default one), portable under `portable`.
pub fn dir_of(
    env: &dyn Fn(&str) -> Option<OsString>,
    portable: Option<&Path>,
    name: Option<&str>,
) -> PathBuf {
    match portable {
        Some(root) => profile::within(root, name),
        None => profile::within(&app_root(env), name).join(GUI),
    }
}

/// `~/Downloads`, the download folder until Preferences names another.
pub fn downloads() -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Downloads").to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_and_portable() {
        let env = |k: &str| (k == "HOME").then(|| OsString::from("/Users/u"));
        assert_eq!(
            dir_of(&env, None, None),
            PathBuf::from("/Users/u/Library/Application Support/Rookparcel/mac")
        );
        assert_eq!(
            dir_of(&env, None, Some("work")),
            PathBuf::from("/Users/u/Library/Application Support/Rookparcel/profiles/work/mac")
        );
        let root = Path::new("/Volumes/Stick/Rookparcel");
        assert_eq!(dir_of(&env, Some(root), None), root);
        assert_eq!(dir_of(&env, Some(root), Some("vpn")), root.join("profiles").join("vpn"));
    }
}
