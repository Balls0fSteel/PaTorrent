//! Preferences > Web UI: the PaTorrent nox service, which serves the Web UI (docs/DAEMON.md).
//! The GUI has no Web UI of its own: nox is a client of its own beside this exe
//! (`patorrent-nox.exe`, which the installer and the zip ship), with its own torrents, and the
//! page sets up the Windows service that runs it. The page shows the service as the Service
//! Control Manager has it (installed, running, the options in its command line) and changes it
//! through one elevated `patorrent-nox service` command per Apply (`install --replace --now`,
//! `start` or `uninstall`), so the user sees one UAC prompt. The login is not kept here: a new
//! password goes to `service install`, which stores it hashed in the service's `webui.json`.

use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::null;

use patorrent_frontend::tr;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError, LocalFree, WAIT_OBJECT_0};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfigW,
    QueryServiceStatus, SC_HANDLE, SC_MANAGER_CONNECT, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS,
    SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS,
};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE};
use windows_sys::Win32::UI::Shell::{
    CommandLineToArgvW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, SW_HIDE,
    TranslateMessage,
};

use crate::util::{exe_path, from_wide_ptr, wide};

/// The daemon beside `patorrent.exe`.
pub const NOX_EXE: &str = "patorrent-nox.exe";
/// The service's name (`patorrent-nox service`, `sc query PaTorrentNox`).
pub const SERVICE: &str = "PaTorrentNox";
pub const DEFAULT_PORT: u16 = 8080;
const DEFAULT_USER: &str = "admin";

/// What the page edits: the service's `run` options as far as the page shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    pub username: String,
    pub port: u16,
    /// Listen on every address (`0.0.0.0`), not only on this computer.
    pub remote: bool,
    /// Serve HTTPS with nox's self-signed certificate.
    pub https: bool,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup { username: DEFAULT_USER.into(), port: DEFAULT_PORT, remote: false, https: false }
    }
}

impl Setup {
    /// The page's address on this computer.
    pub fn url(&self) -> String {
        format!("{}://127.0.0.1:{}/", if self.https { "https" } else { "http" }, self.port)
    }

    /// The `run` options the service is installed with.
    fn run_args(&self) -> Vec<String> {
        let addr = if self.remote { "0.0.0.0" } else { "127.0.0.1" };
        let mut args = vec!["--webui".to_owned(), format!("{addr}:{}", self.port)];
        args.push("--webui-user".into());
        args.push(self.username.clone());
        if self.https {
            args.push("--webui-https".into());
        }
        args
    }

    /// The setup in a service's image path (`"<exe>" service run OPTIONS`), with the exe;
    /// `None` for a command line that is not a `patorrent-nox service run`.
    pub fn parse_image_path(line: &str) -> Option<(PathBuf, Setup)> {
        let argv = split_args(line);
        let (exe, rest) = argv.split_first()?;
        let exe = PathBuf::from(exe);
        let is_nox = exe.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(NOX_EXE));
        if !is_nox || rest.len() < 2 || rest[0] != "service" || rest[1] != "run" {
            return None;
        }
        let mut setup = Setup::default();
        let mut it = rest[2..].iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--webui" => {
                    let v = it.next()?;
                    // [ADDR:]PORT as nox parses it: a bare port listens on 127.0.0.1
                    let (host, port) = match v.rsplit_once(':') {
                        Some((h, p)) => (h.trim_start_matches('[').trim_end_matches(']'), p),
                        None => ("127.0.0.1", v.as_str()),
                    };
                    setup.port = port.parse().ok()?;
                    setup.remote = !matches!(host, "127.0.0.1" | "localhost" | "::1");
                }
                "--webui-user" => setup.username = it.next()?.clone(),
                "--webui-https" => setup.https = true,
                "--webui-cert" => {
                    it.next()?;
                    setup.https = true;
                }
                _ => {}
            }
        }
        Some((exe, setup))
    }
}

