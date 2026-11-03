//! `patorrent.lock`: one process at a time in a settings or state folder (D42), whether the
//! second one runs on this computer, in another session, or on another computer that opened
//! the same folder over a share.
//!
//! On Windows the file is held open with no sharing at all, which SMB enforces for every
//! client of the share; on Unix it carries `flock(LOCK_EX | LOCK_NB)` (NFS turns it into a
//! lock its server keeps). Either way the operating system lets go when the process ends,
//! however it ends, so a crash never leaves a folder locked. The file itself stays: removing it
//! would let a process that opened the old one before the removal lock a file nobody else sees.

use std::fs::File;
use std::io;
use std::path::Path;

/// The file's name in the folder it guards.
pub const FILE_NAME: &str = "patorrent.lock";

/// A held lock; released when dropped (or when the process ends).
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

impl Lock {
    /// Locks `path`, creating it if needed. Fails with [`io::ErrorKind::WouldBlock`] (see
    /// [`is_taken`]) while another process holds it, with the open's own error otherwise.
    pub fn take(path: &Path) -> io::Result<Lock> {
        let mut o = File::options();
        o.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            // never through a symbolic link planted under the name
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600).custom_flags(super::private_file::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            o.share_mode(0);
        }
        let file = match o.open(path) {
            Ok(f) => f,
            // ERROR_SHARING_VIOLATION: another process has it open
            Err(e) if cfg!(windows) && e.raw_os_error() == Some(32) => return Err(taken()),
            Err(e) => return Err(e),
        };
        #[cfg(unix)]
        unix::try_lock(&file)?;
        Ok(Lock { _file: file })
    }
}

/// Whether [`Lock::take`] failed because another process holds the lock.
pub fn is_taken(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::WouldBlock
}

fn taken() -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, "in use by another process")
}

#[cfg(unix)]
mod unix {
    use std::ffi::c_int;
    use std::fs::File;
    use std::io;
    use std::os::fd::AsRawFd;

    // the same values on Linux, the BSDs and macOS
    const LOCK_EX: c_int = 2;
    const LOCK_NB: c_int = 4;

    unsafe extern "C" {
        fn flock(fd: c_int, op: c_int) -> c_int;
    }

    /// `EWOULDBLOCK` comes back as [`io::ErrorKind::WouldBlock`].
    pub(super) fn try_lock(f: &File) -> io::Result<()> {
        // SAFETY: an open file of ours
        if unsafe { flock(f.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time() {
        let dir = crate::disk::tests::temp_dir("lockfile");
        let path = dir.join(FILE_NAME);
        let first = Lock::take(&path).unwrap();
        // a second open is a second holder, in this process as in another one
        let e = Lock::take(&path).unwrap_err();
        assert!(is_taken(&e), "{e:?}");
        drop(first);
        let again = Lock::take(&path).unwrap();
        drop(again);
        // a folder that is not there is an ordinary error, not "taken"
        let e = Lock::take(&dir.join("none").join(FILE_NAME)).unwrap_err();
        assert!(!is_taken(&e), "{e:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
