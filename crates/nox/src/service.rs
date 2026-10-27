// PaTorrent's own OS glue (no libtorrent counterpart; qBittorrent-nox has no Windows service).

//! `patorrent-nox` as a Windows service (PLAN.md Phase 6, docs/DAEMON.md). Windows only.
//!
//! - `service install [--account A] [--start T] [RUN OPTIONS]` registers [`NAME`] ("PaTorrent
//!   nox") with the Service Control Manager. Its image path is this exe, quoted, then `service
//!   run` and the run options, with relative paths made absolute (a service starts in
//!   System32). The account is LocalService unless `--account` names NetworkService or
//!   LocalSystem (built-in accounts need no password). Without `--state-dir` the state goes to
//!   `%ProgramData%\PaTorrent\nox`, not the account's `%APPDATA%` (hidden under
//!   `C:\Windows\ServiceProfiles`); the state directory gets a protected ACL (SYSTEM,
//!   Administrators and the account only: `webui.json` and the log hold the Web UI login). One
//!   that exists already is taken only if it, its parents and its contents belong to trusted
//!   owners and it holds no junctions; its ACL is then reset (SEC-23). Without `--save` the downloads go to its `Downloads` folder. Delayed
//!   automatic start, restart 5 s / 30 s / 60 s after a failure (also after a clean exit with an
//!   error code), 60 s for the shutdown when Windows shuts down (PRESHUTDOWN).
//! - `service uninstall`, `service start` and `service stop` (waiting for the state change).
//! - `service run` is what the SCM starts: `StartServiceCtrlDispatcherW` → [`service_main`] →
//!   `RegisterServiceCtrlHandlerExW`. The status follows the points where `daemon` sends
//!   systemd its notifications ([`notify`]): START_PENDING until `READY=1` (session started,
//!   Web UI listening), RUNNING, then STOP_PENDING from a stop control or `STOPPING=1` while
//!   resume data is saved and port mappings are deleted, STOPPED with an exit code. The pending
//!   states count a checkpoint every second for up to two minutes. Controls take the console
//!   events' path through `signals`: STOP, SHUTDOWN and PRESHUTDOWN stop cleanly, PARAMCHANGE
//!   reloads as SIGHUP does; logoff events are ignored. There is no console, so `--log`
//!   defaults to `nox.log` in the state directory.
//! - `service debug [RUN OPTIONS]` runs the same body in a console, without the SCM, printing
//!   the states it would report.

