//! Which folders the Web UI may name (SEC-05). A Web UI login can save, move and delete files as
//! the daemon's account, so every path a request supplies is checked here first:
//!
//! - always: rooted, no `..`, and on Windows a plain drive path (no `\\server\share`, `\\?\`
//!   or `\\.\` device paths, which would also hand the account's NTLM hash to a remote host,
//!   and no `:` alternate data streams);
//! - with `--webui-save-root` given: inside one of those folders. The roots come from the
//!   command line only, so a Web UI user cannot widen them.
//!
//! The comparison is lexical: a root should not contain links that other users can change.

use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct PathPolicy {
    roots: Vec<PathBuf>,
}

impl PathPolicy {
    pub fn new(roots: Vec<PathBuf>) -> PathPolicy {
        PathPolicy { roots }
    }

    /// `Err` with the reason when a request may not use `path`.
    pub fn check(&self, path: &str) -> Result<(), &'static str> {
        let path = Path::new(path.trim());
        shape(path)?;
        if self.roots.is_empty() || self.roots.iter().any(|root| within(path, root)) {
            Ok(())
        } else {
            Err("path is outside the folders the Web UI may use")
        }
    }
}

fn shape(path: &Path) -> Result<(), &'static str> {
    // rooted, not merely absolute: on Windows `\dl` (the current drive) is local and fine,
    // while `C:dl` (relative to that drive's current folder) is not
    if !path.has_root() {
        return Err("path must be absolute");
    }
    for c in path.components() {
        match c {
            #[cfg(windows)]
            Component::Prefix(p) if !matches!(p.kind(), std::path::Prefix::Disk(_)) => {
                return Err("network, device and \\\\?\\ paths are not allowed");
            }
            Component::ParentDir => return Err("path must not contain .."),
            #[cfg(windows)]
            Component::Normal(n) if n.to_string_lossy().contains(':') => {
                return Err("path must not contain ':'");
            }
            _ => {}
        }
    }
    Ok(())
}

/// `path` is `root` or below it.
fn within(path: &Path, root: &Path) -> bool {
    let mut p = path.components();
    for r in root.components() {
        match p.next() {
            Some(c) if same(c, r) => {}
            _ => return false,
        }
    }
    true
}

fn same(a: Component<'_>, b: Component<'_>) -> bool {
    if cfg!(windows) {
        a.as_os_str().to_string_lossy().eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    const ROOT: &str = r"D:\Media";
    #[cfg(not(windows))]
    const ROOT: &str = "/srv/media";

    fn p(s: &str) -> String {
        if cfg!(windows) { s.replace("/srv/media", ROOT).replace('/', "\\") } else { s.to_owned() }
    }

    #[test]
    fn unsafe_shapes_are_refused_without_roots() {
        let open = PathPolicy::default();
        assert!(open.check(&p("/srv/media/films")).is_ok());
        assert!(open.check("relative/dir").is_err());
        assert!(open.check(&p("/srv/media/../../etc")).is_err());
        assert!(open.check("").is_err());
        // the current drive's root folder is local
        assert!(open.check("/srv/tv").is_ok());
        #[cfg(windows)]
        for bad in [
            r"\\attacker\share\x",
            r"//attacker/share/x",
            r"\\?\C:\Windows",
            r"\\.\PhysicalDrive0",
            r"C:relative",
            r"C:\dl\file.txt:stream",
        ] {
            assert!(open.check(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn roots_limit_paths() {
        let policy = PathPolicy::new(vec![PathBuf::from(ROOT)]);
        assert!(policy.check(&p("/srv/media")).is_ok());
        assert!(policy.check(&p("/srv/media/tv/show")).is_ok());
        assert!(policy.check(&p("/srv/mediaX")).is_err());
        assert!(policy.check(&p("/srv")).is_err());
        assert!(policy.check(&p("/srv/media/../secret")).is_err());
        #[cfg(windows)]
        {
            assert!(policy.check(r"d:\media\TV").is_ok());
            assert!(
                policy
                    .check(r"C:\Users\x\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup")
                    .is_err()
            );
        }
        #[cfg(not(windows))]
        assert!(policy.check("/home/x/.config/autostart").is_err());
    }
}
