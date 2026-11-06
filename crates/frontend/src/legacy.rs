//! The former name, PaTorrent (D59): finding its folders and taking them over once.
//!
//! PaTorrent 1.0-1.2 kept its settings and library in `%APPDATA%\PaTorrent`, in
//! `~/Library/Application Support/PaTorrent`, or in the XDG `patorrent` folders, with the
//! settings in `patorrent.ini` (also the portable marker) and the lock in `patorrent.lock`.
//! [`migrate`] runs right after the profile is known, before any folder is read or made: each old
//! default root that has no new one beside it is renamed to the new name, and the files inside
//! it are renamed after it ([`rename_inside`]). A portable folder is taken over where it is, when
//! the profile finds the old marker ([`crate::profile::portable_root_in`]).
//!
//! Nothing moves while a PaTorrent still runs there (it holds its `patorrent.lock`): [`migrate`]
//! names the folder instead, and the caller says it is in use. [`rookparcel_core::util::lockfile`]
//! holds the old lock as well as its own, so a folder never serves both programs at once.
//!
//! The Windows nox service's `%ProgramData%\PaTorrent` is moved by `rookparcel-nox service
//! install`, which replaces the old service; system services on Linux and FreeBSD are moved by
//! hand (docs/DAEMON.md).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rookparcel_core::util::lockfile;

/// The settings file (and portable marker) under the former name.
pub const OLD_INI: &str = "patorrent.ini";

/// The folder name under the former name: `%APPDATA%` and Application Support.
const OLD_DIR: &str = "PaTorrent";
/// The same below the XDG base directories.
const OLD_XDG: &str = "patorrent";

/// Moves the former name's default folders to the new name, once. `Err` names a folder that a
/// PaTorrent still uses, or that could not be moved (the reason went to stderr); nothing should
/// start on the new folder then, or the old data would be left behind for good.
pub fn migrate() -> Result<(), PathBuf> {
    if crate::profile::current().portable_root.is_some() {
        return Ok(());
    }
    for (old, new) in default_roots(&|k| std::env::var_os(k)) {
        move_root(&old, &new)?;
    }
    Ok(())
}

/// The old and new default roots on this system.
pub fn default_roots(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<(PathBuf, PathBuf)> {
    let absolute = |v: &OsString| Path::new(v).is_absolute() || v.as_encoded_bytes().first() == Some(&b'/');
    let pair = |base: PathBuf, old: &str, new: &str| (base.join(old), base.join(new));
    if cfg!(windows) {
        return env("APPDATA")
            .filter(|v| !v.is_empty())
            .map(|d| pair(PathBuf::from(d), OLD_DIR, "Rookparcel"))
            .into_iter()
            .collect();
    }
    let home = env("HOME").filter(absolute).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home
            .map(|h| pair(h.join("Library").join("Application Support"), OLD_DIR, "Rookparcel"))
            .into_iter()
            .collect();
    }
    let xdg = |var: &str, fallback: &[&str]| {
        env(var)
            .filter(absolute)
            .map(PathBuf::from)
            .or_else(|| home.clone().map(|h| fallback.iter().fold(h, |p, c| p.join(c))))
    };
    [xdg("XDG_CONFIG_HOME", &[".config"]), xdg("XDG_DATA_HOME", &[".local", "share"])]
        .into_iter()
        .flatten()
        .map(|base| pair(base, OLD_XDG, "rookparcel"))
        .collect()
}

/// Renames `old` to `new` when only `old` is there and no PaTorrent holds it, then the files in
/// it ([`rename_inside`]).
pub fn move_root(old: &Path, new: &Path) -> Result<(), PathBuf> {
    if new.exists() || !old.is_dir() {
        return Ok(());
    }
    if held(old) {
        return Err(old.to_path_buf());
    }
    if let Some(parent) = new.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::rename(old, new) {
        eprintln!("! moving {} to {}: {e}", old.display(), new.display());
        return Err(old.to_path_buf());
    }
    rename_inside(new, Some(old));
    eprintln!("* moved PaTorrent's folder {} to {}", old.display(), new.display());
    Ok(())
}

/// A portable folder found by the old marker: its files take the new names, unless a PaTorrent
/// still runs there (then the lock refuses the start and nothing is renamed).
pub fn adopt_portable(dir: &Path) {
    if !held(dir) {
        rename_inside(dir, None);
    }
}

/// Whether a PaTorrent holds a lock anywhere in the root `root`.
pub fn held(root: &Path) -> bool {
    folders(root).iter().any(|d| lockfile::is_held(&d.join(lockfile::LEGACY_FILE_NAME)))
}

/// In every settings folder of `root` (its own, the profiles', nox's and GTK's, the remote
/// windows'): `patorrent.ini` becomes `rookparcel.ini`, and the old lock goes. When the root
/// itself moved from `was`, the settings' absolute paths into it follow it.
pub fn rename_inside(root: &Path, was: Option<&Path>) {
    for dir in folders(root) {
        let (old, new) = (dir.join(OLD_INI), dir.join(crate::profile::PORTABLE_MARKER));
        if old.is_file() && !new.exists() {
            if let Err(e) = std::fs::rename(&old, &new) {
                eprintln!("! renaming {}: {e}", old.display());
                continue;
            }
            if let Some(was) = was {
                repoint(&new, was, root);
            }
        }
        let _ = std::fs::remove_file(dir.join(lockfile::LEGACY_FILE_NAME));
    }
}