use std::ffi::{OsStr, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_CALL_NOT_IMPLEMENTED, ERROR_FAILED_SERVICE_CONTROLLER_CONNECT,
    ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_EXISTS,
    ERROR_SERVICE_MARKED_FOR_DELETE, ERROR_SERVICE_NOT_ACTIVE, ERROR_SERVICE_SPECIFIC_ERROR, LocalFree,
    NO_ERROR,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
    SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{CreateDirectoryW, DELETE};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfig2W, CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW,
    OpenServiceW, QueryServiceStatus, RegisterServiceCtrlHandlerExW, SC_ACTION, SC_ACTION_RESTART, SC_HANDLE,
    SC_MANAGER_CONNECT, SC_MANAGER_CREATE_SERVICE, SERVICE_ACCEPT_PARAMCHANGE, SERVICE_ACCEPT_PRESHUTDOWN,
    SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS, SERVICE_AUTO_START,
    SERVICE_CONFIG_DELAYED_AUTO_START_INFO, SERVICE_CONFIG_DESCRIPTION, SERVICE_CONFIG_FAILURE_ACTIONS,
    SERVICE_CONFIG_FAILURE_ACTIONS_FLAG, SERVICE_CONFIG_PRESHUTDOWN_INFO, SERVICE_CONTROL_INTERROGATE,
    SERVICE_CONTROL_PARAMCHANGE, SERVICE_CONTROL_PRESHUTDOWN, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP,
    SERVICE_DELAYED_AUTO_START_INFO, SERVICE_DEMAND_START, SERVICE_DESCRIPTIONW, SERVICE_ERROR_NORMAL,
    SERVICE_FAILURE_ACTIONS_FLAG, SERVICE_FAILURE_ACTIONSW, SERVICE_PRESHUTDOWN_INFO, SERVICE_QUERY_STATUS,
    SERVICE_RUNNING, SERVICE_START, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_HANDLE,
    SERVICE_STOP, SERVICE_STOP_PENDING, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
    StartServiceCtrlDispatcherW, StartServiceW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::core::PWSTR;

use crate::signals::{self, ServiceControl};

/// The service's name (`sc query PaTorrentNox`).
pub const NAME: &str = "PaTorrentNox";
const DISPLAY_NAME: &str = "PaTorrent nox";
const DESCRIPTION: &str =
    "Headless PaTorrent BitTorrent client (patorrent-nox run), driven through its Web UI.";
/// The log file in the state directory when no `--log` is given.
const LOG_FILE: &str = "nox.log";

/// The service-specific exit codes (`sc query` shows them as SERVICE_EXIT_CODE).
const EXIT_FAILED: u32 = 1;
const EXIT_ARGS: u32 = 2;
const EXIT_LOG: u32 = 3;

/// `run` options with a path for a value: made absolute at install.
const PATH_OPTIONS: [&str; 10] = [
    "--save",
    "--state",
    "--state-dir",
    "--config-dir",
    "--ip-filter",
    "--pid-file",
    "--log",
    "--move-on-complete",
    "--webui-cert",
    "--webui-key",
];

const USAGE: &str = "install, uninstall, start, stop, run or debug (patorrent-nox --help)";

/// `patorrent-nox service ...`. `Err` is a usage error; failures are printed and give exit 1.
pub fn main(args: &[String]) -> Result<ExitCode, String> {
    let (cmd, rest) = args.split_first().ok_or_else(|| format!("service: {USAGE}"))?;
    let no_args = || if rest.is_empty() { Ok(()) } else { Err(format!("service {cmd} takes no options")) };
    match cmd.as_str() {
        "install" => parse_install(rest, program_data()).map(|i| outcome(install(&i))),
        "uninstall" | "remove" => no_args().map(|()| outcome(uninstall())),
        "start" => no_args().map(|()| outcome(start())),
        "stop" => no_args().map(|()| outcome(stop())),
        "run" => Ok(dispatch(rest)),
        "debug" => {
            Ok(if serve(Box::new(NoScm), rest, false) == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        other => Err(format!("service: unknown command {other:?}; {USAGE}")),
    }
}

fn outcome(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

// ---- install: the options and the image path ----

/// The account the service runs as: the built-in ones, which have no password.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Account {
    LocalService,
    NetworkService,
    LocalSystem,
}

impl Account {
    fn parse(s: &str) -> Result<Account, String> {
        let s = s.to_ascii_lowercase();
        match s.strip_prefix(r"nt authority\").unwrap_or(&s) {
            "localservice" => Ok(Account::LocalService),
            "networkservice" => Ok(Account::NetworkService),
            "localsystem" | "system" => Ok(Account::LocalSystem),
            _ => Err(format!("--account {s}: LocalService, NetworkService or LocalSystem")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Account::LocalService => r"NT AUTHORITY\LocalService",
            Account::NetworkService => r"NT AUTHORITY\NetworkService",
            Account::LocalSystem => "LocalSystem",
        }
    }

    /// The protected ACL of a state directory created at install: full control for SYSTEM,
    /// Administrators and the account, inherited by everything inside, nothing from above.
    fn state_dir_sddl(self) -> String {
        let me = match self {
            Account::LocalService => "(A;OICI;FA;;;LS)",
            Account::NetworkService => "(A;OICI;FA;;;NS)",
            Account::LocalSystem => "",
        };
        format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA){me}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartType {
    /// Automatic, delayed until the boot has settled (and the network is usually up).
    Delayed,
    Auto,
    Manual,
}

impl StartType {
    fn parse(s: &str) -> Result<StartType, String> {
        match s.to_ascii_lowercase().as_str() {
            "delayed" => Ok(StartType::Delayed),
            "auto" | "automatic" => Ok(StartType::Auto),
            "manual" | "demand" => Ok(StartType::Manual),
            _ => Err(format!("--start {s}: delayed, auto or manual")),
        }
    }
}

/// `service install`'s choices.
#[derive(Debug, PartialEq)]
struct Install {
    account: Account,
    start: StartType,
    /// The `run` options for the image path: paths absolute, the defaults added.
    args: Vec<String>,
    state: PathBuf,
    /// The downloads folder when `--save` was not given (inside the state directory).
    default_save: Option<PathBuf>,
    log: PathBuf,
    /// `--webui-pass` ends up in the service's configuration, which every local user can read.
    password_in_args: bool,
    webui: bool,
}

/// Whether `run` takes a value after `opt`, asked of `run`'s own parser (so the two cannot
/// drift apart): it fails with "OPT needs a value" when the value is missing.
fn takes_value(opt: &str) -> bool {
    opt.starts_with("--") && crate::parse(&[opt.to_owned()]).err() == Some(format!("{opt} needs a value"))
}

/// Parses `service install`'s options: its own (`--account`, `--start`) and `run`'s, which are
/// checked by `run`'s parser. `program_data` is `%ProgramData%`, for the default state.
fn parse_install(args: &[String], program_data: Option<PathBuf>) -> Result<Install, String> {
    let mut account = Account::LocalService;
    let mut start = StartType::Delayed;
    let mut out = Vec::new();
    let mut has_save = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--account" => account = Account::parse(&value()?)?,
            "--start" => start = StartType::parse(&value()?)?,
            _ if takes_value(a) => {
                let v = value()?;
                has_save |= a == "--save";
                out.push(a.clone());
                out.push(if PATH_OPTIONS.contains(&a.as_str()) {
                    let abs = std::path::absolute(&v).map_err(|e| format!("{a} {v}: {e}"))?;
                    abs.to_string_lossy().into_owned()
                } else {
                    v
                });
            }
            _ => out.push(a.clone()),
        }
    }
    let o = crate::parse(&out)?;
    if !o.sources.is_empty() {
        return Err(format!(
            "service install: {} would be added again at every start; add torrents in the Web UI",
            o.sources[0]
        ));
    }
    if o.service.daemon {
        return Err("service install: --daemon is for Unix; the service runs in the background anyway".into());
    }
    if o.no_state {
        return Err("service install: a service keeps its state; leave out --no-state".into());
    }
    let state = match &o.state {
        Some(s) => s.clone(),
        None => {
            let base =
                program_data.ok_or("service install: %ProgramData% is not set; give --state-dir DIR")?;
            let s = base.join("PaTorrent").join("nox");
            out.push("--state-dir".into());
            out.push(s.to_string_lossy().into_owned());
            s
        }
    };
    let default_save = (!has_save).then(|| {
        let d = state.join("Downloads");
        out.push("--save".into());
        out.push(d.to_string_lossy().into_owned());
        d
    });
    Ok(Install {
        account,
        start,
        log: o.service.log.clone().unwrap_or_else(|| state.join(LOG_FILE)),
        state,
        default_save,
        password_in_args: o.webui_pass.is_some(),
        webui: o.webui.is_some(),
        args: out,
    })
}

/// Appends `arg` quoted the way `CommandLineToArgvW` and the MSVC runtime (and Rust's
/// `std::env::args`) split it again: quotes only when needed; inside them a `"` gets a
/// backslash and the backslashes before it (or before the closing quote) are doubled.
fn quote_arg(arg: &str, out: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    out.extend(std::iter::repeat_n('\\', backslashes));
    out.push('"');
}

/// The service's image path: the exe, always quoted (its own rule: no escapes, and a path with
/// spaces unquoted is the classic "unquoted service path" hole), then the arguments.
fn command_line(exe: &Path, args: &[String]) -> String {
    let mut s = format!("\"{}\"", exe.display());
    for a in args {
        s.push(' ');
        quote_arg(a, &mut s);
    }
    s
}

fn program_data() -> Option<PathBuf> {
    std::env::var_os("ProgramData").filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Whether `path` is inside `dir`, ignoring case as Windows does.
fn inside(path: &Path, dir: &Path) -> bool {
    let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    lower(path).starts_with(lower(dir))
}

// ---- install, uninstall, start, stop ----

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

/// An open SCM or service handle.
struct Sc(SC_HANDLE);

impl Drop for Sc {
    fn drop(&mut self) {
        // SAFETY: a handle we opened and close once
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn manager(access: u32) -> io::Result<Sc> {
    // SAFETY: the local machine's active database
    let h = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
    if h.is_null() { Err(io::Error::last_os_error()) } else { Ok(Sc(h)) }
}

fn open(access: u32) -> io::Result<Sc> {
    let m = manager(SC_MANAGER_CONNECT)?;
    let name = wide(NAME);
    // SAFETY: an open manager handle and a NUL-terminated name
    let h = unsafe { OpenServiceW(m.0, name.as_ptr(), access) };
    if h.is_null() { Err(io::Error::last_os_error()) } else { Ok(Sc(h)) }
}

fn query(s: &Sc) -> io::Result<SERVICE_STATUS> {
    // SAFETY: zeroes are a valid SERVICE_STATUS
    let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: an open service handle with SERVICE_QUERY_STATUS and a valid out pointer
    if unsafe { QueryServiceStatus(s.0, &mut st) } == 0 { Err(io::Error::last_os_error()) } else { Ok(st) }
}

/// Polls the service while it is in `pending`; returns the status it ended in.
fn wait_while(s: &Sc, pending: u32, timeout: Duration) -> Result<SERVICE_STATUS, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let st = query(s).map_err(|e| scm_error("querying the service", e))?;
        if st.dwCurrentState != pending {
            return Ok(st);
        }
        if Instant::now() >= deadline {
            return Err(format!("{NAME} still {} after {} s", state_name(pending), timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// An SCM error with what to do about it.
fn scm_error(what: &str, e: io::Error) -> String {
    let hint = match e.raw_os_error().map(|c| c as u32) {
        Some(ERROR_ACCESS_DENIED) => "; run it from an administrator prompt (\"Run as administrator\")",
        Some(ERROR_SERVICE_DOES_NOT_EXIST) => "; install it first: patorrent-nox service install [OPTIONS]",
        Some(ERROR_SERVICE_EXISTS) => "; it is installed already (patorrent-nox service uninstall first)",
        Some(ERROR_SERVICE_MARKED_FOR_DELETE) => {
            "; close the Services console (services.msc) and other programs holding it, then try again"
        }
        _ => "",
    };
    format!("{what}: {e}{hint}")
}

fn state_name(state: u32) -> &'static str {
    match state {
        SERVICE_START_PENDING => "START_PENDING",
        SERVICE_RUNNING => "RUNNING",
        SERVICE_STOP_PENDING => "STOP_PENDING",
        SERVICE_STOPPED => "STOPPED",
        _ => "in another state",
    }
}

/// Why a service stopped, from its exit codes.
fn exit_text(win32: u32, specific: u32) -> String {
    match (win32, specific) {
        (NO_ERROR, _) => "exit code 0".into(),
        (ERROR_SERVICE_SPECIFIC_ERROR, EXIT_ARGS) => {
            format!("exit code {EXIT_ARGS}: its command line is invalid (sc qc {NAME} shows it)")
        }
        (ERROR_SERVICE_SPECIFIC_ERROR, EXIT_LOG) => {
            format!("exit code {EXIT_LOG}: it cannot open its log file")
        }
        (ERROR_SERVICE_SPECIFIC_ERROR, c) => format!("exit code {c}: see {LOG_FILE} in its state directory"),
        (w, _) => io::Error::from_raw_os_error(w as i32).to_string(),
    }
}

/// Creates `dir` (not its parents) with the ACL in `sddl`.
fn create_dir_with_acl(dir: &Path, sddl: &str) -> io::Result<()> {
    let sddl = wide(sddl);
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: a NUL-terminated string and a valid out pointer; the size is not wanted
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    let path = wide(dir);
    // SAFETY: a NUL-terminated path and attributes pointing at a valid descriptor
    let created = unsafe { CreateDirectoryW(path.as_ptr(), &sa) } != 0;
    let err = io::Error::last_os_error();
    // SAFETY: the descriptor was allocated by the conversion above, with LocalAlloc
    unsafe { LocalFree(sd) };
    if created { Ok(()) } else { Err(err) }
}

/// Owners trusted for a state directory, its parents and what is in it (SEC-23): SYSTEM,
/// Administrators, LocalService, NetworkService and TrustedInstaller. Whoever runs the install
/// (an administrator) is trusted too. Any other owner may be a user who made the directory
/// before the install and can still change it.
const TRUSTED_OWNERS: [&str; 5] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-19",
    "S-1-5-20",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

/// `FILE_ATTRIBUTE_REPARSE_POINT`: a junction or a symbolic link.
const REPARSE_POINT: u32 = 0x400;

/// A SID as text (`S-1-5-18`).
fn sid_string(sid: PSID) -> io::Result<String> {
    let mut s: PWSTR = std::ptr::null_mut();
    // SAFETY: a valid SID and an out pointer; the string is allocated with LocalAlloc
    if unsafe { ConvertSidToStringSidW(sid, &mut s) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a NUL-terminated string from the call above, freed once read
    let text = unsafe {
        let len = (0..).take_while(|&i| *s.add(i) != 0).count();
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(s, len));
        LocalFree(s.cast());
        text
    };
    Ok(text)
}

/// The owner of `path` (not following a link at its end only if it is not one: callers refuse
/// reparse points first); `None` when it has none, as on a FAT drive.
fn owner_of(path: &Path) -> io::Result<Option<String>> {
    let name = wide(path);
    let mut owner: PSID = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: a NUL-terminated path and out pointers; the owner points into `sd`
    let err = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if err != NO_ERROR {
        return Err(io::Error::from_raw_os_error(err as i32));
    }
    let text = if owner.is_null() { Ok(None) } else { sid_string(owner).map(Some) };
    // SAFETY: allocated by GetNamedSecurityInfoW with LocalAlloc
    unsafe { LocalFree(sd) };
    text
}

/// The SID of the user running this process.
fn current_user() -> io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo handle of this process and an out pointer
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // TOKEN_USER and its SID; u64 for the alignment
    let mut buf = [0u64; 64];
    let mut len = 0;
    // SAFETY: an open token and a buffer of the size given
    let ok = unsafe {
        GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), size_of_val(&buf) as u32, &mut len)
    };
    let err = io::Error::last_os_error();
    // SAFETY: opened above
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return Err(err);
    }
    // SAFETY: the call filled the buffer with a TOKEN_USER, its SID pointing into the buffer
    sid_string(unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}

/// Sets the protected ACL in `sddl` on `dir`; Windows hands its inherited entries down to
/// everything inside, replacing what they inherited before.
fn secure_dir(dir: &Path, sddl: &str) -> io::Result<()> {
    let sddl = wide(sddl);
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: a NUL-terminated string and a valid out pointer
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let (mut present, mut defaulted, mut dacl) = (0, 0, std::ptr::null_mut::<ACL>());
    // SAFETY: the descriptor converted above; the ACL points into it
    let ok = unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) };
    let path = wide(dir);
    let err = if ok == 0 || present == 0 {
        Err(io::Error::other("no DACL in the descriptor"))
    } else {
        // SAFETY: a NUL-terminated path and the ACL from the live descriptor
        match unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            )
        } {
            NO_ERROR => Ok(()),
            e => Err(io::Error::from_raw_os_error(e as i32)),
        }
    };
    // SAFETY: allocated by the conversion with LocalAlloc
    unsafe { LocalFree(sd) };
    err
}

/// Why `path` cannot be trusted, if it cannot: a junction or link, or an owner other than the
/// trusted ones.
fn distrust(path: &Path, trusted: &dyn Fn(&str) -> bool, owner: &Owner<'_>) -> Option<String> {
    use std::os::windows::fs::MetadataExt;
    let shown = path.display();
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_attributes() & REPARSE_POINT != 0 => {
            return Some(format!("{shown} is a junction or link"));
        }
        Ok(_) => {}
        Err(e) => return Some(format!("{shown}: {e}")),
    }
    owner_problem(path, trusted, owner)
}

