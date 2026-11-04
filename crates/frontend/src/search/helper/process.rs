//! Running the search helper: started when a search first needs it, stopped after
//! [`IDLE`] without one and at exit. [`Runner`] owns the child process; how to start it is a
//! [`Launch`] its launcher builds for the port picked, so the tests start a stand-in of ours
//! instead of Jackett.
//!
//! A start: a free loopback port (the last one when it is still free), the launcher's command
//! (no console window on Windows, `CREATE_NO_WINDOW`; no inherited output), then `GET /health`
//! every [`POLL`] until it answers. A child that exits first, or does not answer within the
//! [`Runner::start_timeout`], is killed and the start fails. A stop asks the helper to shut
//! down through its API, waits a little, then kills it.

use std::ffi::OsString;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::api;

/// Stopped after this long without a search.
pub const IDLE: Duration = Duration::from_secs(10 * 60);
/// Between health checks while it starts.
pub const POLL: Duration = Duration::from_millis(250);
/// How long a start may take (a .NET program's first start on a slow disk is slow).
pub const START_TIMEOUT: Duration = Duration::from_secs(90);

/// How to start the helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(OsString, OsString)>,
    /// For the clean stop ([`api::shutdown`]); empty: killed straight away.
    pub api_key: String,
}

/// Builds the [`Launch`] for a port (writing the helper's settings for it first).
pub type Launcher = Box<dyn FnMut(u16) -> Result<Launch, String> + Send>;

struct Running {
    child: Child,
    port: u16,
    api_key: String,
    last_use: Instant,
}

/// The helper's process; see the module documentation.
pub struct Runner {
    launcher: Launcher,
    running: Option<Running>,
    /// The port to try first (the last one used).
    pub preferred_port: u16,
    pub start_timeout: Duration,
    pub idle: Duration,
    /// Set: a start under way gives up (the program exits).
    pub cancel: Option<&'static AtomicBool>,
}

/// A free port on `127.0.0.1`: `preferred` when it is free, else one the system picks.
pub fn free_port(preferred: u16) -> Result<u16, String> {
    if preferred != 0 && TcpListener::bind((Ipv4Addr::LOCALHOST, preferred)).is_ok() {
        return Ok(preferred);
    }
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

/// The command for a launch: no window of its own, no output inherited, no input.
pub fn command(l: &Launch) -> Command {
    let mut c = Command::new(&l.program);
    c.args(&l.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(dir) = &l.cwd {
        c.current_dir(dir);
    }
    for (k, v) in &l.env {
        c.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// The helper dies with us, however we die (a crash, a kill from Task Manager): on Windows every
/// helper started is put into one job object with "kill on job close", whose only handle is
/// ours and is closed by the system when our process ends. Elsewhere the normal exit kills it
/// ([`super::shutdown`]); a crash can leave it running until its next start.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    /// The job's handle, never closed (that is what kills the helper).
    struct Job(HANDLE);
    // SAFETY: a job handle is used from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    fn job() -> Option<HANDLE> {
        JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            (ok != 0).then_some(Job(h))
        })
        .as_ref()
        .map(|j| j.0)
    }

    /// Puts `child` into the job; true when it is there (false leaves the helper to the normal
    /// exit, as on other systems).
    pub fn assign(child: &Child) -> bool {
        let Some(job) = job() else { return false };
        unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) != 0 }
    }
}

#[cfg(not(windows))]
mod job {
    pub fn assign(_child: &std::process::Child) -> bool {
        false
    }
}

impl Runner {
    pub fn new(launcher: Launcher) -> Runner {
        Runner {
            launcher,
            running: None,
            preferred_port: 0,
            start_timeout: START_TIMEOUT,
            idle: IDLE,
            cancel: None,
        }
    }

    /// The port of the running helper, if it runs (a child that exited is forgotten).
    pub fn port(&mut self) -> Option<u16> {
        let r = self.running.as_mut()?;
        let exited = !matches!(r.child.try_wait(), Ok(None));
        if exited {
            self.running = None;
        }
        self.running.as_ref().map(|r| r.port)
    }

    pub fn is_running(&mut self) -> bool {
        self.port().is_some()
    }

