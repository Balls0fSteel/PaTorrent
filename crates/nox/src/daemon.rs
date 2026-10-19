// PaTorrent's own OS glue (no libtorrent counterpart).

//! Running as a service (PLAN.md Phase 6): the pid file, `--log FILE`, `--daemon` and systemd's
//! readiness notifications.
//!
//! - `--pid-file FILE` is written with our process id and locked for as long as we run
//!   (`flock` on Unix, a share mode that refuses writers on Windows), so a second instance with
//!   the same file stops with "already running". It is removed at a clean exit.
//! - `--log FILE` appends stdout and stderr to FILE; SIGHUP reopens it (for logrotate).
//! - `--daemon` (Unix) detaches the classic way: fork, `setsid`, fork again, `chdir /`, stdin
//!   from `/dev/null`. The command returns only once the daemon is up (session started, Web
//!   UI listening), with status 0, or 1 if it failed; until then its output still reaches the
//!   terminal (with `--log`, stdout goes to the log from the start and stderr from then on),
//!   afterwards it goes to the log or `/dev/null`.
//! - In the foreground under systemd (`Type=notify`), `NOTIFY_SOCKET` gets `READY=1` once the
//!   daemon is up, `RELOADING=1` / `READY=1` around a SIGHUP and `STOPPING=1` at shutdown
//!   (`sd_notify(3)`, written by hand: one datagram on a Unix socket).
//! - As a Windows service the same points set the status the Service Control Manager sees
//!   (`service`): `READY=1` is RUNNING, `STOPPING=1` STOP_PENDING.
//!
//! Unix calls are declared here by hand (D4: no libc crate).

use std::fs::File;
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};

/// The service options from the command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub daemon: bool,
    pub pid_file: Option<PathBuf>,
    pub log: Option<PathBuf>,
}

/// The process's service state, from [`Service::start`] to exit.
pub struct Service {
    pid_file: Option<PidFile>,
    log: Option<PathBuf>,
    /// The log file the standard handles point at (Windows keeps the handle itself).
    #[cfg_attr(unix, allow(dead_code))]
    log_file: Option<File>,
    /// `--daemon`: the socket the waiting parent reads "up" from.
    #[cfg(unix)]
    detached: Option<std::os::unix::net::UnixStream>,
}

impl Service {
    /// Locks the pid file, detaches with `--daemon`, sends output to the log and writes the
    /// pid. Call before anything starts a thread (`fork` keeps only the calling one).
    pub fn start(o: &Options) -> Result<Service, String> {
        let pid_file = o.pid_file.as_deref().map(PidFile::lock).transpose()?;
        let mut s = Service {
            pid_file,
            log: o.log.clone(),
            log_file: None,
            #[cfg(unix)]
            detached: None,
        };
        if o.daemon {
            #[cfg(unix)]
            {
                s.detached = Some(unix::detach().map_err(|e| format!("--daemon: {e}"))?);
                println!("* running in the background, pid {}", std::process::id());
            }
            #[cfg(not(unix))]
            return Err(
                "--daemon is for Unix (on Windows, install a service: patorrent-nox service install)".into(),
            );
        }
        if let Some(log) = &s.log {
            // while a detached daemon starts, its errors still go to the terminal
            let stderr_too = !o.daemon;
            s.log_file =
                Some(redirect(log, stderr_too).map_err(|e| format!("--log {}: {e}", log.display()))?);
        }
        if let Some(p) = &mut s.pid_file {
            p.write(std::process::id()).map_err(|e| format!("--pid-file {}: {e}", p.path.display()))?;
        }
        Ok(s)
    }

    /// The daemon is up: tells systemd and, with `--daemon`, the waiting parent (whose
    /// terminal stops getting our output now).
    pub fn ready(&mut self, status: &str) {
        notify(&format!("READY=1\nSTATUS={status}"));
        #[cfg(unix)]
        if let Some(mut parent) = self.detached.take() {
            let out = match &self.log {
                Some(log) => redirect(log, true),
                None => File::options()
                    .write(true)
                    .open("/dev/null")
                    .and_then(|f| unix::dup_stdio(&f).map(|()| f)),
            };
            if let Err(e) = out {
                eprintln!("error: {e}");
            }
            let _ = parent.write_all(b"R");
        }
    }

    /// SIGHUP: tells systemd a reload started and reopens the log file (a rotated log is
    /// started anew). Returns an error to print; the old log stays in use then.
    pub fn reloading(&mut self) -> Option<String> {
        let usec = monotonic_usec().map_or(String::new(), |t| format!("\nMONOTONIC_USEC={t}"));
        notify(&format!("RELOADING=1{usec}"));
        let log = self.log.as_ref()?;
        match redirect(log, true) {
            Ok(f) => {
                self.log_file = Some(f);
                None
            }
            Err(e) => Some(format!("--log {}: {e}", log.display())),
        }
    }

    /// The reload is done.
    pub fn reloaded(&self) {
        notify("READY=1");
    }