/// Looks up an owner; a parameter so tests can name other owners.
type Owner<'a> = dyn Fn(&Path) -> io::Result<Option<String>> + 'a;

/// Creates the state directory with the protected ACL in `sddl`, or takes one that exists
/// (SEC-23). Any user can make folders under `%ProgramData%` and owns what they make, so a
/// directory made before the install could hold a planted `webui.json` or a `Downloads`
/// junction, or be renamed away by the owner of a parent. Its existing parents, the directory
/// and everything in it must belong to a trusted owner and hold no junctions or links; the
/// directory's ACL is then reset. True if it was created.
fn prepare_state_dir(
    state: &Path,
    sddl: &str,
    trusted: &dyn Fn(&str) -> bool,
    owner: &Owner<'_>,
) -> Result<bool, String> {
    let refuse = |why: String| {
        Err(format!(
            "{why}: another user may have made it and could still change it. Remove it, or install \
             with another --state-dir"
        ))
    };
    for parent in state.ancestors().skip(1).filter(|p| !p.as_os_str().is_empty() && p.exists()) {
        if let Some(why) = owner_problem(parent, trusted, owner) {
            return refuse(why);
        }
    }
    if std::fs::symlink_metadata(state).is_err() {
        if let Some(parent) = state.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        create_dir_with_acl(state, sddl).map_err(|e| format!("{}: {e}", state.display()))?;
        return Ok(true);
    }
    if let Some(why) = distrust(state, trusted, owner) {
        return refuse(why);
    }
    if !state.is_dir() {
        return Err(format!("{} is not a directory", state.display()));
    }
    // first the ACL, so no new entries appear while the contents are checked
    secure_dir(state, sddl).map_err(|e| format!("{}: cannot reset its permissions: {e}", state.display()))?;
    let mut dirs = vec![state.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
            if let Some(why) = distrust(&path, trusted, owner) {
                return refuse(why);
            }
            if path.is_dir() {
                dirs.push(path);
            }
        }
    }
    Ok(false)
}

/// A parent's owner only: the path to the state directory may pass through a junction.
fn owner_problem(path: &Path, trusted: &dyn Fn(&str) -> bool, owner: &Owner<'_>) -> Option<String> {
    let shown = path.display();
    match owner(path) {
        Ok(Some(sid)) if trusted(&sid) => None,
        Ok(Some(sid)) => Some(format!("{shown} belongs to {sid}, not to SYSTEM, Administrators or you")),
        Ok(None) => Some(format!("{shown} has no owner (a drive without permissions?)")),
        Err(e) => Some(format!("{shown}: cannot read its owner: {e}")),
    }
}

