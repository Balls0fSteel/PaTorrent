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

/// The GTK bundle's folder below `Application Support/Rookparcel` (and each profile's), and the
/// native Mac GUI's that takes it over (D61).
const GTK_DIR: &str = "gtk";
const MAC_DIR: &str = "mac";

/// The native Mac GUI's first start takes over the GTK bundle's libraries, once (D61): below
/// `~/Library/Application Support/Rookparcel`, the default profile's `gtk` folder and each
/// `profiles/<name>/gtk` become `mac` ([`take_over_gtk_in`]). Nothing in portable mode (the
/// portable folder is shared as it is). `Err` names this profile's `gtk` folder while a GTK
/// Rookparcel runs on it, or when it could not be moved: nothing should start on a new `mac`
/// folder then.
pub fn take_over_gtk() -> Result<(), PathBuf> {
    let p = crate::profile::current();
    if p.portable_root.is_some() {
        return Ok(());
    }
    let Some(home) = std::env::var_os("HOME").filter(|h| Path::new(h).is_absolute()) else { return Ok(()) };
    let root = Path::new(&home).join("Library").join("Application Support").join("Rookparcel");
    take_over_gtk_in(&root, p.name.as_deref())
}

/// [`take_over_gtk`] in `root` for the running profile `current` (`None`: the default one): each
/// `gtk` folder (the root's and every `profiles/<name>`'s) whose `mac` sibling is absent and
/// whose `rookparcel.lock` is free is renamed to `mac`, and the paths into it in its settings
/// files (its own `rookparcel.ini` and its remote windows') follow it. A folder a GTK Rookparcel
/// holds is left for a later start; only `current`'s is an error.
pub fn take_over_gtk_in(root: &Path, current: Option<&str>) -> Result<(), PathBuf> {
    let mut bases: Vec<(Option<String>, PathBuf)> = vec![(None, root.to_path_buf())];
    if let Ok(dir) = std::fs::read_dir(root.join(crate::profile::FOLDER)) {
        for e in dir.flatten() {
            if e.path().is_dir() {
                bases.push((Some(e.file_name().to_string_lossy().into_owned()), e.path()));
            }
        }
    }
    let mut result = Ok(());
    for (name, base) in bases {
        let (gtk, mac) = (base.join(GTK_DIR), base.join(MAC_DIR));
        if mac.exists() || !gtk.is_dir() {
            continue;
        }
        let mine = name.as_deref() == current;
        if lockfile::is_held(&gtk.join(lockfile::FILE_NAME)) {
            if mine {
                result = Err(gtk);
            }
            continue;
        }
        if let Err(e) = std::fs::rename(&gtk, &mac) {
            eprintln!("! moving {} to {}: {e}", gtk.display(), mac.display());
            if mine {
                result = Err(gtk);
            }
            continue;
        }
        let mut inis = vec![mac.join(crate::profile::PORTABLE_MARKER)];
        if let Ok(remote) = std::fs::read_dir(mac.join("remote")) {
            inis.extend(remote.flatten().map(|e| e.path().join(crate::profile::PORTABLE_MARKER)));
        }
        for ini in inis {
            repoint_folder(&ini, &gtk, &mac);
        }
        eprintln!("* took over the GTK bundle's folder {} as {}", gtk.display(), mac.display());
    }
    result
}

