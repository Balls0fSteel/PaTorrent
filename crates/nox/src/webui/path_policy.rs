//! Which folders the Web UI may name (SEC-05). A Web UI login can save, move and delete files as
//! the daemon's account, so every path a request supplies is checked here first:
//!
//! - always: rooted, no `..`, and on Windows a plain drive path (no `\\server\share`, `\\?\`
//!   or `\\.\` device paths, which would also hand the account's NTLM hash to a remote host,
//!   and no `:` alternate data streams);
//! - with `--webui-save-root` given: inside one of those folders. The roots come from the
//!   command line only, so a Web UI user cannot widen them.
//!
//! A path inside a root by name must also be inside it on disk: its deepest existing folder,
//! links and junctions followed, must be inside the root's (so a link in a root does not lead
//! out of it). A link made after the check is not caught: a root should not contain folders
//! that other users can change.

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
        const OUTSIDE: &str = "path is outside the folders the Web UI may use";
        let path = Path::new(path.trim());
        shape(path)?;
        if self.roots.is_empty() {
            return Ok(());
        }
        if !self.roots.iter().any(|root| within(path, root)) {
            return Err(OUTSIDE);
        }
        let real = resolve(path);
        if self.roots.iter().any(|root| within(&real, &resolve(root))) { Ok(()) } else { Err(OUTSIDE) }
    }
}

/// `path` as it is on disk: its deepest existing ancestor canonicalized (links and junctions
/// followed), then the rest, which does not exist yet, as it was given. Unchanged when nothing
/// of it exists.
fn resolve(path: &Path) -> PathBuf {
    let mut base = path;
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(base) {
            return rest.iter().rev().fold(real, |p, c| p.join(c));
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                base = parent;
            }
            _ => return path.to_owned(),
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

    /// A link (a junction on Windows) to `target` at `link`; false when none can be made here.
    fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, link).is_ok();
        #[cfg(windows)]
        return std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success());
        #[cfg(not(any(unix, windows)))]
        false
    }

    /// SEC-05: a path inside a root by name is refused when a link or junction in the root
    /// leads out of it, also for folders below the link that do not exist yet; a root that is
    /// itself a link still takes the paths below it.
    #[test]
    fn links_inside_a_root_are_followed() {
        let base = std::env::temp_dir().join(format!("nox-path-policy-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, outside) = (base.join("root"), base.join("outside"));
        std::fs::create_dir_all(root.join("films")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let policy = PathPolicy::new(vec![root.clone()]);
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        assert!(policy.check(&s(root.join("films"))).is_ok());
        assert!(policy.check(&s(root.join("new").join("deeper"))).is_ok());
        if !link_dir(&outside, &root.join("out")) {
            eprintln!("no link could be made here; links are not tested");
            return;
        }
        assert!(policy.check(&s(root.join("out"))).is_err());
        assert!(policy.check(&s(root.join("out").join("new").join("deeper"))).is_err());
        assert!(policy.check(&s(root.join("films").join("x"))).is_ok());
        let alias = base.join("alias");
        assert!(link_dir(&root, &alias));
        let through = PathPolicy::new(vec![alias.clone()]);
        assert!(through.check(&s(alias.join("films"))).is_ok());
        assert!(through.check(&s(alias.join("out"))).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
