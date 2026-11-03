//! Paths kept relative to a portable folder (D42): a PaTorrent that runs from a USB stick or a
//! synced folder stores the paths inside that folder as `./Downloads/...` (`/` separators), so
//! they still hold once the folder moves to another drive letter, mount point or computer, and
//! resolves them against wherever the folder is at load. Paths outside it stay absolute. In
//! memory every path is absolute; only what is written is relative.
//!
//! New code (libtorrent stores absolute paths).

use std::path::{Component, Path, PathBuf};

/// `path` relative to `root` as `./a/b` (`.` for `root` itself) when it is absolute and lies
/// inside `root`; `None` otherwise. Windows compares without case, as its file systems do.
pub fn relative_to(root: &Path, path: &str) -> Option<String> {
    let path = Path::new(path);
    if !path.is_absolute() || !root.is_absolute() {
        return None;
    }
    let mut rest = path.components();
    for want in root.components().filter(|c| *c != Component::CurDir) {
        if !same(want, rest.next()?) {
            return None;
        }
    }
    let mut out = String::from(".");
    for c in rest {
        match c {
            Component::Normal(n) => {
                out.push('/');
                out.push_str(n.to_str()?);
            }
            Component::CurDir => {}
            // `..` would climb out again: not inside
            _ => return None,
        }
    }
    Some(out)
}

fn same(a: Component<'_>, b: Component<'_>) -> bool {
    if cfg!(windows) {
        a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// A stored `./a/b` (or `.`) resolved inside `root`; `None` for anything else (an absolute
/// path, a plain relative one, or one that climbs out of `root` with `..`).
pub fn resolve(root: &Path, stored: &str) -> Option<PathBuf> {
    let rest = match stored {
        "." => "",
        s => s.strip_prefix("./").or_else(|| s.strip_prefix(".\\"))?,
    };
    let mut out = root.to_path_buf();
    for part in rest.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

/// What to write for `path`: relative when it is inside `root` (and there is one), else as it is.
pub fn store(root: Option<&Path>, path: &str) -> String {
    root.and_then(|r| relative_to(r, path).or_else(|| relative_resolved(r, path)))
        .unwrap_or_else(|| path.to_owned())
}

/// [`relative_to`] after both are resolved on disk: the same folder spelt two ways, through a
/// link (macOS's `/var` is `/private/var`) or a Windows short name (`C:\Users\RUNNER~1`), is
/// still inside. A path not made yet is resolved from its nearest existing folder.
fn relative_resolved(root: &Path, path: &str) -> Option<String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let root = std::fs::canonicalize(root).ok()?;
    let mut missing = Vec::new();
    let mut existing = path;
    let resolved = loop {
        if let Ok(p) = std::fs::canonicalize(existing) {
            break p;
        }
        missing.push(existing.file_name()?);
        existing = existing.parent()?;
    };
    let full = missing.iter().rev().fold(resolved, |p, part| p.join(part));
    relative_to(&root, full.to_str()?)
}

/// What a written `stored` means: resolved inside `root` when it is relative to it, else as it is.
pub fn load(root: Option<&Path>, stored: &str) -> String {
    root.and_then(|r| resolve(r, stored))
        .map_or_else(|| stored.to_owned(), |p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"D:\Stick\PaTorrent")
        } else {
            PathBuf::from("/media/stick/PaTorrent")
        }
    }

    fn inside(rest: &str) -> String {
        root().join(rest).to_string_lossy().into_owned()
    }

    #[test]
    fn inside_the_root_becomes_relative_and_back() {
        let r = root();
        assert_eq!(relative_to(&r, &inside("Downloads")), Some("./Downloads".into()));
        assert_eq!(
            relative_to(&r, &inside("Downloads/Linux ISOs")).as_deref(),
            Some("./Downloads/Linux ISOs")
        );
        assert_eq!(relative_to(&r, &r.to_string_lossy()), Some(".".into()));
        assert_eq!(resolve(&r, "./Downloads/Linux ISOs"), Some(r.join("Downloads").join("Linux ISOs")));
        assert_eq!(resolve(&r, "."), Some(r.clone()));
        // the folder moved: the same text resolves in its new place
        let moved = if cfg!(windows) {
            PathBuf::from(r"E:\PaTorrent")
        } else {
            PathBuf::from("/run/media/x/PaTorrent")
        };
        assert_eq!(resolve(&moved, "./Downloads"), Some(moved.join("Downloads")));
        for p in ["Downloads", "Downloads/a b", "."] {
            let full = load(Some(&r), &format!("./{p}"));
            assert_eq!(load(Some(&r), &store(Some(&r), &full)), full, "{p}");
        }
    }

    #[test]
    fn outside_stays_absolute() {
        let r = root();
        let elsewhere = if cfg!(windows) { r"C:\Users\x\Downloads" } else { "/home/x/Downloads" };
        assert_eq!(relative_to(&r, elsewhere), None);
        // a sibling whose name only starts like the root
        assert_eq!(relative_to(&r, &format!("{}2", r.to_string_lossy())), None);
        assert_eq!(store(Some(&r), elsewhere), elsewhere);
        assert_eq!(store(None, &inside("Downloads")), inside("Downloads"));
        assert_eq!(load(Some(&r), elsewhere), elsewhere);
        // relative, empty and climbing paths are not ours to resolve
        for s in ["Downloads", "", "./../x", "./a/../../x", "../x"] {
            assert_eq!(resolve(&r, s), None, "{s:?}");
        }
        assert_eq!(relative_to(&r, "relative/path"), None);
    }

    #[test]
    fn the_same_folder_spelt_another_way_is_inside() {
        let base = std::env::temp_dir().join(format!("portable-spelling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Stick").join("Downloads")).unwrap();
        // the root as the program finds itself (resolved: `\\?\C:\...`, `/private/var/...`)
        let r = std::fs::canonicalize(base.join("Stick")).unwrap();
        // and the paths as a user or a temp variable spells them; on Unix through a link too
        let spellings = [base.join("Stick"), base.join("Link")];
        #[cfg(unix)]
        std::os::unix::fs::symlink(base.join("Stick"), base.join("Link")).unwrap();
        #[cfg(not(unix))]
        let spellings = &spellings[..1];
        for s in spellings {
            let dl = s.join("Downloads").to_string_lossy().into_owned();
            assert_eq!(store(Some(&r), &dl), "./Downloads", "{dl}");
            // a folder not made yet
            let new = s.join("Downloads").join("new").to_string_lossy().into_owned();
            assert_eq!(store(Some(&r), &new), "./Downloads/new", "{new}");
        }
        let elsewhere = base.join("Other").to_string_lossy().into_owned();
        assert_eq!(store(Some(&r), &elsewhere), elsewhere);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn windows_forms() {
        let r = root();
        // case, and a UNC root
        assert_eq!(relative_to(&r, r"d:\stick\patorrent\DL"), Some("./DL".into()));
        let unc = Path::new(r"\\server\share\PaTorrent");
        assert_eq!(relative_to(unc, r"\\server\share\PaTorrent\Downloads"), Some("./Downloads".into()));
        assert_eq!(relative_to(unc, r"\\other\share\PaTorrent\Downloads"), None);
        assert_eq!(resolve(unc, "./Downloads"), Some(unc.join("Downloads")));
        // another drive
        assert_eq!(relative_to(&r, r"E:\Stick\PaTorrent\DL"), None);
    }
}