/// Rewrites the paths into the old root in the settings file `ini` to the new root.
fn repoint(ini: &Path, was: &Path, root: &Path) {
    let (Some(was), Some(now)) = (was.to_str(), root.to_str()) else { return };
    let Ok(text) = std::fs::read_to_string(ini) else { return };
    if text.contains(was) {
        let _ = std::fs::write(ini, text.replace(was, now));
    }
}

/// The folders below `root` that may hold settings or a lock.
fn folders(root: &Path) -> Vec<PathBuf> {
    let subdirs = |d: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(d)
            .map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default()
    };
    let mut bases = vec![root.to_path_buf()];
    bases.extend(subdirs(&root.join(crate::profile::FOLDER)));
    let mut out = Vec::new();
    for b in bases {
        for d in [b.clone(), b.join("nox"), b.join("gtk")] {
            out.extend(subdirs(&d.join("remote")));
            out.push(d);
        }
    }
    out
}

/// The environment variable `ROOKPARCEL_<suffix>`, else the former name's `PATORRENT_<suffix>`.
pub fn env_var(suffix: &str) -> Option<OsString> {
    std::env::var_os(format!("ROOKPARCEL_{suffix}"))
        .or_else(|| std::env::var_os(format!("PATORRENT_{suffix}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rookparcel-legacy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_roots_by_system() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
        };
        if cfg!(windows) {
            let r = default_roots(&env(&[("APPDATA", r"C:\Users\u\AppData\Roaming")]));
            assert_eq!(
                r,
                [(
                    PathBuf::from(r"C:\Users\u\AppData\Roaming\PaTorrent"),
                    PathBuf::from(r"C:\Users\u\AppData\Roaming\Rookparcel")
                )]
            );
            assert!(default_roots(&env(&[])).is_empty());
        } else if !cfg!(target_os = "macos") {
            let r = default_roots(&env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/data")]));
            assert_eq!(
                r,
                [
                    (PathBuf::from("/home/u/.config/patorrent"), PathBuf::from("/home/u/.config/rookparcel")),
                    (PathBuf::from("/data/patorrent"), PathBuf::from("/data/rookparcel")),
                ]
            );
        }
    }

    #[test]
    fn a_patorrent_root_moves_once_with_its_profiles() {
        let base = temp("move");
        let (old, new) = (base.join("PaTorrent"), base.join("Rookparcel"));
        let work = old.join("profiles").join("work");
        std::fs::create_dir_all(work.join("nox")).unwrap();
        let ini = format!("dir.ipfilter={}\n", old.join("ipfilter.dat").display());
        std::fs::write(old.join(OLD_INI), &ini).unwrap();
        std::fs::write(old.join("resume.db"), "db").unwrap();
        std::fs::write(old.join(lockfile::LEGACY_FILE_NAME), "").unwrap();
        std::fs::write(work.join(OLD_INI), "ui.language=hu\n").unwrap();
        std::fs::write(work.join("nox").join(OLD_INI), "").unwrap();

        move_root(&old, &new).unwrap();
        assert!(!old.exists());
        assert_eq!(std::fs::read_to_string(new.join("resume.db")).unwrap(), "db");
        let moved = std::fs::read_to_string(new.join("rookparcel.ini")).unwrap();
        assert_eq!(moved, format!("dir.ipfilter={}\n", new.join("ipfilter.dat").display()));
        assert!(!new.join(OLD_INI).exists() && !new.join(lockfile::LEGACY_FILE_NAME).exists());
        let work = new.join("profiles").join("work");
        assert_eq!(std::fs::read_to_string(work.join("rookparcel.ini")).unwrap(), "ui.language=hu\n");
        assert!(work.join("nox").join("rookparcel.ini").is_file());

        // once: a new root is never replaced, even if an old one turns up again
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(OLD_INI), "old").unwrap();
        move_root(&old, &new).unwrap();
        assert!(old.join(OLD_INI).is_file());
        assert_eq!(std::fs::read_to_string(new.join("rookparcel.ini")).unwrap(), moved);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn nothing_moves_under_a_running_patorrent() {
        let base = temp("held");
        let (old, new) = (base.join("PaTorrent"), base.join("Rookparcel"));
        let profile = old.join("profiles").join("work");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(old.join(OLD_INI), "").unwrap();
        // a PaTorrent of the "work" profile runs
        let running = lockfile::Lock::take(&profile.join(lockfile::LEGACY_FILE_NAME)).unwrap();
        assert_eq!(move_root(&old, &new), Err(old.clone()));
        assert!(old.join(OLD_INI).is_file() && !new.exists());
        drop(running);
        move_root(&old, &new).unwrap();
        assert!(new.join("rookparcel.ini").is_file());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_portable_folder_is_taken_over_in_place() {
        let dir = temp("portable");
        std::fs::write(dir.join(OLD_INI), "x=1\n").unwrap();
        assert_eq!(crate::profile::portable_root_in(&dir), Some(dir.clone()));
        assert_eq!(std::fs::read_to_string(dir.join("rookparcel.ini")).unwrap(), "x=1\n");
        assert!(!dir.join(OLD_INI).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
