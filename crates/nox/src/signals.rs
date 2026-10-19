// PaTorrent's own OS glue (no libtorrent counterpart: qBittorrent-nox does this in Qt).

//! Signals and console events, turned into requests the alert loop handles (PLAN.md Phase 6).
//!
//! - Unix: SIGINT and SIGTERM stop cleanly (resume data saved, port mappings deleted, as `q`
//!   does), SIGHUP reloads (the IP filter, the `--log` file), SIGPIPE is ignored. The handlers
//!   only set a bit and write a byte to a socket pair (the self-pipe trick); a thread reading
//!   the other end wakes the alert loop. `sigaction` is declared by hand (D4) on Linux and
//!   FreeBSD, where its layout is known and tested; other Unixes use `signal`, which has the
//!   same BSD semantics (handler kept, calls restarted) in their libcs.
//! - Windows: `SetConsoleCtrlHandler` takes Ctrl+C, Ctrl+Break, closing the console window,
//!   logoff and system shutdown. For the last three Windows ends the process when the handler
//!   returns, so the handler waits until the loop has shut the session down ([`finished`]).
//!   (A Windows service, a follow-up, would have to ignore logoff: it is sent to services for
//!   every user who logs off.)
//!
//! A second Ctrl+C / SIGINT / SIGTERM while the first one's shutdown is still running ends the
//! process at once, without saving.

use std::sync::atomic::{AtomicU32, Ordering};

/// What arrived since the last [`take`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Events {
    /// The first stop request's name ("SIGTERM", "Ctrl+C", ...).
    pub stop: Option<&'static str>,
    /// SIGHUP: reload what can be reloaded.
    pub reload: bool,
}

/// The stop requests, one bit each (`1 << index`).
#[cfg(unix)]
const STOP_NAMES: [&str; 2] = ["SIGINT", "SIGTERM"];
#[cfg(unix)]
const SIGINT_BIT: u32 = 1 << 0;
#[cfg(unix)]
const SIGTERM_BIT: u32 = 1 << 1;
#[cfg(windows)]
const STOP_NAMES: [&str; 5] = ["Ctrl+C", "Ctrl+Break", "console closed", "logoff", "system shutdown"];
#[cfg(windows)]
const CTRL_C_BIT: u32 = 1 << 0;
#[cfg(windows)]
const CTRL_BREAK_BIT: u32 = 1 << 1;
#[cfg(windows)]
const CLOSE_BIT: u32 = 1 << 2;
#[cfg(windows)]
const LOGOFF_BIT: u32 = 1 << 3;
#[cfg(windows)]
const SHUTDOWN_BIT: u32 = 1 << 4;
const RELOAD_BIT: u32 = 1 << 31;

/// Requests not taken yet.
static PENDING: AtomicU32 = AtomicU32::new(0);
/// Stop signals received so far: the second one exits at once.
static STOPS: AtomicU32 = AtomicU32::new(0);