    /// The helper's port, started first when it does not run; a search uses it now (the idle
    /// time starts again). Blocking: up to the start timeout.
    pub fn ensure(&mut self) -> Result<u16, String> {
        if let Some(port) = self.port() {
            if let Some(r) = &mut self.running {
                r.last_use = Instant::now();
            }
            return Ok(port);
        }
        let port = free_port(self.preferred_port)?;
        let launch = (self.launcher)(port)?;
        let mut child = command(&launch).spawn().map_err(|e| format!("{}: {e}", launch.program.display()))?;
        job::assign(&child);
        let deadline = Instant::now() + self.start_timeout;
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("the search helper stopped at its start ({status})"));
            }
            if api::healthy(port, Duration::from_secs(2)) {
                break;
            }
            let cancelled = self.cancel.is_some_and(|c| c.load(Ordering::Relaxed));
            if Instant::now() >= deadline || cancelled {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the search helper did not answer; it was stopped".to_owned());
            }
            std::thread::sleep(POLL);
        }
        self.preferred_port = port;
        self.running = Some(Running { child, port, api_key: launch.api_key, last_use: Instant::now() });
        Ok(port)
    }

    /// Marks a use without starting it (a download through it, say).
    pub fn touch(&mut self) {
        if let Some(r) = &mut self.running {
            r.last_use = Instant::now();
        }
    }

    /// Stops it when it has had no use for the idle time; true when it did.
    pub fn stop_if_idle(&mut self, now: Instant) -> bool {
        let idle =
            self.running.as_ref().is_some_and(|r| now.saturating_duration_since(r.last_use) >= self.idle);
        if idle {
            self.stop();
        }
        idle
    }

    /// Stops it: the API's clean shutdown, a short wait, then a kill.
    pub fn stop(&mut self) {
        let Some(mut r) = self.running.take() else { return };
        if !r.api_key.is_empty() && api::shutdown(r.port, &r.api_key) {
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until {
                if !matches!(r.child.try_wait(), Ok(None)) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let _ = r.child.kill();
        let _ = r.child.wait();
    }

    /// Kills it at once (the program's exit, where nothing may wait).
    pub fn kill(&mut self) {
        if let Some(mut r) = self.running.take() {
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }

    /// The child's process id while it runs (tests).
    pub fn pid(&self) -> Option<u32> {
        self.running.as_ref().map(|r| r.child.id())
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The stand-in helper the tests start: this test binary itself, run with one test selected
/// and `PATORRENT_HELPER_STANDIN` naming what to do. It serves `/health` (and the shutdown call)
/// on the port in `PATORRENT_HELPER_PORT`, so the start, the health check and the stops are
/// exercised with a real child process and no third-party code.
#[cfg(test)]
pub(crate) mod standin {
    use super::*;

    pub const MODE: &str = "PATORRENT_HELPER_STANDIN";
    pub const PORT: &str = "PATORRENT_HELPER_PORT";

    /// A launch of the stand-in in `mode`: `serve` answers, `mute` never does, `exit` ends at
    /// once.
    pub fn launch(port: u16, mode: &str) -> Launch {
        Launch {
            program: std::env::current_exe().unwrap(),
            args: ["search::helper::process::standin::child", "--exact", "--nocapture", "--test-threads=1"]
                .iter()
                .map(OsString::from)
                .collect(),
            cwd: None,
            env: vec![(MODE.into(), mode.into()), (PORT.into(), port.to_string().into())],
            api_key: if mode == "serve" { "standin-key".to_owned() } else { String::new() },
        }
    }

    /// The child's side; a plain test run (no `PATORRENT_HELPER_STANDIN`) passes at once.
    #[test]
    fn child() {
        use std::io::{Read, Write};
        let Ok(mode) = std::env::var(MODE) else { return };
        let port: u16 = std::env::var(PORT).unwrap().parse().unwrap();
        match mode.as_str() {
            "exit" => std::process::exit(3),
            "mute" => {
                let _l = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
                std::thread::sleep(Duration::from_secs(600));
            }
            _ => {
                let l = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
                for s in l.incoming() {
                    let Ok(mut s) = s else { continue };
                    let mut buf = [0u8; 2048];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let body = "{\"status\":\"OK\"}";
                    let _ = s.write_all(
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes(),
                    );
                    drop(s);
                    if req.starts_with("POST /api/v2.0/server/Shutdown?apikey=standin-key ") {
                        std::process::exit(0);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::standin::launch;
    use super::*;

    fn runner(mode: &'static str) -> Runner {
        let mut r = Runner::new(Box::new(move |port| Ok(launch(port, mode))));
        r.start_timeout = Duration::from_secs(20);
        r
    }

    fn alive(pid: u32, r: &mut Runner) -> bool {
        r.pid() == Some(pid) && r.is_running()
    }

    #[test]
    fn starts_answers_and_stops() {
        let mut r = runner("serve");
        assert!(!r.is_running());
        let port = r.ensure().unwrap();
        assert!(api::healthy(port, Duration::from_secs(2)));
        let pid = r.pid().unwrap();
        // a second search uses the running one
        assert_eq!(r.ensure().unwrap(), port);
        assert!(alive(pid, &mut r));
        // not idle yet; idle after the idle time
        assert!(!r.stop_if_idle(Instant::now()));
        r.idle = Duration::from_millis(50);
        std::thread::sleep(Duration::from_millis(100));
        assert!(r.stop_if_idle(Instant::now()));
        assert!(!r.is_running());
        assert!(!api::healthy(port, Duration::from_millis(500)), "the clean shutdown ended it");
        // the next start prefers the same port
        assert_eq!(r.ensure().unwrap(), port);
        // the program's exit kills it
        r.kill();
        assert!(!r.is_running());
        assert!(!api::healthy(port, Duration::from_millis(500)));
        // and dropping a runner does too
        let port = r.ensure().unwrap();
        drop(r);
        std::thread::sleep(Duration::from_millis(200));
        assert!(!api::healthy(port, Duration::from_millis(500)));
    }

    #[test]
    fn a_child_that_does_not_answer_is_killed() {
        let mut r = runner("mute");
        r.start_timeout = Duration::from_secs(3);
        let e = r.ensure().unwrap_err();
        assert!(e.contains("did not answer"), "{e}");
        assert!(!r.is_running());
        let mut r = runner("exit");
        let e = r.ensure().unwrap_err();
        assert!(e.contains("stopped at its start"), "{e}");
        // a launcher's error is the start's
        let mut r = Runner::new(Box::new(|_| Err("not installed".to_owned())));
        assert_eq!(r.ensure().unwrap_err(), "not installed");
    }

    #[test]
    fn a_taken_port_is_not_reused() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken = l.local_addr().unwrap().port();
        let p = free_port(taken).unwrap();
        assert_ne!(p, taken);
        drop(l);
        assert_eq!(free_port(taken).unwrap(), taken);
    }
}