/// The service as the Service Control Manager has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Service {
    /// The exe its command line names (empty for a command line that is not nox's).
    pub exe: PathBuf,
    pub setup: Setup,
    pub running: bool,
}

/// What the page shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `patorrent-nox.exe` beside this exe, when it is there.
    pub nox: Option<PathBuf>,
    /// The service, when installed.
    pub service: Option<Service>,
}

impl Status {
    pub fn installed(&self) -> bool {
        self.service.is_some()
    }

    pub fn running(&self) -> bool {
        self.service.as_ref().is_some_and(|s| s.running)
    }

    /// The service runs another copy than the one beside this exe.
    pub fn elsewhere(&self) -> bool {
        match (&self.nox, &self.service) {
            (Some(nox), Some(s)) => !same_path(nox, &s.exe),
            _ => false,
        }
    }

    /// The page's setup: the service's, else the defaults.
    pub fn setup(&self) -> Setup {
        self.service.as_ref().map(|s| s.setup.clone()).unwrap_or_default()
    }

    /// The page's status line.
    pub fn text(&self) -> String {
        if self.nox.is_none() {
            return tr!(WebUiNoExe).to_owned();
        }
        match &self.service {
            None => tr!(WebUiAbsent).to_owned(),
            Some(s) if self.elsewhere() => tr!(WebUiElsewhere, path = s.exe.display()),
            Some(s) if s.running => tr!(WebUiRunning, url = s.setup.url()),
            Some(_) => tr!(WebUiStopped).to_owned(),
        }
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// What Apply asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Install (over an installed service) and start, with a new password when given.
    Install {
        setup: Setup,
        password: Option<String>,
    },
    Start,
    Uninstall,
}

impl Change {
    /// The `patorrent-nox` command line (without the exe).
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["service".to_owned()];
        match self {
            Change::Install { setup, password } => {
                args.extend(["install", "--replace", "--now"].map(str::to_owned));
                args.extend(setup.run_args());
                if let Some(p) = password {
                    args.push("--webui-pass".into());
                    args.push(p.clone());
                }
            }
            Change::Start => args.push("start".into()),
            Change::Uninstall => args.push("uninstall".into()),
        }
        args
    }
}

/// The dialog's hook: `None` reads the status, `Some(change)` runs the elevated command (one
/// UAC prompt; the window keeps painting meanwhile) and reads it afterwards.
pub fn apply(change: Option<&Change>) -> Result<Status, String> {
    if let Some(c) = change {
        let nox = nox_exe().ok_or_else(|| tr!(WebUiNoExe).to_owned())?;
        run_elevated(&nox, &c.args())?;
    }
    Ok(read())
}

pub fn read() -> Status {
    Status { nox: nox_exe(), service: query_service() }
}

fn nox_exe() -> Option<PathBuf> {
    let p = exe_path().parent()?.join(NOX_EXE);
    p.is_file().then_some(p)
}

struct Sc(SC_HANDLE);

impl Drop for Sc {
    fn drop(&mut self) {
        // SAFETY: a handle we opened and close once
        unsafe { CloseServiceHandle(self.0) };
    }
}

/// The service's configuration and state. Querying needs no elevation; `None` when it is not
/// installed (or cannot be opened).
fn query_service() -> Option<Service> {
    unsafe {
        // SAFETY: the local machine's active database, connect only
        let m = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if m.is_null() {
            return None;
        }
        let m = Sc(m);
        let name = wide(SERVICE);
        // SAFETY: an open manager handle and a NUL-terminated name
        let s = OpenServiceW(m.0, name.as_ptr(), SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS);
        if s.is_null() {
            return None;
        }
        let s = Sc(s);
        // the size first, then the configuration into a buffer of that size
        let mut needed = 0u32;
        QueryServiceConfigW(s.0, std::ptr::null_mut(), 0, &mut needed);
        let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        let config = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
        // SAFETY: an open handle with SERVICE_QUERY_CONFIG and a buffer of the size asked for
        let line = if QueryServiceConfigW(s.0, config, needed, &mut needed) == 0
            || (*config).lpBinaryPathName.is_null()
        {
            String::new()
        } else {
            from_wide_ptr((*config).lpBinaryPathName)
        };
        // SAFETY: zeroes are a valid SERVICE_STATUS; an open handle with SERVICE_QUERY_STATUS
        let mut st: SERVICE_STATUS = zeroed();
        let running = QueryServiceStatus(s.0, &mut st) != 0
            && matches!(st.dwCurrentState, SERVICE_RUNNING | SERVICE_START_PENDING);
        let (exe, setup) = Setup::parse_image_path(&line).unwrap_or_default();
        Some(Service { exe, setup, running })
    }
}

