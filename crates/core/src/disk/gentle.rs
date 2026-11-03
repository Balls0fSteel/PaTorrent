//! Gentle checks (DISKIO.md part 1, D48): a low I/O priority for the checking thread, and hints
//! that keep a check's reads from pushing other programs' data out of the OS file cache. Each
//! is best effort and a no-op where the platform has no such tool.

use std::fs::File;
use std::marker::PhantomData;

/// Lowers the calling thread's I/O priority until the guard drops (then normal again).
/// Windows: background mode (`THREAD_MODE_BACKGROUND_BEGIN`, which also lowers the memory
/// priority of the pages it reads). Linux: the idle I/O class (`ioprio_set`), the previous one
/// restored on drop. macOS: `IOPOL_THROTTLE`, restored on drop. Elsewhere a no-op. Failures are
/// ignored, but `active()` says whether it took. Bound to its thread (not `Send`).
pub(crate) struct BackgroundIo {
    active: bool,
    /// What to restore on drop (Linux: the ioprio; macOS: the I/O policy).
    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    previous: i32,
    _thread: PhantomData<*const ()>,
}

impl BackgroundIo {
    pub(crate) fn begin() -> BackgroundIo {
        let (active, previous) = imp::begin();
        BackgroundIo { active, previous, _thread: PhantomData }
    }

    /// Whether the priority was lowered: false when the platform has no such call, it failed, or
    /// (Windows) the thread was already in background mode, which then stays as it was.
    pub(crate) fn active(&self) -> bool {
        self.active
    }
}

impl Drop for BackgroundIo {
    fn drop(&mut self) {
        if self.active {
            imp::end(self.previous);
        }
    }
}

/// A file a check reads once from start to end. Linux and FreeBSD: `POSIX_FADV_SEQUENTIAL`
/// (more read-ahead). macOS: `F_RDAHEAD` and `F_NOCACHE`. Windows: nothing (background mode
/// already puts the pages on the low-priority standby list).
pub(crate) fn advise_sequential(f: &File) {
    imp::advise_sequential(f);
}

/// The check is done with `len` bytes at `offset`. Linux and FreeBSD: `POSIX_FADV_DONTNEED` on
/// that range, so the pages leave the cache first. Else nothing.
pub(crate) fn advise_done(f: &File, offset: u64, len: u64) {
    imp::advise_done(f, offset, len);
}

