//! Preferences > Web UI: this window serves the Web UI itself (D40), as µTorrent and
//! qBittorrent serve theirs: `patorrent_nox::webui::WebUi` over this window's engine, fed this
//! window's alert batches, with this window's preferences mirrored into it (the labels, the
//! pause marks, the settings) and what a browser changes handed back through [`Host`]. The
//! page edits the `webui.*` keys of `patorrent.ini`; the server starts with the window when
//! enabled and is restarted on Apply. A remote window (File > Connect to Remote) has no engine
//! of its own and serves nothing.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};

use patorrent_core::{SessionHandle, Settings};
use patorrent_frontend::prefs::{Change, Prefs};
use patorrent_frontend::tr;
use patorrent_nox::webui::auth::{DEFAULT_ITERATIONS, PasswordHash};
use patorrent_nox::webui::https::{Certificate, default_names};
use patorrent_nox::webui::{self, Config, Host, Hosted, WebUi};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const DEFAULT_PORT: u16 = 8080;
const DEFAULT_USER: &str = "admin";

/// What the page edits: the `webui.*` keys of the preferences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    pub enabled: bool,
    pub username: String,
    pub port: u16,
    /// Listen on every address (`0.0.0.0`), not only on this computer.
    pub remote: bool,
    /// Serve HTTPS with a self-signed certificate, made on first use and kept in the settings
    /// folder (`webui-cert.pem`, `webui-key.pem`).
    pub https: bool,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            enabled: false,
            username: DEFAULT_USER.into(),
            port: DEFAULT_PORT,
            remote: false,
            https: false,
        }
    }
}

impl Setup {
    /// The stored setup (`webui.enabled`, `webui.username`, `webui.listen`, `webui.https`).
    pub fn load(prefs: &Prefs) -> Setup {
        let listen = prefs.get("webui.listen").and_then(|s| webui::parse_addr(s).ok());
        let username = prefs.get("webui.username").map(str::trim).filter(|u| !u.is_empty());
        Setup {
            enabled: prefs.get_bool("webui.enabled", false),
            username: username.unwrap_or(DEFAULT_USER).to_owned(),
            port: listen.map_or(DEFAULT_PORT, |a| a.port()),
            remote: listen.is_some_and(|a| a.ip().is_unspecified()),
            https: prefs.get_bool("webui.https", false),
        }
    }

    /// Stores the setup, and a new `password` hashed as `webui.password` (the Web UI reads
    /// the hash; the password itself is never kept).
    pub fn store(&self, prefs: &mut Prefs, password: Option<&str>) {
        prefs.set_bool("webui.enabled", self.enabled);
        prefs.set("webui.username", &self.username);
        prefs.set("webui.listen", self.addr());
        prefs.set_bool("webui.https", self.https);
        if let Some(p) = password {
            prefs.set("webui.password", PasswordHash::new(p, DEFAULT_ITERATIONS).to_string());
        }
    }

    /// The address to bind.
    pub fn addr(&self) -> SocketAddr {
        let ip = if self.remote { Ipv4Addr::UNSPECIFIED } else { Ipv4Addr::LOCALHOST };
        SocketAddr::new(ip.into(), self.port)
    }

    /// The page's address on this computer (the running server reports its own).
    #[cfg(test)]
    pub fn url(&self) -> String {
        format!("{}://127.0.0.1:{}/", if self.https { "https" } else { "http" }, self.port)
    }
}

/// Whether a login is stored (`webui.password` holds a hash the Web UI can read).
pub fn has_password(prefs: &Prefs) -> bool {
    prefs.get("webui.password").is_some_and(|h| PasswordHash::parse(h).is_some())
}

/// Where the server stands, for the page's status line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Not enabled.
    #[default]
    Off,
    /// Serving at this address.
    Running(String),
    /// Enabled but not running: why it could not start (the port is in use, the certificate
    /// is bad, no login is stored).
    Failed(String),
    /// A remote window: nothing of its own to serve.
    Unavailable,
}

impl Status {
    pub fn text(&self) -> String {
        match self {
            Status::Off => tr!(WebUiStopped).to_owned(),
            Status::Running(url) => tr!(WebUiRunning, url),
            Status::Failed(error) => tr!(WebUiFailed, error),
            Status::Unavailable => tr!(WebUiRemoteWindow).to_owned(),
        }
    }

    pub fn running(&self) -> bool {
        matches!(self, Status::Running(_))
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            Status::Running(url) => Some(url),
            _ => None,
        }
    }
}