/// Runs `exe args` elevated (a UAC prompt), without a console window, and waits for it,
/// pumping this thread's messages meanwhile. The errors are the user's texts.
fn run_elevated(exe: &Path, args: &[String]) -> Result<(), String> {
    let params = command_line(args);
    let command = format!("\"{}\" {params}", exe.display());
    let (verb, file, parameters) = (wide("runas"), wide(&exe.to_string_lossy()), wide(&params));
    unsafe {
        // SAFETY: zeroes are a valid SHELLEXECUTEINFOW; the strings outlive the call
        let mut info: SHELLEXECUTEINFOW = zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = parameters.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            let e = GetLastError();
            return Err(if e == ERROR_CANCELLED {
                tr!(WebUiDeclined).to_owned()
            } else {
                tr!(WebUiFailed, code = e, command)
            });
        }
        let process = info.hProcess;
        if process.is_null() {
            return Err(tr!(WebUiFailed, code = 0, command));
        }
        // MsgWaitForMultipleObjects returns when the process ends or input is waiting
        loop {
            let r = MsgWaitForMultipleObjects(1, &process, 0, INFINITE, QS_ALLINPUT);
            if r == WAIT_OBJECT_0 {
                break;
            }
            let mut msg: MSG = zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let mut code = 0u32;
        let got = GetExitCodeProcess(process, &mut code) != 0;
        CloseHandle(process);
        if got && code == 0 { Ok(()) } else { Err(tr!(WebUiFailed, code, command)) }
    }
}

/// Splits a command line as Windows does (`CommandLineToArgvW`).
fn split_args(line: &str) -> Vec<String> {
    if line.trim().is_empty() {
        return Vec::new();
    }
    let w = wide(line);
    let mut n = 0i32;
    unsafe {
        // SAFETY: a NUL-terminated string and a valid count pointer; the array is freed once
        let argv = CommandLineToArgvW(w.as_ptr(), &mut n);
        if argv.is_null() {
            return Vec::new();
        }
        let out = (0..n.max(0) as usize).map(|i| from_wide_ptr(*argv.add(i))).collect();
        LocalFree(argv as _);
        out
    }
}