#[cfg(windows)]
mod imp {
    use std::fs::File;
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, THREAD_MODE_BACKGROUND_END,
    };

    /// Fails (and leaves the mode alone) when the thread is already in background mode, so a
    /// nested guard does not end the outer one's mode.
    pub(super) fn begin() -> (bool, i32) {
        // SAFETY: the pseudo-handle of the calling thread.
        let ok = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) } != 0;
        (ok, 0)
    }

    pub(super) fn end(_previous: i32) {
        // SAFETY: as above; the guard is not `Send`, so this is the thread that began it.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END) };
    }

    pub(super) fn advise_sequential(_f: &File) {}

    pub(super) fn advise_done(_f: &File, _offset: u64, _len: u64) {}
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::{c_int, c_long};
    use std::fs::File;

    // ioprio_set/ioprio_get through syscall(): glibc has no wrappers.
    #[cfg(target_arch = "x86_64")]
    const IOPRIO_CALLS: Option<(c_long, c_long)> = Some((251, 252));
    #[cfg(any(target_arch = "aarch64", target_arch = "riscv64", target_arch = "loongarch64"))]
    const IOPRIO_CALLS: Option<(c_long, c_long)> = Some((30, 31));
    #[cfg(target_arch = "x86")]
    const IOPRIO_CALLS: Option<(c_long, c_long)> = Some((289, 290));
    #[cfg(target_arch = "arm")]
    const IOPRIO_CALLS: Option<(c_long, c_long)> = Some((314, 315));
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "loongarch64",
        target_arch = "x86",
        target_arch = "arm"
    )))]
    const IOPRIO_CALLS: Option<(c_long, c_long)> = None;

    const IOPRIO_WHO_PROCESS: c_long = 1;
    /// `IOPRIO_PRIO_VALUE(IOPRIO_CLASS_IDLE, 0)`.
    const IOPRIO_IDLE: c_long = 3 << 13;

    unsafe extern "C" {
        fn syscall(number: c_long, ...) -> c_long;
        #[cfg(target_pointer_width = "64")]
        fn posix_fadvise(fd: c_int, offset: i64, len: i64, advice: c_int) -> c_int;
    }

    /// `ioprio_set(IOPRIO_WHO_PROCESS, 0, value)`: who 0 is the calling thread.
    fn set(value: c_long) -> bool {
        let Some((set, _)) = IOPRIO_CALLS else { return false };
        // SAFETY: a plain syscall with integer arguments.
        unsafe { syscall(set, IOPRIO_WHO_PROCESS, 0 as c_long, value) == 0 }
    }

    pub(super) fn begin() -> (bool, i32) {
        let Some((_, get)) = IOPRIO_CALLS else { return (false, 0) };
        // SAFETY: as above.
        let previous = unsafe { syscall(get, IOPRIO_WHO_PROCESS, 0 as c_long) };
        let ok = previous >= 0 && set(IOPRIO_IDLE);
        (ok, previous as i32)
    }

    pub(super) fn end(previous: i32) {
        // 0 (IOPRIO_CLASS_NONE: follow the CPU nice level) when the old value is refused.
        if !set(previous as c_long) {
            set(0);
        }
    }

    /// The calling thread's ioprio.
    #[cfg(test)]
    pub(super) fn current() -> Option<c_long> {
        let (_, get) = IOPRIO_CALLS?;
        // SAFETY: a plain syscall with integer arguments.
        let p = unsafe { syscall(get, IOPRIO_WHO_PROCESS, 0 as c_long) };
        (p >= 0).then_some(p)
    }

    #[cfg(target_pointer_width = "64")]
    fn fadvise(f: &File, offset: u64, len: u64, advice: c_int) {
        use std::os::fd::AsRawFd;
        let (Ok(offset), Ok(len)) = (i64::try_from(offset), i64::try_from(len)) else { return };
        // SAFETY: an open descriptor; the call only gives the kernel a hint.
        unsafe { posix_fadvise(f.as_raw_fd(), offset, len, advice) };
    }

    #[cfg(not(target_pointer_width = "64"))]
    fn fadvise(_f: &File, _offset: u64, _len: u64, _advice: c_int) {}

    const POSIX_FADV_SEQUENTIAL: c_int = 2;
    #[cfg(target_arch = "s390x")]
    const POSIX_FADV_DONTNEED: c_int = 6;
    #[cfg(not(target_arch = "s390x"))]
    const POSIX_FADV_DONTNEED: c_int = 4;

    pub(super) fn advise_sequential(f: &File) {
        fadvise(f, 0, 0, POSIX_FADV_SEQUENTIAL);
    }

    pub(super) fn advise_done(f: &File, offset: u64, len: u64) {
        // Length 0 would mean "to the end of the file".
        if len > 0 {
            fadvise(f, offset, len, POSIX_FADV_DONTNEED);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_int;
    use std::fs::File;
    use std::os::fd::AsRawFd;

    const IOPOL_TYPE_DISK: c_int = 0;
    const IOPOL_SCOPE_THREAD: c_int = 1;
    const IOPOL_DEFAULT: c_int = 0;
    const IOPOL_THROTTLE: c_int = 3;
    const F_RDAHEAD: c_int = 45;
    const F_NOCACHE: c_int = 48;

    unsafe extern "C" {
        fn getiopolicy_np(iotype: c_int, scope: c_int) -> c_int;
        fn setiopolicy_np(iotype: c_int, scope: c_int, policy: c_int) -> c_int;
        fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    }

    pub(super) fn begin() -> (bool, i32) {
        // SAFETY: plain calls on the calling thread's policy.
        unsafe {
            let previous = getiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_THREAD);
            let ok = setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_THREAD, IOPOL_THROTTLE) == 0;
            (ok, if previous < 0 { IOPOL_DEFAULT } else { previous })
        }
    }

    pub(super) fn end(previous: i32) {
        // SAFETY: as above.
        unsafe { setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_THREAD, previous) };
    }

    pub(super) fn advise_sequential(f: &File) {
        let fd = f.as_raw_fd();
        // SAFETY: an open descriptor; both commands take an int.
        unsafe {
            fcntl(fd, F_RDAHEAD, 1 as c_int);
            fcntl(fd, F_NOCACHE, 1 as c_int);
        }
    }

    pub(super) fn advise_done(_f: &File, _offset: u64, _len: u64) {}
}

