//! Files and folders only their owner may read: the settings files that hold secrets (the
//! proxy password, the Web UI's password hash) and the folders that keep them and the torrent
//! library.
//!
//! On Unix a file is written with mode 0600 and a folder this code creates gets 0700, so other
//! local users cannot read them through a home directory that is 0755 (Debian's default). A
//! folder that already exists keeps its mode: it is the user's choice. On Windows both inherit
//! the parent's ACL, which in the profile is the user's own (and DPAPI seals the secrets).

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Creates `dir` and any missing parents, each readable by the owner only (Unix).
pub fn create_dir_all(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

/// The temporary file [`write`] goes through: `<name>.tmp` beside `path`.
pub fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Writes `data` to `path` through [`tmp_path`] and a rename, so a crash leaves the old file or
/// the new one, never half of one. On Unix the result is mode 0600 whatever the old file's
/// mode was (the rename puts the temporary file in its place), and a temporary file left
/// behind with a wider mode is narrowed before anything is written to it.
pub fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = tmp_path(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // never through a link planted under the temporary name
        options.mode(0o600).custom_flags(O_NOFOLLOW);
    }
    let mut f = options.open(&tmp)?;
    #[cfg(unix)]
    {
        // the mode above applies only to a new file
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(data)?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// `open`'s `O_NOFOLLOW` (fail on a symbolic link), without the `libc` crate: Linux has the
/// ARM value on ARM and PowerPC and the x86 one elsewhere; macOS and the BSDs share theirs.
#[cfg(unix)]
pub const O_NOFOLLOW: i32 = if cfg!(not(any(target_os = "linux", target_os = "android"))) {
    0x100
} else if cfg!(any(
    target_arch = "arm",
    target_arch = "aarch64",
    target_arch = "powerpc",
    target_arch = "powerpc64"
)) {
    0o100000
} else {
    0o400000
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_through_a_temporary_file() {
        let dir = crate::disk::tests::temp_dir("private-file");
        let path = dir.join("a").join("b").join("webui.json");
        create_dir_all(path.parent().unwrap()).unwrap();
        write(&path, b"one").unwrap();
        // a temporary file left by a crash is replaced
        std::fs::write(tmp_path(&path), b"stale and longer").unwrap();
        write(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(!tmp_path(&path).exists());
        assert_eq!(tmp_path(&path), dir.join("a").join("b").join("webui.json.tmp"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_may_read() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = crate::disk::tests::temp_dir("private-mode");
        let sub = dir.join("config").join("patorrent");
        create_dir_all(&sub).unwrap();
        assert_eq!(mode(&dir.join("config")), 0o700);
        assert_eq!(mode(&sub), 0o700);
        // an existing file, and a stale temporary file, both readable by everyone
        let path = sub.join("patorrent.ini");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(tmp_path(&path), b"stale").unwrap();
        std::fs::set_permissions(tmp_path(&path), std::fs::Permissions::from_mode(0o666)).unwrap();
        write(&path, b"lt.proxy_password=secret").unwrap();
        assert_eq!(mode(&path), 0o600);
        // a link planted under the temporary name is not followed
        let target = dir.join("elsewhere");
        std::os::unix::fs::symlink(&target, tmp_path(&path)).unwrap();
        assert!(write(&path, b"x").is_err());
        assert!(!target.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