/// Joins `args` the way `CommandLineToArgvW` splits them again (nox's own rule): quotes only
/// when needed; inside them a `"` gets a backslash and the backslashes before it doubled.
fn command_line(args: &[String]) -> String {
    let mut s = String::new();
    for a in args {
        if !s.is_empty() {
            s.push(' ');
        }
        if !a.is_empty() && !a.contains([' ', '\t', '\n', '\x0b', '"']) {
            s.push_str(a);
            continue;
        }
        s.push('"');
        let mut backslashes = 0;
        for c in a.chars() {
            if c == '\\' {
                backslashes += 1;
            } else {
                if c == '"' {
                    s.extend(std::iter::repeat_n('\\', backslashes + 1));
                }
                backslashes = 0;
            }
            s.push(c);
        }
        s.extend(std::iter::repeat_n('\\', backslashes));
        s.push('"');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_image_path_gives_the_setup_back() {
        let line = r#""C:\Program Files\PaTorrent\patorrent-nox.exe" service run --webui 0.0.0.0:8443 --webui-user me --webui-https --state-dir "C:\ProgramData\PaTorrent\nox" --save "C:\ProgramData\PaTorrent\nox\Downloads""#;
        let (exe, setup) = Setup::parse_image_path(line).unwrap();
        assert_eq!(exe, PathBuf::from(r"C:\Program Files\PaTorrent\patorrent-nox.exe"));
        assert_eq!(setup, Setup { username: "me".into(), port: 8443, remote: true, https: true });
        assert_eq!(setup.url(), "https://127.0.0.1:8443/");

        // a bare port and the defaults
        let (_, setup) =
            Setup::parse_image_path(r#""D:\pa\PATORRENT-NOX.EXE" service run --webui 8080"#).unwrap();
        assert_eq!(setup, Setup::default());
        assert_eq!(setup.url(), "http://127.0.0.1:8080/");
        let (_, setup) =
            Setup::parse_image_path(r#""D:\pa\patorrent-nox.exe" service run --webui [::1]:9000"#).unwrap();
        assert_eq!((setup.port, setup.remote), (9000, false));
        let (_, setup) =
            Setup::parse_image_path(r#""D:\pa\patorrent-nox.exe" service run --webui-cert c.pem"#).unwrap();
        assert!(setup.https);

        // not nox's service
        assert!(Setup::parse_image_path(r#""C:\x\other.exe" service run --webui 8080"#).is_none());
        assert!(Setup::parse_image_path(r#""C:\x\patorrent-nox.exe" run --webui 8080"#).is_none());
        assert!(Setup::parse_image_path(r#""C:\x\patorrent-nox.exe" service run --webui"#).is_none());
        assert!(Setup::parse_image_path("").is_none());
    }

    #[test]
    fn the_changes_are_one_service_command_each() {
        let setup = Setup { username: "a b".into(), port: 8080, remote: false, https: false };
        let install = Change::Install { setup: setup.clone(), password: Some(r#"p"q\"#.into()) };
        assert_eq!(
            install.args(),
            strings(&[
                "service",
                "install",
                "--replace",
                "--now",
                "--webui",
                "127.0.0.1:8080",
                "--webui-user",
                "a b",
                "--webui-pass",
                r#"p"q\"#,
            ])
        );
        let line = command_line(&install.args());
        assert_eq!(
            line,
            r#"service install --replace --now --webui 127.0.0.1:8080 --webui-user "a b" --webui-pass "p\"q\\""#
        );
        assert_eq!(split_args(&line), install.args());

        let remote = Setup { remote: true, https: true, ..Setup::default() };
        let args = Change::Install { setup: remote, password: None }.args();
        assert!(args.contains(&"0.0.0.0:8080".to_owned()) && args.contains(&"--webui-https".to_owned()));
        assert!(!args.iter().any(|a| a == "--webui-pass"));
        assert_eq!(Change::Start.args(), strings(&["service", "start"]));
        assert_eq!(Change::Uninstall.args(), strings(&["service", "uninstall"]));
    }

    #[test]
    fn the_status_knows_whose_service_it_is() {
        let nox = PathBuf::from(r"C:\pa\patorrent-nox.exe");
        let mut st = Status { nox: Some(nox.clone()), service: None };
        assert!(!st.installed() && !st.running() && !st.elsewhere());
        assert_eq!(st.setup(), Setup::default());
        st.service = Some(Service {
            exe: PathBuf::from(r"c:\PA\patorrent-nox.exe"),
            setup: Setup::default(),
            running: true,
        });
        assert!(st.installed() && st.running() && !st.elsewhere(), "paths compare without case");
        st.service.as_mut().unwrap().exe = PathBuf::from(r"D:\old\patorrent-nox.exe");
        assert!(st.elsewhere());
        assert!(st.text().contains(r"D:\old\patorrent-nox.exe"));
        st.nox = None;
        assert!(!st.elsewhere());
    }
}