#[cfg(target_os = "freebsd")]
mod imp {
    use std::ffi::c_int;
    use std::fs::File;
    use std::os::fd::AsRawFd;

    const POSIX_FADV_SEQUENTIAL: c_int = 2;
    const POSIX_FADV_DONTNEED: c_int = 4;

    unsafe extern "C" {
        fn posix_fadvise(fd: c_int, offset: i64, len: i64, advice: c_int) -> c_int;
    }

    fn fadvise(f: &File, offset: u64, len: u64, advice: c_int) {
        let (Ok(offset), Ok(len)) = (i64::try_from(offset), i64::try_from(len)) else { return };
        // SAFETY: an open descriptor; the call only gives the kernel a hint.
        unsafe { posix_fadvise(f.as_raw_fd(), offset, len, advice) };
    }

    /// FreeBSD has no per-thread I/O priority.
    pub(super) fn begin() -> (bool, i32) {
        (false, 0)
    }

    pub(super) fn end(_previous: i32) {}

    pub(super) fn advise_sequential(f: &File) {
        fadvise(f, 0, 0, POSIX_FADV_SEQUENTIAL);
    }

    pub(super) fn advise_done(f: &File, offset: u64, len: u64) {
        if len > 0 {
            fadvise(f, offset, len, POSIX_FADV_DONTNEED);
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos", target_os = "freebsd")))]
mod imp {
    use std::fs::File;

    pub(super) fn begin() -> (bool, i32) {
        (false, 0)
    }

    pub(super) fn end(_previous: i32) {}

    pub(super) fn advise_sequential(_f: &File) {}

    pub(super) fn advise_done(_f: &File, _offset: u64, _len: u64) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn background_io_begins_and_ends() {
        let outer = BackgroundIo::begin();
        if cfg!(any(windows, target_os = "macos")) {
            assert!(outer.active(), "background I/O did not take");
        }
        // A nested guard must not end the outer one's mode early, and dropping both restores.
        let inner = BackgroundIo::begin();
        if cfg!(windows) {
            assert!(!inner.active(), "Windows refuses a second THREAD_MODE_BACKGROUND_BEGIN");
        }
        let outer_took = outer.active();
        drop(inner);
        drop(outer);
        // Normal again: a fresh guard takes as the first did.
        assert_eq!(BackgroundIo::begin().active(), outer_took);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_ioprio_is_idle_then_restored() {
        let Some(before) = imp::current() else { return };
        let g = BackgroundIo::begin();
        if g.active() {
            assert_eq!(imp::current().map(|p| p >> 13), Some(3), "not the idle class");
        }
        drop(g);
        let after = imp::current().unwrap_or(0);
        assert!(after >> 13 != 3 || before >> 13 == 3, "the idle class was left behind: {after:#x}");
    }

    #[test]
    fn advice_on_a_real_file_is_harmless() {
        let dir = std::env::temp_dir().join(format!("patorrent-gentle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.bin");
        std::fs::File::create(&path).unwrap().write_all(&[7u8; 100_000]).unwrap();
        let mut f = std::fs::File::open(&path).unwrap();
        advise_sequential(&f);
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        advise_done(&f, 0, buf.len() as u64);
        advise_done(&f, 0, 0);
        advise_done(&f, u64::MAX, u64::MAX);
        assert_eq!(buf.len(), 100_000);
        drop(f);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