    /// The shutdown has started.
    pub fn stopping(&self) {
        notify("STOPPING=1");
    }
}

/// Appends stdout (and stderr) to `path`; returns the opened file.
fn redirect(path: &Path, stderr_too: bool) -> io::Result<File> {
    let mut o = File::options();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        // the log may show the Web UI's temporary password
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let f = o.open(path)?;
    io::stdout().flush()?;
    #[cfg(unix)]
    {
        unix::dup2_checked(&f, 1)?;
        if stderr_too {
            unix::dup2_checked(&f, 2)?;
        }
    }
    #[cfg(windows)]
    win::set_std_handles(&f, stderr_too)?;
    Ok(f)
}

/// A locked pid file; removed when dropped.
pub struct PidFile {
    file: Option<File>,
    path: PathBuf,
}

impl PidFile {
    /// Opens and locks `path`; fails with "already running" while another process holds it.
    pub fn lock(path: &Path) -> Result<PidFile, String> {
        let mut o = File::options();
        o.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o644);
        }
        #[cfg(windows)]
        {
            // others may read it, and we may delete it, but nobody else may open it to write
            use std::os::windows::fs::OpenOptionsExt;
            o.share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
        }
        let running = || {
            let pid =
                std::fs::read_to_string(path).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
            format!(
                "--pid-file {}: patorrent-nox is already running{}",
                path.display(),
                pid.map_or(String::new(), |p| format!(" (pid {p})"))
            )
        };
        let file = match o.open(path) {
            Ok(f) => f,
            // ERROR_SHARING_VIOLATION: another process has it open
            Err(e) if cfg!(windows) && e.raw_os_error() == Some(32) => return Err(running()),
            Err(e) => return Err(format!("--pid-file {}: {e}", path.display())),
        };
        #[cfg(unix)]
        if let Err(e) = unix::try_lock(&file) {
            return Err(if e.kind() == io::ErrorKind::WouldBlock {
                running()
            } else {
                format!("--pid-file {}: {e}", path.display())
            });
        }
        Ok(PidFile { file: Some(file), path: path.to_owned() })
    }

    /// Replaces the contents with `pid`.
    pub fn write(&mut self, pid: u32) -> io::Result<()> {
        let f = self.file.as_mut().expect("open until dropped");
        f.set_len(0)?;
        f.rewind()?;
        writeln!(f, "{pid}")?;
        f.flush()
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        // Unix: remove while still locked, so a new instance never locks the old file; Windows
        // cannot delete a file we hold open without delete sharing, so close first
        if cfg!(windows) {
            self.file = None;
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Sends `msg` to systemd's `NOTIFY_SOCKET` (`sd_notify`), if there is one.
#[cfg(unix)]
pub fn notify(msg: &str) -> bool {
    std::env::var_os("NOTIFY_SOCKET").is_some_and(|path| notify_to(&path, msg))
}

/// Sends `msg` to the notification socket `path` (a file system path, or `@name`).
#[cfg(unix)]
fn notify_to(path: &std::ffi::OsStr, msg: &str) -> bool {
    use std::os::unix::net::UnixDatagram;
    let Ok(sock) = UnixDatagram::unbound() else { return false };
    let r = match path.as_encoded_bytes().first() {
        Some(b'/') => sock.send_to(msg.as_bytes(), Path::new(path)),
        // an abstract socket (Linux): `@name`
        #[cfg(target_os = "linux")]
        Some(b'@') => {
            use std::os::linux::net::SocketAddrExt;
            std::os::unix::net::SocketAddr::from_abstract_name(&path.as_encoded_bytes()[1..])
                .and_then(|a| sock.send_to_addr(msg.as_bytes(), &a))
        }
        _ => return false,
    };
    r.is_ok()
}

#[cfg(windows)]
pub fn notify(msg: &str) -> bool {
    crate::service::notify(msg)
}

#[cfg(not(any(unix, windows)))]
pub fn notify(_msg: &str) -> bool {
    false
}

/// `CLOCK_MONOTONIC` in microseconds, which `RELOADING=1` carries (systemd's
/// `Type=notify-reload` wants it); Linux only, the one OS with systemd.
#[cfg(target_os = "linux")]
fn monotonic_usec() -> Option<u64> {
    use std::ffi::{c_int, c_long};
    #[repr(C)]
    struct Timespec {
        sec: c_long,
        nsec: c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: c_int, t: *mut Timespec) -> c_int;
    }
    const CLOCK_MONOTONIC: c_int = 1;
    let mut t = Timespec { sec: 0, nsec: 0 };
    // SAFETY: a valid out pointer
    (unsafe { clock_gettime(CLOCK_MONOTONIC, &mut t) } == 0)
        .then(|| t.sec as u64 * 1_000_000 + t.nsec as u64 / 1000)
}

#[cfg(not(target_os = "linux"))]
fn monotonic_usec() -> Option<u64> {
    None
}

#[cfg(unix)]
mod unix {
    use std::ffi::c_int;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    // the same values on Linux, the BSDs and macOS
    const LOCK_EX: c_int = 2;
    const LOCK_NB: c_int = 4;

    unsafe extern "C" {
        fn fork() -> c_int;
        fn setsid() -> c_int;
        fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
        fn dup2(old: c_int, new: c_int) -> c_int;
        fn flock(fd: c_int, op: c_int) -> c_int;
        fn _exit(code: c_int) -> !;
    }

    pub(super) fn try_lock(f: &File) -> io::Result<()> {
        // SAFETY: an open file of ours
        if unsafe { flock(f.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn dup2_checked(f: &File, to: c_int) -> io::Result<()> {
        // SAFETY: both descriptors are open; `to` is one of the standard ones
        if unsafe { dup2(f.as_raw_fd(), to) } < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// Points stdout and stderr at `f`.
    pub(super) fn dup_stdio(f: &File) -> io::Result<()> {
        io::stdout().flush()?;
        dup2_checked(f, 1)?;
        dup2_checked(f, 2)
    }

    /// Fork, `setsid`, fork: the grandchild returns with the socket to tell the original
    /// process "up" through; the original process waits for that and exits 0, or 1 when the
    /// socket closes first (the daemon failed to start and said why on stderr).
    pub(super) fn detach() -> io::Result<UnixStream> {
        let (mut rx, tx) = UnixStream::pair()?;
        io::stdout().flush()?;
        io::stderr().flush()?;
        // SAFETY: no other thread runs yet
        match unsafe { fork() } {
            -1 => return Err(io::Error::last_os_error()),
            0 => {}
            child => {
                drop(tx);
                let mut status = 0;
                // SAFETY: our child, which exits right after its own fork
                unsafe { waitpid(child, &mut status, 0) };
                let mut b = [0u8];
                let up = matches!(rx.read(&mut b), Ok(1));
                std::process::exit(if up { 0 } else { 1 });
            }
        }
        drop(rx);
        // a new session: no controlling terminal, no SIGHUP when the terminal closes
        // SAFETY: plain call
        if unsafe { setsid() } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: still one thread
        match unsafe { fork() } {
            -1 => return Err(io::Error::last_os_error()),
            0 => {}
            // the session leader goes, so the daemon can never reacquire a terminal
            // SAFETY: ends this intermediate process without running anything else
            _ => unsafe { _exit(0) },
        }
        std::env::set_current_dir("/")?;
        let null = File::options().read(true).write(true).open("/dev/null")?;
        dup2_checked(&null, 0)?;
        Ok(tx)
    }
}

#[cfg(windows)]
mod win {
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};

    /// Points the standard output (and error) handles at `f`; std looks the handle up on every
    /// write, so the redirect takes effect at once. The caller keeps `f` open.
    pub(super) fn set_std_handles(f: &File, stderr_too: bool) -> io::Result<()> {
        let h = f.as_raw_handle();
        // SAFETY: a valid handle, kept open by the caller while it is the standard handle
        if unsafe { SetStdHandle(STD_OUTPUT_HANDLE, h) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: as above
        if stderr_too && unsafe { SetStdHandle(STD_ERROR_HANDLE, h) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nox-daemon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn pid_file_is_locked_while_held_and_removed_after() {
        let d = temp("pid");
        let path = d.join("nox.pid");
        let mut a = PidFile::lock(&path).unwrap();
        a.write(4242).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "4242\n");
        // a second instance (another open file, even in this process) is refused
        let e = PidFile::lock(&path).err().expect("locked");
        assert!(e.contains("already running (pid 4242)"), "{e}");
        // a shorter pid replaces the whole contents
        a.write(7).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "7\n");
        drop(a);
        assert!(!path.exists());
        // free again; a stale file left by a crash is taken over
        std::fs::write(&path, "99999\n").unwrap();
        let mut b = PidFile::lock(&path).unwrap();
        b.write(5).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "5\n");
        drop(b);
        // a pid file in a folder that does not exist is an error, not a panic
        assert!(PidFile::lock(&d.join("none").join("x.pid")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn notify_sends_a_datagram() {
        use std::os::unix::net::UnixDatagram;
        let d = temp("notify");
        let sock = d.join("notify.sock");
        let rx = UnixDatagram::bind(&sock).unwrap();
        assert!(notify_to(sock.as_os_str(), "READY=1\nSTATUS=up"));
        let mut buf = [0u8; 64];
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1\nSTATUS=up");
        // no listener, a relative path
        assert!(!notify_to(d.join("none.sock").as_os_str(), "READY=1"));
        assert!(!notify_to("relative.sock".as_ref(), "READY=1"));
        #[cfg(target_os = "linux")]
        {
            let name = format!("nox-notify-test-{}", std::process::id());
            use std::os::linux::net::SocketAddrExt;
            let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
            let rx = UnixDatagram::bind_addr(&addr).unwrap();
            assert!(notify_to(format!("@{name}").as_ref(), "STOPPING=1"));
            let n = rx.recv(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"STOPPING=1");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}