/// What the page shows: the stored setup, whether a login is stored, the server's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub setup: Setup,
    pub has_password: bool,
    pub status: Status,
}

/// The page's Apply: the setup and a new password (`None` keeps the stored one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub setup: Setup,
    pub password: Option<String>,
}

/// The window as the Web UI's host: what a browser changed is queued here and the window
/// told with `WM_APP_WEBUI` (`wparam` 0; 1 asks it to exit), so it is taken on its own
/// thread.
pub struct WindowHost {
    hwnd: isize,
    pending: Mutex<Vec<Change>>,
}

impl WindowHost {
    pub fn new(hwnd: HWND) -> Arc<WindowHost> {
        Arc::new(WindowHost { hwnd: hwnd as isize, pending: Mutex::new(Vec::new()) })
    }

    /// The queued changes, oldest first.
    pub fn take(&self) -> Vec<Change> {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn post(&self, what: usize) {
        // SAFETY: a message to a window of this process; a destroyed window ignores it.
        unsafe { PostMessageW(self.hwnd as HWND, crate::app::WM_APP_WEBUI, what, 0) };
    }
}

impl Host for WindowHost {
    fn options_changed(&self, changes: Vec<Change>) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).extend(changes);
        self.post(0);
    }

    fn shutdown(&self) {
        self.post(1);
    }
}

/// Starts the server for `setup` over `handle`: `prefs` is mirrored into it (the login hash
/// included), `settings` and `save_path` are this window's, `dir` the settings folder (the
/// certificate, `ipfilter.dat`).
pub fn start(
    setup: &Setup,
    prefs: &Prefs,
    settings: Settings,
    save_path: &str,
    dir: &Path,
    handle: SessionHandle,
    host: Arc<WindowHost>,
) -> Result<WebUi, String> {
    if !has_password(prefs) {
        return Err(tr!(WebUiNeedLogin).to_owned());
    }
    let addr = setup.addr();
    let mut c = Config::new(addr, settings, save_path);
    c.username = setup.username.clone();
    c.state_dir = Some(dir.to_path_buf());
    if setup.https {
        c.https = Some(Certificate::Generated { dir: Some(dir.to_path_buf()), names: default_names(addr) });
    }
    c.hosted = Some(Hosted { host, options: prefs.clone() });
    WebUi::start(handle, c).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setup_round_trips_through_the_preferences() {
        let mut p = Prefs::default();
        assert_eq!(Setup::load(&p), Setup::default());
        assert!(!has_password(&p));
        let s = Setup { enabled: true, username: "me".into(), port: 8443, remote: true, https: true };
        s.store(&mut p, Some("s3cret"));
        assert_eq!(p.get("webui.listen"), Some("0.0.0.0:8443"));
        assert_eq!(p.get("webui.enabled"), Some("1"));
        assert!(has_password(&p), "a hash, never the password");
        assert!(!p.get("webui.password").unwrap().contains("s3cret"));
        assert!(PasswordHash::parse(p.get("webui.password").unwrap()).unwrap().verify("s3cret"));
        assert_eq!(Setup::load(&p), s);
        assert_eq!(s.url(), "https://127.0.0.1:8443/");
        // the stored hash stays without a new password; a local setup binds the loopback
        let local = Setup { remote: false, https: false, ..s.clone() };
        local.store(&mut p, None);
        assert!(has_password(&p));
        assert_eq!(p.get("webui.listen"), Some("127.0.0.1:8443"));
        assert_eq!(local.url(), "http://127.0.0.1:8443/");
        assert_eq!(Setup::load(&p), local);
        // a hand-edited file: a blank user name is the default, a bad address the default port
        p.set("webui.username", " ");
        p.set("webui.listen", "nonsense");
        assert_eq!(Setup::load(&p).username, "admin");
        assert_eq!(Setup::load(&p).port, DEFAULT_PORT);
    }

    #[test]
    fn the_status_line() {
        assert_eq!(Status::Off.text(), tr!(WebUiStopped));
        let s = Status::Running("http://127.0.0.1:8080/".into());
        assert!(s.running());
        assert_eq!(s.url(), Some("http://127.0.0.1:8080/"));
        assert_eq!(s.text(), tr!(WebUiRunning, url = "http://127.0.0.1:8080/"));
        assert_eq!(Status::Failed("x".into()).text(), tr!(WebUiFailed, error = "x"));
        assert!(!Status::Unavailable.running() && Status::Unavailable.url().is_none());
    }
}