/// `ChangeServiceConfig2W` with `info`; an error to print as a warning.
fn config2<T>(s: &Sc, level: u32, info: &T, what: &str) -> Option<String> {
    // SAFETY: an open handle with SERVICE_CHANGE_CONFIG and the structure `level` names
    let ok = unsafe { ChangeServiceConfig2W(s.0, level, (info as *const T).cast::<c_void>()) } != 0;
    (!ok).then(|| format!("{what}: {}", io::Error::last_os_error()))
}

fn install(i: &Install) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this exe: {e}"))?;
    let exe = PathBuf::from(exe.to_string_lossy().trim_start_matches(r"\\?\"));
    let mut args = vec!["service".to_owned(), "run".to_owned()];
    args.extend(i.args.iter().cloned());
    let image = command_line(&exe, &args);
    // first, so an unelevated prompt fails before anything is created
    let m = manager(SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE)
        .map_err(|e| scm_error("service install", e))?;

    let me = current_user().ok();
    let trusted = |sid: &str| TRUSTED_OWNERS.contains(&sid) || me.as_deref() == Some(sid);
    let created = prepare_state_dir(&i.state, &i.account.state_dir_sddl(), &trusted, &owner_of)?;
    if let Some(d) = &i.default_save {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }

    let (name, display, path) = (wide(NAME), wide(DISPLAY_NAME), wide(&image));
    let account = (i.account != Account::LocalSystem).then(|| wide(i.account.name()));
    let empty = wide("");
    let start_type = if i.start == StartType::Manual { SERVICE_DEMAND_START } else { SERVICE_AUTO_START };
    // SAFETY: NUL-terminated strings that outlive the call; no load order group, tag or
    // dependencies; the built-in accounts take an empty password (LocalSystem none)
    let h = unsafe {
        CreateServiceW(
            m.0,
            name.as_ptr(),
            display.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            start_type,
            SERVICE_ERROR_NORMAL,
            path.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            account.as_ref().map_or(std::ptr::null(), |a| a.as_ptr()),
            if account.is_some() { empty.as_ptr() } else { std::ptr::null() },
        )
    };
    if h.is_null() {
        return Err(scm_error("service install", io::Error::last_os_error()));
    }
    let s = Sc(h);

    let mut warnings = Vec::new();
    let mut description = wide(DESCRIPTION);
    let d = SERVICE_DESCRIPTIONW { lpDescription: description.as_mut_ptr() };
    warnings.extend(config2(&s, SERVICE_CONFIG_DESCRIPTION, &d, "description"));
    if i.start == StartType::Delayed {
        let d = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: 1 };
        warnings.extend(config2(&s, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, &d, "delayed start"));
    }
    let mut actions = [5_000, 30_000, 60_000].map(|ms| SC_ACTION { Type: SC_ACTION_RESTART, Delay: ms });
    let f = SERVICE_FAILURE_ACTIONSW {
        // a day without failures starts the count again
        dwResetPeriod: 24 * 3600,
        lpRebootMsg: std::ptr::null_mut(),
        lpCommand: std::ptr::null_mut(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    warnings.extend(config2(&s, SERVICE_CONFIG_FAILURE_ACTIONS, &f, "restart on failure"));
    // also when it stops by itself with an error code, not only when it crashes
    let f = SERVICE_FAILURE_ACTIONS_FLAG { fFailureActionsOnNonCrashFailures: 1 };
    warnings.extend(config2(&s, SERVICE_CONFIG_FAILURE_ACTIONS_FLAG, &f, "restart on failure"));
    let p = SERVICE_PRESHUTDOWN_INFO { dwPreshutdownTimeout: 60_000 };
    warnings.extend(config2(&s, SERVICE_CONFIG_PRESHUTDOWN_INFO, &p, "shutdown timeout"));

    let start = match i.start {
        StartType::Delayed => "automatic (delayed) start",
        StartType::Auto => "automatic start",
        StartType::Manual => "manual start",
    };
    println!("* installed the service {NAME} (\"{DISPLAY_NAME}\"), {start}, as {}", i.account.name());
    println!("* command line: {image}");
    let who = match i.account {
        Account::LocalSystem => "SYSTEM and Administrators".to_owned(),
        a => format!("SYSTEM, Administrators and {}", a.name()),
    };
    let how = if created { "created" } else { "existed; its permissions were reset" };
    println!("* state: {} ({how}; only {who} can open it)", i.state.display());
    match &i.default_save {
        Some(d) => println!("* downloads: {}", d.display()),
        None if i.account != Account::LocalSystem => {
            println!("* downloads: --save; {} needs to write there (icacls ... /grant)", i.account.name())
        }
        None => {}
    }
    println!("* log: {} (open it from an administrator prompt)", i.log.display());
    for w in &warnings {
        println!("! {w}");
    }
    if i.password_in_args {
        println!(
            "! --webui-pass is part of the service's command line, which every local user can read \
             (sc qc {NAME}); set the password in the Web UI instead and install again without it"
        );
    } else if i.webui {
        println!("* without a stored Web UI password the log shows a temporary one at each start");
    }
    if !i.webui {
        println!("! no --webui: nothing can add or control torrents while it runs");
    }
    let profile = std::env::var_os("USERPROFILE").filter(|_| i.account != Account::LocalSystem);
    if profile.is_some_and(|p| inside(&exe, Path::new(&p))) {
        println!(
            "! {} is in your user profile, which {} cannot open: copy the exe to e.g. \
             C:\\Program Files\\PaTorrent and install from there",
            exe.display(),
            i.account.name()
        );
    }
    println!("* start it with: patorrent-nox service start (or sc start {NAME})");
    Ok(())
}

fn uninstall() -> Result<(), String> {
    let s =
        open(DELETE | SERVICE_STOP | SERVICE_QUERY_STATUS).map_err(|e| scm_error("service uninstall", e))?;
    let st = query(&s).map_err(|e| scm_error("service uninstall", e))?;
    if st.dwCurrentState != SERVICE_STOPPED {
        println!("* stopping {NAME}");
        stop_and_wait(&s)?;
    }
    // SAFETY: an open handle with DELETE
    if unsafe { DeleteService(s.0) } == 0 {
        return Err(scm_error("service uninstall", io::Error::last_os_error()));
    }
    println!("* removed the service {NAME}; its state directory and log are left in place");
    Ok(())
}

fn start() -> Result<(), String> {
    let s = open(SERVICE_START | SERVICE_QUERY_STATUS).map_err(|e| scm_error("service start", e))?;
    // SAFETY: an open handle with SERVICE_START; no service arguments
    if unsafe { StartServiceW(s.0, 0, std::ptr::null()) } == 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_SERVICE_ALREADY_RUNNING as i32) {
            println!("* {NAME} is running already");
            return Ok(());
        }
        return Err(scm_error("service start", e));
    }
    println!("* starting {NAME}");
    let st = wait_while(&s, SERVICE_START_PENDING, Duration::from_secs(150))?;
    match st.dwCurrentState {
        SERVICE_RUNNING => {
            println!("* {NAME} is running");
            Ok(())
        }
        SERVICE_STOPPED => Err(format!(
            "{NAME} stopped while starting: {}",
            exit_text(st.dwWin32ExitCode, st.dwServiceSpecificExitCode)
        )),
        other => Err(format!("{NAME} is {}", state_name(other))),
    }
}

fn stop() -> Result<(), String> {
    let s = open(SERVICE_STOP | SERVICE_QUERY_STATUS).map_err(|e| scm_error("service stop", e))?;
    stop_and_wait(&s)
}

