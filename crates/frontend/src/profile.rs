//! Profiles and portable mode (D42, docs/PROFILES_PLAN.md).
//!
//! Portable mode (D7): a `patorrent.ini` file beside the executable makes its folder the
//! application root, so settings and state travel with the program, as µTorrent's
//! `settings.dat` beside `uTorrent.exe` did. One test for every frontend: [`portable_root`].

use std::path::{Path, PathBuf};

/// The file whose presence beside the executable means portable mode.
pub const PORTABLE_MARKER: &str = "patorrent.ini";

/// The portable root: the executable's folder when [`PORTABLE_MARKER`] is a file there. Windows
/// only for now; the other platforms follow with slice 6 of the plan.
pub fn portable_root() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    portable_root_in(exe.parent()?)
}

/// `dir` when it holds the portable marker as a file (a folder of that name does not count).
pub fn portable_root_in(dir: &Path) -> Option<PathBuf> {
    dir.join(PORTABLE_MARKER).is_file().then(|| dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_must_be_a_file() {
        let dir = std::env::temp_dir().join(format!("pa-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(portable_root_in(&dir), None);
        std::fs::create_dir(dir.join(PORTABLE_MARKER)).unwrap();
        assert_eq!(portable_root_in(&dir), None, "a folder named patorrent.ini");
        std::fs::remove_dir(dir.join(PORTABLE_MARKER)).unwrap();
        std::fs::write(dir.join(PORTABLE_MARKER), "").unwrap();
        assert_eq!(portable_root_in(&dir), Some(dir.clone()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