/// Rewrites the paths into the folder `was` in the settings file `ini` to `now`: only whole
/// path parts (`.../gtk`, `.../gtk/...`), never `.../gtk2`.
fn repoint_folder(ini: &Path, was: &Path, now: &Path) {
    let (Some(was), Some(now)) = (was.to_str(), now.to_str()) else { return };
    let Ok(text) = std::fs::read_to_string(ini) else { return };
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find(was) {
        let after = &rest[at + was.len()..];
        out.push_str(&rest[..at]);
        let whole = after.is_empty() || after.starts_with(['/', '\\', '\n', '\r', '|']);
        out.push_str(if whole { now } else { was });
        rest = after;
    }
    out.push_str(rest);
    if out != text {
        let _ = std::fs::write(ini, out);
    }
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

    /// The GTK bundle's 1.2.5 library under Application Support (D61): settings with paths into
    /// it, a remote window's folder, a profile.
    fn gtk_library(base: &Path) -> (PathBuf, PathBuf) {
        let gtk = base.join("gtk");
        let work = base.join("profiles").join("work").join("gtk");
        std::fs::create_dir_all(gtk.join("remote").join("nas_8080")).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let ini = format!(
            "ipfilter.path={}\nadd.recent_dirs={}|{}\nother={}2/x\n",
            gtk.join("ipfilter.dat").display(),
            gtk.join("in").display(),
            gtk.display(),
            gtk.display()
        );
        std::fs::write(gtk.join("rookparcel.ini"), ini).unwrap();
        std::fs::write(gtk.join("resume.db"), "db").unwrap();
        let remote = format!("dir.torrents={}\n", gtk.join("remote").display());
        std::fs::write(gtk.join("remote").join("nas_8080").join("rookparcel.ini"), remote).unwrap();
        std::fs::write(work.join("rookparcel.ini"), "ui.language=hu\n").unwrap();
        (gtk, work)
    }

    #[test]
    fn the_gtk_library_becomes_the_mac_one_once() {
        let base = temp("gtk");
        let (gtk, work) = gtk_library(&base);
        let mac = base.join("mac");
        take_over_gtk_in(&base, None).unwrap();
        assert!(!gtk.exists() && !work.exists());
        assert_eq!(std::fs::read_to_string(mac.join("resume.db")).unwrap(), "db");
        let ini = std::fs::read_to_string(mac.join("rookparcel.ini")).unwrap();
        let want = format!(
            "ipfilter.path={}\nadd.recent_dirs={}|{}\nother={}2/x\n",
            mac.join("ipfilter.dat").display(),
            mac.join("in").display(),
            mac.display(),
            gtk.display()
        );
        assert_eq!(ini, want, "whole path parts only: gtk2 stays");
        let remote =
            std::fs::read_to_string(mac.join("remote").join("nas_8080").join("rookparcel.ini")).unwrap();
        assert_eq!(remote, format!("dir.torrents={}\n", mac.join("remote").display()));
        let work_mac = base.join("profiles").join("work").join("mac");
        assert_eq!(std::fs::read_to_string(work_mac.join("rookparcel.ini")).unwrap(), "ui.language=hu\n");

        // once: a mac folder is never replaced by a gtk one that turns up again
        std::fs::create_dir_all(&gtk).unwrap();
        std::fs::write(gtk.join("rookparcel.ini"), "new gtk\n").unwrap();
        take_over_gtk_in(&base, None).unwrap();
        assert!(gtk.join("rookparcel.ini").is_file());
        assert_eq!(std::fs::read_to_string(mac.join("rookparcel.ini")).unwrap(), want);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_running_gtk_rookparcel_keeps_its_library() {
        let base = temp("gtk-held");
        let (gtk, work) = gtk_library(&base);
        // the GTK bundle runs the "work" profile
        let running = lockfile::Lock::take(&work.join(lockfile::FILE_NAME)).unwrap();
        // the default profile starts: its library moves, "work"'s waits for a later start
        take_over_gtk_in(&base, None).unwrap();
        assert!(base.join("mac").is_dir() && !gtk.exists());
        assert!(work.is_dir() && !base.join("profiles").join("work").join("mac").exists());
        // "work" itself cannot start on an empty library meanwhile
        assert_eq!(take_over_gtk_in(&base, Some("work")), Err(work.clone()));
        drop(running);
        take_over_gtk_in(&base, Some("work")).unwrap();
        assert!(base.join("profiles").join("work").join("mac").join("rookparcel.ini").is_file());
        // nothing to take over: nothing happens
        let empty = temp("gtk-none");
        take_over_gtk_in(&empty, Some("x")).unwrap();
        assert!(!empty.join("mac").exists());
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&empty).unwrap();
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