fn stop_and_wait(s: &Sc) -> Result<(), String> {
    // SAFETY: zeroes are a valid SERVICE_STATUS
    let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: an open handle with SERVICE_STOP and a valid out pointer
    if unsafe { ControlService(s.0, SERVICE_CONTROL_STOP, &mut st) } == 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_SERVICE_NOT_ACTIVE as i32) {
            println!("* {NAME} is not running");
            return Ok(());
        }
        return Err(scm_error("service stop", e));
    }
    println!("* stopping {NAME} (resume data is saved, port mappings deleted)");
    let st = wait_while(s, SERVICE_STOP_PENDING, Duration::from_secs(90))?;
    println!(
        "* {NAME} is {} ({})",
        state_name(st.dwCurrentState),
        exit_text(st.dwWin32ExitCode, st.dwServiceSpecificExitCode)
    );
    Ok(())
}

// ---- the service's status ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    StartPending,
    Running,
    StopPending,
    Stopped,
}

/// One status report, as `SetServiceStatus` takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Status {
    state: State,
    /// `SERVICE_ACCEPT_*`
    accepts: u32,
    /// The service-specific exit code; 0 is a clean exit.
    exit: u32,
    checkpoint: u32,
    wait_hint_ms: u32,
}

/// The controls a running service takes.
const RUNNING_ACCEPTS: u32 =
    SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_PRESHUTDOWN | SERVICE_ACCEPT_PARAMCHANGE;
/// How long the SCM waits for the next checkpoint.
const WAIT_HINT_MS: u32 = 5_000;
/// How often a pending state counts a checkpoint, and for how many at most: a start or stop
/// that hangs longer than that stops counting, and the SCM sees it as hung.
const HEARTBEAT: Duration = Duration::from_secs(1);
const MAX_TICKS: u32 = 120;

impl Status {
    fn describe(&self) -> String {
        match self.state {
            State::StartPending => "START_PENDING".into(),
            State::Running => "RUNNING".into(),
            State::StopPending => "STOP_PENDING".into(),
            State::Stopped => format!("STOPPED (exit {})", self.exit),
        }
    }

    fn to_win(self) -> SERVICE_STATUS {
        SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: match self.state {
                State::StartPending => SERVICE_START_PENDING,
                State::Running => SERVICE_RUNNING,
                State::StopPending => SERVICE_STOP_PENDING,
                State::Stopped => SERVICE_STOPPED,
            },
            dwControlsAccepted: self.accepts,
            dwWin32ExitCode: if self.exit == 0 { NO_ERROR } else { ERROR_SERVICE_SPECIFIC_ERROR },
            dwServiceSpecificExitCode: self.exit,
            dwCheckPoint: self.checkpoint,
            dwWaitHint: self.wait_hint_ms,
        }
    }
}

/// The status as the service moves through its life; each step returns the report to send,
/// or `None` when it does not apply (a second `READY=1` after a reload, a stop while stopping).
#[derive(Debug)]
struct Machine {
    status: Status,
    ticks: u32,
}

impl Machine {
    fn new() -> Machine {
        Machine { status: Machine::pending(State::StartPending), ticks: 0 }
    }

    fn pending(state: State) -> Status {
        Status { state, accepts: 0, exit: 0, checkpoint: 1, wait_hint_ms: WAIT_HINT_MS }
    }

    /// `READY=1`: the session runs and the Web UI listens.
    fn running(&mut self) -> Option<Status> {
        (self.status.state == State::StartPending).then(|| {
            self.status = Status { state: State::Running, accepts: RUNNING_ACCEPTS, ..Status::stopped(0) };
            self.status
        })
    }

    /// A stop control or `STOPPING=1`.
    fn stopping(&mut self) -> Option<Status> {
        matches!(self.status.state, State::StartPending | State::Running).then(|| {
            self.status = Machine::pending(State::StopPending);
            self.ticks = 0;
            self.status
        })
    }

    /// The heartbeat: the next checkpoint while pending, up to [`MAX_TICKS`].
    fn tick(&mut self) -> Option<Status> {
        let pending = matches!(self.status.state, State::StartPending | State::StopPending);
        (pending && self.ticks < MAX_TICKS).then(|| {
            self.ticks += 1;
            self.status.checkpoint += 1;
            self.status
        })
    }

    fn stopped(&mut self, exit: u32) -> Option<Status> {
        (self.status.state != State::Stopped).then(|| {
            self.status = Status::stopped(exit);
            self.status
        })
    }
}

impl Status {
    fn stopped(exit: u32) -> Status {
        Status { state: State::Stopped, accepts: 0, exit, checkpoint: 0, wait_hint_ms: 0 }
    }
}

/// Where status reports go: `SetServiceStatus`, nowhere (`service debug`), or a test's list.
trait Scm: Send + Sync {
    fn set(&self, s: &Status);
}

/// The SCM's handle for this service.
struct ScmHandle(SERVICE_STATUS_HANDLE);

// SAFETY: the status handle is a plain value that SetServiceStatus takes from any thread
unsafe impl Send for ScmHandle {}
// SAFETY: as above
unsafe impl Sync for ScmHandle {}

impl Scm for ScmHandle {
    fn set(&self, s: &Status) {
        // SAFETY: the handle RegisterServiceCtrlHandlerExW returned (never closed) and a
        // valid status
        unsafe { windows_sys::Win32::System::Services::SetServiceStatus(self.0, &s.to_win()) };
    }
}

struct NoScm;

impl Scm for NoScm {
    fn set(&self, _: &Status) {}
}

/// The status machine and where its reports go; reached from the control handler, the
/// heartbeat thread and `daemon`'s notifications.
struct Reporter {
    scm: Box<dyn Scm>,
    machine: Mutex<Machine>,
    /// Print "STOPPED" too. Not in a service: by then the log file handle that stdout points at
    /// is closed.
    print_stopped: bool,
}

/// Whether a control is a request for the alert loop or answered here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Request(ServiceControl),
    Interrogate,
    NotImplemented,
}

fn classify(control: u32) -> Control {
    match control {
        SERVICE_CONTROL_STOP => Control::Request(ServiceControl::Stop),
        SERVICE_CONTROL_SHUTDOWN | SERVICE_CONTROL_PRESHUTDOWN => Control::Request(ServiceControl::Shutdown),
        SERVICE_CONTROL_PARAMCHANGE => Control::Request(ServiceControl::ParamChange),
        SERVICE_CONTROL_INTERROGATE => Control::Interrogate,
        _ => Control::NotImplemented,
    }
}

impl Reporter {
    fn new(scm: Box<dyn Scm>, print_stopped: bool) -> Reporter {
        Reporter { scm, machine: Mutex::new(Machine::new()), print_stopped }
    }

    /// Applies a step and reports its status (under the lock, so reports stay in order).
    fn apply(&self, step: impl FnOnce(&mut Machine) -> Option<Status>) {
        let mut m = self.machine.lock().unwrap_or_else(|e| e.into_inner());
        let before = m.status.state;
        let Some(s) = step(&mut m) else { return };
        self.scm.set(&s);
        if s.state != before && (s.state != State::Stopped || self.print_stopped) {
            println!("* service: {}", s.describe());
        }
    }

    /// Sends the current status again (the first START_PENDING, INTERROGATE).
    fn report(&self) {
        let m = self.machine.lock().unwrap_or_else(|e| e.into_inner());
        self.scm.set(&m.status);
    }