/// Installs the handlers; `wake` runs (on another thread) after each request, to wake the
/// alert loop. Call once, before the session starts (after `--daemon`'s forks).
pub fn install(wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<()> {
    imp::install(Box::new(wake))
}

/// Takes the requests that arrived since the last call.
pub fn take() -> Events {
    let bits = PENDING.swap(0, Ordering::SeqCst);
    let stop = (0..STOP_NAMES.len()).find(|i| bits & (1 << i) != 0).map(|i| STOP_NAMES[i]);
    Events { stop, reload: bits & RELOAD_BIT != 0 }
}

/// The session is shut down and its state saved: a Windows close / logoff / shutdown handler
/// waiting for that may return (and Windows then ends the process).
pub fn finished() {
    imp::finished();
}

/// Counts a stop request; true for the second and later ones.
fn second_stop() -> bool {
    STOPS.fetch_add(1, Ordering::SeqCst) >= 1
}

#[cfg(unix)]
mod imp {
    use std::ffi::{c_int, c_void};
    use std::io::{self, Read};
    use std::os::fd::IntoRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::{PENDING, RELOAD_BIT, SIGINT_BIT, SIGTERM_BIT, second_stop};

    // the same numbers on Linux, the BSDs and macOS
    const SIGHUP: c_int = 1;
    const SIGINT: c_int = 2;
    const SIGPIPE: c_int = 13;
    const SIGTERM: c_int = 15;
    const SIG_IGN: usize = 1;

    /// The write end of the wake-up socket pair, for the handler.
    static WAKE_FD: AtomicI32 = AtomicI32::new(-1);
    static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

    /// glibc's and musl's `struct sigaction` (the 1024-bit `sigset_t` of both).
    #[cfg(target_os = "linux")]
    #[repr(C)]
    struct SigAction {
        handler: usize,
        mask: [u64; 16],
        flags: c_int,
        restorer: usize,
    }
    #[cfg(target_os = "linux")]
    const SA_RESTART: c_int = 0x1000_0000;

    /// FreeBSD's `struct sigaction` (`sigset_t` is four 32-bit words).
    #[cfg(target_os = "freebsd")]
    #[repr(C)]
    struct SigAction {
        handler: usize,
        flags: c_int,
        mask: [u32; 4],
    }
    #[cfg(target_os = "freebsd")]
    const SA_RESTART: c_int = 0x0002;

    unsafe extern "C" {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        fn sigaction(sig: c_int, act: *const SigAction, old: *mut SigAction) -> c_int;
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        fn signal(sig: c_int, handler: usize) -> usize;
        fn write(fd: c_int, buf: *const c_void, len: usize) -> isize;
        fn _exit(code: c_int) -> !;
        #[cfg_attr(target_os = "linux", link_name = "__errno_location")]
        #[cfg_attr(
            any(target_os = "freebsd", target_os = "dragonfly", target_vendor = "apple"),
            link_name = "__error"
        )]
        #[cfg_attr(
            any(target_os = "openbsd", target_os = "netbsd", target_os = "android"),
            link_name = "__errno"
        )]
        fn errno_location() -> *mut c_int;
    }

    /// Sets `sig`'s disposition (a handler's address, or `SIG_IGN`).
    fn set_handler(sig: c_int, handler: usize) -> io::Result<()> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            // SAFETY: zeroes are a valid `struct sigaction` (empty mask, no flags)
            let mut act: SigAction = unsafe { std::mem::zeroed() };
            act.handler = handler;
            act.flags = SA_RESTART;
            // SAFETY: a valid action, no old one asked for
            if unsafe { sigaction(sig, &act, std::ptr::null_mut()) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        {
            // SAFETY: `handler` is SIG_IGN or an `extern "C" fn(c_int)`
            if unsafe { signal(sig, handler) } == usize::MAX {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// The handler: async-signal-safe calls only (atomics, `write`, `_exit`), errno kept.
    extern "C" fn on_signal(sig: c_int) {
        // SAFETY: the calling thread's errno
        let errno = unsafe { errno_location() };
        // SAFETY: as above
        let saved = unsafe { *errno };
        let bit = match sig {
            SIGHUP => RELOAD_BIT,
            SIGINT => SIGINT_BIT,
            _ => SIGTERM_BIT,
        };
        if bit != RELOAD_BIT && second_stop() {
            // SAFETY: ends the process; nothing to clean up that a crash would not leave too
            unsafe { _exit(128 + sig) };
        }
        PENDING.fetch_or(bit, Ordering::SeqCst);
        let fd = WAKE_FD.load(Ordering::SeqCst);
        if fd >= 0 {
            // a full socket buffer drops the byte: a wake-up is pending anyway
            // SAFETY: one byte from a static buffer to our own non-blocking socket
            unsafe { write(fd, b"s".as_ptr().cast(), 1) };
        }
        // SAFETY: as above
        unsafe { *errno = saved };
    }

    pub(super) fn install(wake: Box<dyn Fn() + Send + Sync>) -> io::Result<()> {
        if WAKE.set(wake).is_err() {
            return Ok(());
        }
        let (mut rx, tx) = UnixStream::pair()?;
        tx.set_nonblocking(true)?;
        WAKE_FD.store(tx.into_raw_fd(), Ordering::SeqCst);
        std::thread::Builder::new().name("signals".into()).spawn(move || {
            let mut buf = [0u8; 64];
            loop {
                match rx.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => self::wake(),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        })?;
        let handler = on_signal as extern "C" fn(c_int) as usize;
        for sig in [SIGHUP, SIGINT, SIGTERM] {
            set_handler(sig, handler)?;
        }
        // std ignores it already; a write to a closed socket must stay an error, not a death
        set_handler(SIGPIPE, SIG_IGN)
    }

    fn wake() {
        if let Some(w) = WAKE.get() {
            w();
        }
    }

    pub(super) fn finished() {}
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::sync::atomic::Ordering;
    use std::sync::{Condvar, Mutex, OnceLock};
    use std::time::Duration;

    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
        SetConsoleCtrlHandler,
    };
    use windows_sys::core::BOOL;

    use super::{CLOSE_BIT, CTRL_BREAK_BIT, CTRL_C_BIT, LOGOFF_BIT, PENDING, SHUTDOWN_BIT, second_stop};

    static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
    /// Set by [`finished`]; close / logoff / shutdown handlers wait for it.
    static DONE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
    /// Longest a close / logoff / shutdown handler holds Windows off; it ends the process
    /// on its own timeout before that anyway.
    const FINISH_WAIT: Duration = Duration::from_secs(30);

    unsafe extern "system" fn on_event(kind: u32) -> BOOL {
        let bit = match kind {
            CTRL_C_EVENT => CTRL_C_BIT,
            CTRL_BREAK_EVENT => CTRL_BREAK_BIT,
            CTRL_CLOSE_EVENT => CLOSE_BIT,
            CTRL_LOGOFF_EVENT => LOGOFF_BIT,
            CTRL_SHUTDOWN_EVENT => SHUTDOWN_BIT,
            _ => return 0,
        };
        let interactive = matches!(kind, CTRL_C_EVENT | CTRL_BREAK_EVENT);
        if second_stop() && interactive {
            // the default handler: ExitProcess
            return 0;
        }
        PENDING.fetch_or(bit, Ordering::SeqCst);
        wake();
        if !interactive {
            // returning lets Windows end the process: first let the session save its state
            let (lock, cv) = &DONE;
            let done = lock.lock().unwrap_or_else(|e| e.into_inner());
            let _ = cv.wait_timeout_while(done, FINISH_WAIT, |d| !*d);
        }
        1
    }

    pub(super) fn install(wake: Box<dyn Fn() + Send + Sync>) -> io::Result<()> {
        if WAKE.set(wake).is_err() {
            return Ok(());
        }
        // SAFETY: a handler with the documented signature, alive for the whole process
        if unsafe { SetConsoleCtrlHandler(Some(on_event), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn wake() {
        if let Some(w) = WAKE.get() {
            w();
        }
    }

    pub(super) fn finished() {
        let (lock, cv) = &DONE;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn take_names_the_first_stop_and_clears() {
        PENDING.fetch_or(SIGTERM_BIT | RELOAD_BIT, Ordering::SeqCst);
        assert_eq!(take(), Events { stop: Some("SIGTERM"), reload: true });
        assert_eq!(take(), Events::default());
        PENDING.fetch_or(SIGINT_BIT | SIGTERM_BIT, Ordering::SeqCst);
        assert_eq!(take(), Events { stop: Some("SIGINT"), reload: false });
    }

    #[test]
    #[cfg(windows)]
    fn take_names_the_first_stop_and_clears() {
        PENDING.fetch_or(CLOSE_BIT | SHUTDOWN_BIT, Ordering::SeqCst);
        assert_eq!(take(), Events { stop: Some("console closed"), reload: false });
        assert_eq!(take(), Events::default());
        PENDING.fetch_or(CTRL_BREAK_BIT, Ordering::SeqCst);
        assert_eq!(take().stop, Some("Ctrl+Break"));
    }
}
