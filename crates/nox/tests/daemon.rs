//! `patorrent-nox run` as a service. Unix: SIGTERM and SIGINT stop it cleanly, SIGHUP reloads,
//! systemd's `NOTIFY_SOCKET` hears `READY=1`, `RELOADING=1` and `STOPPING=1`, and `--daemon`
//! with `--pid-file` and `--log` detaches, refuses a second instance, reopens the log on SIGHUP
//! and cleans up on SIGTERM. Windows: Ctrl+Break stops it cleanly and removes the pid file.
//! The sessions listen on loopback only, so no port is mapped on the real router.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nox-daemon-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `patorrent-nox run` with nothing saved, listening on loopback only.
fn nox(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_patorrent-nox"));
    c.args(["run", "--no-state", "--listen", "127.0.0.1:0"]).current_dir(dir).stdin(Stdio::null());
    c.env_remove("NOTIFY_SOCKET");
    c
}

fn wait_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        assert!(Instant::now() < deadline, "patorrent-nox did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::c_int;
    use std::os::unix::net::UnixDatagram;

    use super::*;

    unsafe extern "C" {
        fn kill(pid: c_int, sig: c_int) -> c_int;
    }

    const SIGHUP: c_int = 1;
    const SIGINT: c_int = 2;
    const SIGTERM: c_int = 15;

    fn signal(pid: u32, sig: c_int) {
        // SAFETY: plain call
        assert_eq!(unsafe { kill(pid as c_int, sig) }, 0, "kill {pid} {sig}");
    }

    fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The rest of the child's stdout, once it exited.
    fn rest_of_stdout(child: &mut Child) -> String {
        let mut out = String::new();
        child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
        out
    }

    fn recv(sock: &UnixDatagram) -> String {
        let mut buf = [0u8; 512];
        let n = sock.recv(&mut buf).expect("a notification");
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    #[test]
    fn signals_stop_cleanly_and_notify_systemd() {
        for (sig, name) in [(SIGTERM, "SIGTERM"), (SIGINT, "SIGINT")] {
            let dir = temp_dir(&format!("sig{sig}"));
            let sock_path = dir.join("notify.sock");
            let sock = UnixDatagram::bind(&sock_path).unwrap();
            sock.set_read_timeout(Some(TIMEOUT)).unwrap();
            let mut child =
                nox(&dir).env("NOTIFY_SOCKET", &sock_path).stdout(Stdio::piped()).spawn().unwrap();
            let pid = child.id();

            assert_eq!(recv(&sock), "READY=1\nSTATUS=running");
            signal(pid, SIGHUP);
            let reloading = recv(&sock);
            assert!(reloading.starts_with("RELOADING=1"), "{reloading:?}");
            if cfg!(target_os = "linux") {
                assert!(reloading.contains("\nMONOTONIC_USEC="), "{reloading:?}");
            }
            assert_eq!(recv(&sock), "READY=1");
            signal(pid, sig);
            assert_eq!(recv(&sock), "STOPPING=1");
            let status = wait_exit(&mut child);
            let out = rest_of_stdout(&mut child);
            assert!(status.success(), "{name}: {status:?}\n{out}");
            for line in ["* SIGHUP: reloading", &format!("* {name} received"), "* shutting down", "* stopped"]
            {
                assert!(out.contains(line), "{name}: no {line:?} in\n{out}");
            }
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn daemon_mode_with_pid_file_and_log() {
        let dir = temp_dir("bg");
        let (pid_file, log) = (dir.join("nox.pid"), dir.join("nox.log"));
        // relative paths: the daemon makes them absolute before it changes to /
        let start =
            || nox(&dir).args(["--daemon", "--pid-file", "nox.pid", "--log", "nox.log"]).output().unwrap();
        let first = start();
        let stdout = String::from_utf8_lossy(&first.stdout).into_owned();
        assert!(first.status.success(), "{first:?}");
        // the command returned only once the daemon was up
        let pid: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        assert!(stdout.contains(&format!("* running in the background, pid {pid}")), "{stdout}");
        // SAFETY: plain call; signal 0 only checks that the process exists
        assert_eq!(unsafe { kill(pid as c_int, 0) }, 0);
        #[cfg(target_os = "linux")]
        assert_eq!(std::fs::read_link(format!("/proc/{pid}/cwd")).unwrap(), Path::new("/"));

        // a second instance with the same pid file refuses to start
        let second = start();
        assert_eq!(second.status.code(), Some(1), "{second:?}");
        let err = String::from_utf8_lossy(&second.stderr);
        assert!(err.contains(&format!("already running (pid {pid})")), "{err}");

        // logrotate: move the log away, SIGHUP, and the daemon writes a new one
        let rotated = dir.join("nox.log.1");
        std::fs::rename(&log, &rotated).unwrap();
        signal(pid, SIGHUP);
        wait_for("the reopened log", || {
            std::fs::read_to_string(&log).is_ok_and(|s| s.contains("* SIGHUP: reloading"))
        });
        let old = std::fs::read_to_string(&rotated).unwrap();
        assert!(!old.contains("SIGHUP"), "{old}");

        signal(pid, SIGTERM);
        wait_for("the pid file to go", || !pid_file.exists());
        let log = std::fs::read_to_string(&log).unwrap();
        for line in ["* SIGTERM received", "* shutting down", "* stopped"] {
            assert!(log.contains(line), "no {line:?} in\n{log}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(windows)]
mod windows {
    use std::io::{BufRead, BufReader};
    use std::os::windows::process::CommandExt;

    use super::*;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
    }

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
    const CTRL_BREAK_EVENT: u32 = 1;

    /// Ctrl+Break, sent to the child's own process group (Ctrl+C cannot be sent to one group;
    /// both go through the same handler). Needs a console to share with the child: without one
    /// (some CI runners) the test has nothing to send through and passes with a note.
    #[test]
    fn ctrl_break_stops_cleanly() {
        let dir = temp_dir("break");
        let pid_file = dir.join("nox.pid");
        let mut child = nox(&dir)
            .args(["--pid-file", "nox.pid"])
            .stdout(Stdio::piped())
            .creation_flags(CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .unwrap();
        // up once the first status line comes ("[   0s] 0 torrents | ...")
        let mut lines = BufReader::new(child.stdout.take().unwrap());
        let mut seen = String::new();
        loop {
            let mut line = String::new();
            assert!(lines.read_line(&mut line).unwrap() > 0, "no status line:\n{seen}");
            seen.push_str(&line);
            if line.starts_with('[') {
                break;
            }
        }
        assert!(pid_file.exists());
        // SAFETY: plain call
        if unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) } == 0 {
            eprintln!("no console to send Ctrl+Break through: {}", std::io::Error::last_os_error());
            child.kill().unwrap();
            child.wait().unwrap();
            return;
        }
        let status = wait_exit(&mut child);
        lines.read_to_string(&mut seen).unwrap();
        assert!(status.success(), "{status:?}\n{seen}");
        for line in ["* Ctrl+Break received", "* shutting down", "* stopped"] {
            assert!(seen.contains(line), "no {line:?} in\n{seen}");
        }
        assert!(!pid_file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