    /// The heartbeat; false once stopped.
    fn tick(&self) -> bool {
        let mut m = self.machine.lock().unwrap_or_else(|e| e.into_inner());
        if m.status.state == State::Stopped {
            return false;
        }
        if let Some(s) = m.tick() {
            self.scm.set(&s);
        }
        true
    }

    /// `daemon`'s systemd notifications: `READY=1` is RUNNING, `STOPPING=1` STOP_PENDING.
    fn notify(&self, msg: &str) {
        for line in msg.lines() {
            match line {
                "READY=1" => self.apply(Machine::running),
                "STOPPING=1" => self.apply(Machine::stopping),
                _ => {}
            }
        }
    }

    /// A control from the SCM: the handler's answer, and the request to pass on. A stop shows
    /// STOP_PENDING at once, before the alert loop gets to it.
    fn control(&self, control: u32) -> (u32, Option<ServiceControl>) {
        match classify(control) {
            Control::Request(r) => {
                if r != ServiceControl::ParamChange {
                    self.apply(Machine::stopping);
                }
                (NO_ERROR, Some(r))
            }
            Control::Interrogate => {
                self.report();
                (NO_ERROR, None)
            }
            Control::NotImplemented => (ERROR_CALL_NOT_IMPLEMENTED, None),
        }
    }
}

static REPORTER: OnceLock<Reporter> = OnceLock::new();

/// `daemon`'s notification points; true when this process is a service (or `service debug`).
pub fn notify(msg: &str) -> bool {
    REPORTER.get().map(|r| r.notify(msg)).is_some()
}

// ---- the service's runtime ----

/// `service run`'s options, for [`service_main`].
static ARGS: OnceLock<Vec<String>> = OnceLock::new();
/// [`service_main`]'s exit code, for the process's.
static EXIT: AtomicU32 = AtomicU32::new(0);

/// `service run`: hands the main thread to the SCM, which calls [`service_main`] on another one
/// and returns once the service has stopped.
fn dispatch(args: &[String]) -> ExitCode {
    let _ = ARGS.set(args.to_vec());
    let mut name = wide(NAME);
    let table = [
        SERVICE_TABLE_ENTRYW { lpServiceName: name.as_mut_ptr(), lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW { lpServiceName: std::ptr::null_mut(), lpServiceProc: None },
    ];
    // SAFETY: a table ended by a null entry, its strings alive until the call returns
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT as i32) {
            eprintln!(
                "error: {e}\n\n`service run` is what the Service Control Manager starts. Install the \
                 service with `patorrent-nox service install [OPTIONS]` and start it with `patorrent-nox \
                 service start`; to run in this console use `patorrent-nox run [OPTIONS]`, or \
                 `patorrent-nox service debug [OPTIONS]` to watch the service's status."
            );
        } else {
            eprintln!("error: StartServiceCtrlDispatcher: {e}");
        }
        return ExitCode::FAILURE;
    }
    if EXIT.load(Ordering::SeqCst) == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let name = wide(NAME);
    // SAFETY: a NUL-terminated name and a handler with the documented signature
    let h = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(on_control), std::ptr::null()) };
    if h.is_null() {
        EXIT.store(EXIT_FAILED, Ordering::SeqCst);
        return;
    }
    let args = ARGS.get().cloned().unwrap_or_default();
    EXIT.store(serve(Box::new(ScmHandle(h)), &args, true), Ordering::SeqCst);
}

/// The SCM's control handler: answers at once, the alert loop does the work.
unsafe extern "system" fn on_control(
    control: u32,
    _event: u32,
    _data: *mut c_void,
    _ctx: *mut c_void,
) -> u32 {
    let Some(r) = REPORTER.get() else {
        return if control == SERVICE_CONTROL_INTERROGATE { NO_ERROR } else { ERROR_CALL_NOT_IMPLEMENTED };
    };
    let (answer, request) = r.control(control);
    if let Some(req) = request {
        signals::service_control(req);
    }
    answer
}

/// The service's body (also `service debug`'s): START_PENDING, `run`, STOPPED. Returns the
/// service-specific exit code.
fn serve(scm: Box<dyn Scm>, args: &[String], service: bool) -> u32 {
    let r = REPORTER.get_or_init(|| Reporter::new(scm, !service));
    r.report();
    println!("* service: {}", Machine::new().status.describe());
    if let Err(e) = std::thread::Builder::new().name("service status".into()).spawn(move || {
        while r.tick() {
            std::thread::sleep(HEARTBEAT);
        }
    }) {
        eprintln!("! service status thread: {e}");
    }
    signals::set_service();
    let exit = match prepare(args, service) {
        Ok(o) => {
            if crate::run(o, crate::Mode::Run) == ExitCode::SUCCESS {
                0
            } else {
                EXIT_FAILED
            }
        }
        Err((code, e)) => {
            eprintln!("error: {e}");
            code
        }
    };
    r.apply(|m| m.stopped(exit));
    exit
}

