//! Where the Mac GUI keeps its files (D61): `~/Library/Application Support/Rookparcel/mac`, both
//! the settings (`rookparcel.ini`, `rss.dat`) and the torrent library; a named profile (D42) in
//! `Rookparcel/profiles/<name>/mac`. Portable mode (`rookparcel.ini` beside `Rookparcel.app`,
//! `profile::portable_root`) keeps everything in that folder (a named profile's in its
//! `profiles/<name>`), as the other GUIs do. The layout is `frontend::app::dirs`'s.

use std::path::PathBuf;

use rookparcel_frontend::app::Dirs;

/// The program's folder below `Rookparcel` (the GTK bundle's is `gtk`).
pub const GUI: &str = "mac";

/// This process's folders (its profile, portable mode).
pub fn dirs() -> Dirs {
    rookparcel_frontend::app::dirs(GUI, |k| std::env::var_os(k))
}

/// The settings folder of this process's profile (`rookparcel.ini`).
pub fn data_dir() -> PathBuf {
    dirs().config
}

/// The folder whose `profiles/` holds the named profiles (File > Profiles lists them).
pub fn profiles_root() -> PathBuf {
    dirs().root
}

/// `~/Downloads`, the download folder until Preferences names another.
pub fn downloads() -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Downloads").to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use rookparcel_frontend::app::{Os, dirs_for};

    use super::GUI;

    #[test]
    fn default_profile_named_profile_and_portable() {
        let env = |k: &str| (k == "HOME").then(|| OsString::from("/Users/u"));
        let mac = PathBuf::from("/Users/u/Library/Application Support/Rookparcel");
        let d = dirs_for(GUI, Os::MacOs, None, None, env);
        assert_eq!(d.data, mac.join("mac"));
        assert_eq!(d.config, d.data);
        assert_eq!(d.root, mac);
        assert_eq!(dirs_for(GUI, Os::MacOs, Some("work"), None, env).data, mac.join("profiles/work/mac"));
        let root = Path::new("/Volumes/Stick/Rookparcel");
        assert_eq!(dirs_for(GUI, Os::MacOs, None, Some(root), env).data, root);
        assert_eq!(
            dirs_for(GUI, Os::MacOs, Some("vpn"), Some(root), env).data,
            root.join("profiles").join("vpn")
        );
    }
}