/// `run`'s options for the service: `--log` defaults to `nox.log` in the state directory (a
/// service has no console), and the log must open.
fn prepare(args: &[String], service: bool) -> Result<crate::Opts, (u32, String)> {
    let mut o = crate::parse(args).map_err(|e| (EXIT_ARGS, e))?;
    if service && o.service.log.is_none() {
        let state = crate::paths::resolve(o.state.clone(), o.config_dir.clone())
            .state
            .or_else(|| program_data().map(|d| d.join("PaTorrent").join("nox")));
        o.service.log = state.map(|s| s.join(LOG_FILE));
    }
    if let Some(log) = &o.service.log {
        let opened = log
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::File::options().create(true).append(true).open(log));
        opened.map_err(|e| (EXIT_LOG, format!("--log {}: {e}", log.display())))?;
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn abs(p: &str) -> String {
        std::path::absolute(p).unwrap().to_string_lossy().into_owned()
    }

    /// Splits a command line with Windows's own parser.
    fn split(cmdline: &str) -> Vec<String> {
        use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
        let w = wide(cmdline);
        let mut n = 0;
        // SAFETY: a NUL-terminated string and a valid count pointer
        let argv = unsafe { CommandLineToArgvW(w.as_ptr(), &mut n) };
        assert!(!argv.is_null());
        let out = (0..n as usize)
            .map(|i| {
                // SAFETY: `n` NUL-terminated strings
                let p = unsafe { *argv.add(i) };
                let len = (0..).take_while(|&k| unsafe { *p.add(k) } != 0).count();
                // SAFETY: `len` u16s before the NUL
                String::from_utf16(unsafe { std::slice::from_raw_parts(p, len) }).unwrap()
            })
            .collect();
        // SAFETY: CommandLineToArgvW's one allocation
        unsafe { LocalFree(argv.cast()) };
        out
    }

    #[test]
    fn arguments_survive_the_image_path() {
        let exe = Path::new(r"C:\Program Files\PaTorrent\patorrent-nox.exe");
        let tricky = args(&[
            "service",
            "run",
            "plain",
            "",
            "two words",
            r"C:\Program Files\x\",
            r"D:\a b\\",
            r#"say "hi""#,
            r#"back\"slash"#,
            r"\\server\share",
            "tab\there",
            "unicode é ✓",
            r#"""#,
            r"\",
        ]);
        let line = command_line(exe, &tricky);
        assert!(line.starts_with(r#""C:\Program Files\PaTorrent\patorrent-nox.exe" service run plain "" "#));
        let back = split(&line);
        assert_eq!(back[0], exe.to_string_lossy());
        assert_eq!(back[1..], tricky[..]);
        // nothing quoted that does not need it
        let mut s = String::new();
        quote_arg(r"C:\dir\file.dat", &mut s);
        assert_eq!(s, r"C:\dir\file.dat");
    }

    #[test]
    fn install_defaults_to_program_data() {
        let pd = Some(PathBuf::from(r"C:\ProgramData"));
        let i = parse_install(&args(&["--webui", "8080"]), pd.clone()).unwrap();
        let state = PathBuf::from(r"C:\ProgramData\PaTorrent\nox");
        assert_eq!(
            i,
            Install {
                account: Account::LocalService,
                start: StartType::Delayed,
                args: args(&[
                    "--webui",
                    "8080",
                    "--state-dir",
                    r"C:\ProgramData\PaTorrent\nox",
                    "--save",
                    r"C:\ProgramData\PaTorrent\nox\Downloads",
                ]),
                state: state.clone(),
                default_save: Some(state.join("Downloads")),
                log: state.join("nox.log"),
                password_in_args: false,
                webui: true,
            }
        );
        // no %ProgramData%: --state-dir is needed
        let e = parse_install(&[], None).unwrap_err();
        assert!(e.contains("--state-dir"), "{e}");
        assert!(parse_install(&args(&["--state-dir", r"D:\s"]), None).is_ok());
    }

    #[test]
    fn install_makes_paths_absolute_and_keeps_the_rest() {
        let i = parse_install(
            &args(&[
                "--account",
                r"nt authority\NetworkService",
                "--save",
                "dl",
                "--state",
                "st",
                "--ip-filter",
                r"..\f.dat",
                "--log",
                "x.log",
                "--webui",
                "0.0.0.0:8080",
                "--webui-pass",
                "a b",
                "--start",
                "manual",
                "--seed",
                "-v",
                "--dl-limit",
                "100",
            ]),
            None,
        )
        .unwrap();
        assert_eq!((i.account, i.start), (Account::NetworkService, StartType::Manual));
        assert_eq!(
            i.args,
            args(&[
                "--save",
                &abs("dl"),
                "--state",
                &abs("st"),
                "--ip-filter",
                &abs(r"..\f.dat"),
                "--log",
                &abs("x.log"),
                "--webui",
                "0.0.0.0:8080",
                "--webui-pass",
                "a b",
                "--seed",
                "-v",
                "--dl-limit",
                "100",
            ])
        );
        assert_eq!((i.state, i.default_save, i.log), (abs("st").into(), None, abs("x.log").into()));
        assert!(i.password_in_args && i.webui);

        // the image path gives `run` the same options back
        let exe = Path::new(r"C:\pa\patorrent-nox.exe");
        let mut full = args(&["service", "run"]);
        full.extend(i.args.iter().cloned());
        let back = split(&command_line(exe, &full));
        assert_eq!(back[1..3], ["service", "run"]);
        let o = crate::parse(&back[3..]).unwrap();
        assert_eq!(o.save, abs("dl"));
        assert_eq!(o.state, Some(abs("st").into()));
        assert_eq!(o.webui_pass.as_deref(), Some("a b"));
        assert_eq!(o.dl_limit, 100 * 1024);
        assert!(o.seed && o.verbose);
    }

    #[test]
    fn install_refuses_what_a_service_cannot_use() {
        let pd = || Some(PathBuf::from(r"C:\ProgramData"));
        for (a, why) in [
            (&["a.torrent"][..], "added again"),
            (&["--daemon"], "Unix"),
            (&["--no-state"], "--no-state"),
            (&["--account", "bob"], "LocalService"),
            (&["--account"], "--account needs a value"),
            (&["--start", "sometimes"], "delayed"),
            (&["--bogus"], "unknown option --bogus"),
            (&["--save"], "--save needs a value"),
            (&["--webui", "nope"], "nope"),
        ] {
            let e = parse_install(&args(a), pd()).unwrap_err();
            assert!(e.contains(why), "{a:?}: {e}");
        }
    }

    #[test]
    fn value_options_are_asked_of_run() {
        for o in PATH_OPTIONS.iter().chain(&["--webui", "--webui-pass", "--port", "--proxy"]) {
            assert!(takes_value(o), "{o}");
        }
        for o in ["--seed", "--verbose", "-v", "--daemon", "--no-state", "--bogus", "x"] {
            assert!(!takes_value(o), "{o}");
        }
    }

    #[test]
    fn accounts_and_acls() {
        assert_eq!(Account::parse("LocalService"), Ok(Account::LocalService));
        assert_eq!(Account::parse(r"NT AUTHORITY\LOCALSERVICE"), Ok(Account::LocalService));
        assert_eq!(Account::parse("system"), Ok(Account::LocalSystem));
        assert_eq!(
            Account::LocalService.state_dir_sddl(),
            "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;LS)"
        );
        assert_eq!(Account::LocalSystem.state_dir_sddl(), "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
        assert!(inside(Path::new(r"C:\Users\X\bin\nox.exe"), Path::new(r"c:\users\x")));
        assert!(!inside(Path::new(r"C:\Users\Xy\nox.exe"), Path::new(r"C:\Users\X")));
    }

    #[test]
    fn a_directory_gets_its_acl() {
        let base = std::env::temp_dir().join(format!("nox-service-acl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let d = base.join("state");
        // the install's ACL, plus full control for the owner (this user) so the test can clean up
        let sddl = format!("{}(A;OICI;FA;;;OW)", Account::LocalService.state_dir_sddl());
        create_dir_with_acl(&d, &sddl).unwrap();
        std::fs::write(d.join("f"), "x").unwrap();
        assert!(create_dir_with_acl(&d, "D:P(A;OICI;FA;;;SY)").is_err(), "exists");
        assert!(create_dir_with_acl(&base.join("bad"), "not sddl").is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The DACL of `path` as SDDL.
    fn dacl_of(path: &Path) -> String {
        use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
        let name = wide(path);
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: a NUL-terminated path and out pointers
        let err = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut sd,
            )
        };
        assert_eq!(err, NO_ERROR);
        let mut s: PWSTR = std::ptr::null_mut();
        // SAFETY: the descriptor read above and an out pointer; both freed once read
        unsafe {
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut s,
                std::ptr::null_mut(),
            );
            assert_ne!(ok, 0);
            let len = (0..).take_while(|&i| *s.add(i) != 0).count();
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(s, len));
            LocalFree(s.cast());
            LocalFree(sd);
            text
        }
    }

    /// A state directory made by another user before the install is refused (SEC-23).
    #[test]
    fn a_state_dir_from_another_user_is_refused() {
        let base = std::env::temp_dir().join(format!("nox-service-owner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let state = base.join("nox");
        std::fs::create_dir_all(state.join("sub")).unwrap();
        let planted = state.join("sub").join("webui.json");
        std::fs::write(&planted, "{}").unwrap();
        let me = current_user().unwrap();
        let trusted = |sid: &str| TRUSTED_OWNERS.contains(&sid) || sid == me;
        let sddl = format!("{}(A;OICI;FA;;;OW)", Account::LocalService.state_dir_sddl());
        // everything is this user's: taken, not created
        assert_eq!(prepare_state_dir(&state, &sddl, &trusted, &owner_of), Ok(false));
        // a planted file, the directory itself, or a parent that belongs to another user
        let other = "S-1-5-21-1-2-3-1001";
        for bad in [&planted, &state, &base] {
            let fake = |p: &Path| if p == bad.as_path() { Ok(Some(other.to_owned())) } else { owner_of(p) };
            let e = prepare_state_dir(&state, &sddl, &trusted, &fake).unwrap_err();
            assert!(e.contains(other) && e.contains("another user"), "{}: {e}", bad.display());
        }
        let ownerless = |p: &Path| if p == state.as_path() { Ok(None) } else { owner_of(p) };
        assert!(prepare_state_dir(&state, &sddl, &trusted, &ownerless).unwrap_err().contains("no owner"));
        // a Downloads junction that points elsewhere
        let target = base.join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let link = state.join("Downloads");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink: {made:?}");
        let e = prepare_state_dir(&state, &sddl, &trusted, &owner_of).unwrap_err();
        assert!(e.contains("junction"), "{e}");
        std::fs::remove_dir(&link).unwrap();
        // the state directory itself as a junction
        let e = prepare_state_dir(&link, &sddl, &trusted, &owner_of);
        assert_eq!(e, Ok(true), "a missing directory is created");
        std::fs::remove_dir(&link).unwrap();
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success());
        assert!(prepare_state_dir(&link, &sddl, &trusted, &owner_of).unwrap_err().contains("junction"));
        std::fs::remove_dir(&link).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A state directory that is taken loses what it allowed before (SEC-23).
    #[test]
    fn an_existing_state_dir_gets_the_protected_acl() {
        let base = std::env::temp_dir().join(format!("nox-service-reset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let state = base.join("nox");
        std::fs::create_dir_all(&state).unwrap();
        // made before the install, open to every user, with a file inside
        secure_dir(&state, "D:(A;OICI;FA;;;BU)(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)").unwrap();
        std::fs::write(state.join("webui.json"), "{}").unwrap();
        assert!(dacl_of(&state.join("webui.json")).contains(";BU)"));
        let me = current_user().unwrap();
        let trusted = |sid: &str| TRUSTED_OWNERS.contains(&sid) || sid == me;
        let sddl = format!("{}(A;OICI;FA;;;OW)", Account::LocalService.state_dir_sddl());
        assert_eq!(prepare_state_dir(&state, &sddl, &trusted, &owner_of), Ok(false));
        let dir = dacl_of(&state);
        assert!(dir.starts_with("D:P") && !dir.contains(";BU)"), "{dir}");
        assert!(dir.contains(";LS)"), "the service account can write it: {dir}");
        let file = dacl_of(&state.join("webui.json"));
        assert!(!file.contains(";BU)") && file.contains(";LS)"), "{file}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Records the reports.
    #[derive(Default)]
    struct Recorder(std::sync::Arc<Mutex<Vec<Status>>>);

    impl Scm for Recorder {
        fn set(&self, s: &Status) {
            self.0.lock().unwrap().push(*s);
        }
    }

    fn reporter() -> (Reporter, std::sync::Arc<Mutex<Vec<Status>>>) {
        let rec = Recorder::default();
        let seen = rec.0.clone();
        (Reporter::new(Box::new(rec), true), seen)
    }

    fn states(seen: &Mutex<Vec<Status>>) -> Vec<(State, u32, u32)> {
        seen.lock().unwrap().drain(..).map(|s| (s.state, s.checkpoint, s.accepts)).collect()
    }

    #[test]
    fn status_follows_the_daemon_notifications() {
        use State::*;
        let (r, seen) = reporter();
        r.report();
        assert!(r.tick() && r.tick());
        // a reload before ready is not a state change
        r.notify("RELOADING=1");
        r.notify("READY=1\nSTATUS=running, Web UI on http://127.0.0.1:8080/");
        assert!(r.tick());
        // the reload's READY=1 changes nothing
        r.notify("RELOADING=1");
        r.notify("READY=1");
        r.notify("STOPPING=1");
        assert!(r.tick());
        r.apply(|m| m.stopped(0));
        assert!(!r.tick());
        r.apply(|m| m.stopped(1));
        assert_eq!(
            states(&seen),
            [
                (StartPending, 1, 0),
                (StartPending, 2, 0),
                (StartPending, 3, 0),
                (Running, 0, RUNNING_ACCEPTS),
                (StopPending, 1, 0),
                (StopPending, 2, 0),
                (Stopped, 0, 0),
            ]
        );
    }

    #[test]
    fn controls() {
        use State::*;
        let (r, seen) = reporter();
        r.notify("READY=1");
        assert_eq!(r.control(SERVICE_CONTROL_INTERROGATE), (NO_ERROR, None));
        assert_eq!(r.control(SERVICE_CONTROL_PARAMCHANGE), (NO_ERROR, Some(ServiceControl::ParamChange)));
        assert_eq!(r.control(0x0000_00ff), (ERROR_CALL_NOT_IMPLEMENTED, None));
        // a stop shows STOP_PENDING at once; STOPPING=1 from the loop afterwards changes nothing
        assert_eq!(r.control(SERVICE_CONTROL_STOP), (NO_ERROR, Some(ServiceControl::Stop)));
        r.notify("STOPPING=1");
        assert_eq!(r.control(SERVICE_CONTROL_PRESHUTDOWN), (NO_ERROR, Some(ServiceControl::Shutdown)));
        assert_eq!(
            states(&seen),
            [(Running, 0, RUNNING_ACCEPTS), (Running, 0, RUNNING_ACCEPTS), (StopPending, 1, 0)]
        );
        assert_eq!(classify(SERVICE_CONTROL_SHUTDOWN), Control::Request(ServiceControl::Shutdown));
    }

    #[test]
    fn pending_checkpoints_stop_counting_at_the_cap() {
        let mut m = Machine::new();
        for i in 0..MAX_TICKS {
            assert_eq!(m.tick().unwrap().checkpoint, i + 2);
        }
        assert_eq!(m.tick(), None);
        // the stop starts a new count
        assert_eq!(m.stopping().unwrap().checkpoint, 1);
        assert_eq!(m.tick().unwrap().checkpoint, 2);
        // a failed start goes straight to STOPPED with its code
        let mut m = Machine::new();
        let s = m.stopped(EXIT_LOG).unwrap();
        assert_eq!((s.state, s.exit), (State::Stopped, EXIT_LOG));
        assert_eq!(m.running(), None);
        assert_eq!(m.stopping(), None);
        let w = s.to_win();
        assert_eq!(
            (w.dwWin32ExitCode, w.dwServiceSpecificExitCode),
            (ERROR_SERVICE_SPECIFIC_ERROR, EXIT_LOG)
        );
        assert_eq!(Status::stopped(0).to_win().dwWin32ExitCode, NO_ERROR);
    }

    #[test]
    fn exit_codes_are_explained() {
        assert_eq!(exit_text(NO_ERROR, 0), "exit code 0");
        assert!(exit_text(ERROR_SERVICE_SPECIFIC_ERROR, EXIT_ARGS).contains("sc qc PaTorrentNox"));
        assert!(exit_text(ERROR_SERVICE_SPECIFIC_ERROR, EXIT_FAILED).contains("nox.log"));
        assert!(scm_error("service install", io::Error::from_raw_os_error(5)).contains("administrator"));
    }

    #[test]
    fn the_service_log_defaults_to_the_state_directory() {
        let base = std::env::temp_dir().join(format!("nox-service-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let state = base.join("state");
        let o = prepare(&args(&["--state-dir", &state.to_string_lossy()]), true).unwrap();
        assert_eq!(o.service.log, Some(state.join("nox.log")));
        assert!(state.join("nox.log").is_file());
        // `service debug` keeps the console
        assert_eq!(
            prepare(&args(&["--state-dir", &state.to_string_lossy()]), false).unwrap().service.log,
            None
        );
        let e = prepare(&args(&["--bogus"]), true).err().unwrap();
        assert_eq!(e.0, EXIT_ARGS);
        let e = prepare(&args(&["--log", &state.join("nox.log").join("x").to_string_lossy()]), true)
            .err()
            .unwrap();
        assert_eq!(e.0, EXIT_LOG);
        std::fs::remove_dir_all(&base).unwrap();
    }
}
