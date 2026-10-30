//! Preferences > Web UI: this window serves the Web UI itself (D40), as µTorrent and
//! qBittorrent serve theirs: `patorrent_nox::webui::WebUi` over this window's engine, fed this
//! window's alert batches, with this window's preferences mirrored into it (the labels, the
//! pause marks, the settings) and what a browser changes handed back through [`Host`]. The
//! page edits the `webui.*` keys of `patorrent.ini`; the server starts with the window when
//! enabled and is restarted on Apply. A remote window (File > Connect to Remote) has no engine
//! of its own and serves nothing.
//!
//! The RSS Downloader is shared, not proxied: the window's own model (`Rss`, behind an
//! `Arc<Mutex<..>>`) is the one the Web UI's RSS endpoints lock and edit (`Hosted::rss`). The
//! window keeps the worker, the downloads and `rss.dat`; a browser's change comes back as
//! `POST_RSS` (to redraw, and with the downloads it asked for, which go through the window's
//! own RSS add path), and the window's own changes go to `WebUi::rss_changed`. Lock order on
//! every thread: the RSS lock, then the Web UI's; the request threads never wait on the
//! window's thread (everything here queues and posts).

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};

use patorrent_core::{SessionHandle, Settings};
use patorrent_frontend::prefs::{Change, Prefs};
use patorrent_frontend::rss::AddRequest;
use patorrent_frontend::tr;
use patorrent_nox::webui::auth::{DEFAULT_ITERATIONS, PasswordHash};
use patorrent_nox::webui::https::{Certificate, default_names};
use patorrent_nox::webui::{self, Config, Host, Hosted, Rss, WebUi};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const DEFAULT_PORT: u16 = 8080;

/// `WM_APP_WEBUI`'s `wparam`: preferences keys a browser changed are queued
/// ([`WindowHost::take`]).
pub const POST_CHANGES: usize = 0;
/// `WM_APP_WEBUI`'s `wparam`: a browser asked the window to exit.
pub const POST_SHUTDOWN: usize = 1;
/// `WM_APP_WEBUI`'s `wparam`: a browser changed the RSS Downloader, maybe with downloads
/// queued ([`WindowHost::take_rss_downloads`]).
pub const POST_RSS: usize = 2;
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
    /// The host names the Host check lets through besides IP addresses and `localhost`
    /// (`webui.domain_list`, `;`-separated, `*` wildcards).
    pub domains: String,
    /// Accept `Authorization: Bearer <key>` with the stored key (`webui.api_key_enabled`; the
    /// key itself is write-only, hashed in `webui.api_key`).
    pub api_key_enabled: bool,
    /// The subnets whose clients skip the login (`webui.bypass_auth_subnets`, `;`-separated).
    pub subnets: String,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            enabled: false,
            username: DEFAULT_USER.into(),
            port: DEFAULT_PORT,
            remote: false,
            https: false,
            domains: String::new(),
            api_key_enabled: false,
            subnets: String::new(),
        }
    }
}

impl Setup {
    /// The stored setup (`webui.enabled`, `webui.username`, `webui.listen`, `webui.https`,
    /// `webui.domain_list`, `webui.api_key_enabled`, `webui.bypass_auth_subnets`).
    pub fn load(prefs: &Prefs) -> Setup {
        let listen = prefs.get("webui.listen").and_then(|s| webui::parse_addr(s).ok());
        let username = prefs.get("webui.username").map(str::trim).filter(|u| !u.is_empty());
        Setup {
            enabled: prefs.get_bool("webui.enabled", false),
            username: username.unwrap_or(DEFAULT_USER).to_owned(),
            port: listen.map_or(DEFAULT_PORT, |a| a.port()),
            remote: listen.is_some_and(|a| a.ip().is_unspecified()),
            https: prefs.get_bool("webui.https", false),
            domains: prefs.get("webui.domain_list").unwrap_or("").trim().to_owned(),
            api_key_enabled: prefs.get_bool("webui.api_key_enabled", false),
            subnets: prefs.get("webui.bypass_auth_subnets").unwrap_or("").trim().to_owned(),
        }
    }

    /// Stores the setup, and a new `password` and `api_key` hashed as `webui.password` and
    /// `webui.api_key` (the Web UI reads the hashes; neither is kept itself).
    pub fn store(&self, prefs: &mut Prefs, password: Option<&str>, api_key: Option<&str>) {
        prefs.set_bool("webui.enabled", self.enabled);
        prefs.set("webui.username", &self.username);
        prefs.set("webui.listen", self.addr());
        prefs.set_bool("webui.https", self.https);
        if self.domains.is_empty() {
            prefs.remove("webui.domain_list");
        } else {
            prefs.set("webui.domain_list", &self.domains);
        }
        prefs.set_bool("webui.api_key_enabled", self.api_key_enabled);
        if self.subnets.is_empty() {
            prefs.remove("webui.bypass_auth_subnets");
        } else {
            prefs.set("webui.bypass_auth_subnets", &self.subnets);
        }
        if let Some(p) = password {
            prefs.set("webui.password", PasswordHash::new(p, DEFAULT_ITERATIONS).to_string());
        }
        if let Some(k) = api_key {
            prefs.set("webui.api_key", PasswordHash::new(k, DEFAULT_ITERATIONS).to_string());
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

/// Whether an API key is stored (`webui.api_key` holds a hash).
pub fn has_api_key(prefs: &Prefs) -> bool {
    prefs.get("webui.api_key").is_some_and(|h| PasswordHash::parse(h).is_some())
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

/// What the page shows: the stored setup, whether a login and an API key are stored, the
/// server's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub setup: Setup,
    pub has_password: bool,
    pub has_api_key: bool,
    pub status: Status,
}

/// The page's Apply: the setup, a new password and a new API key (`None` keeps the stored
/// one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub setup: Setup,
    pub password: Option<String>,
    pub api_key: Option<String>,
}

/// The window as the Web UI's host: what a browser changed is queued here and the window
/// told with `WM_APP_WEBUI` ([`POST_CHANGES`], [`POST_SHUTDOWN`], [`POST_RSS`]), so it is
/// taken on its own thread.
pub struct WindowHost {
    hwnd: isize,
    pending: Mutex<Vec<Change>>,
    rss_downloads: Mutex<Vec<AddRequest>>,
}

impl WindowHost {
    pub fn new(hwnd: HWND) -> Arc<WindowHost> {
        Arc::new(WindowHost {
            hwnd: hwnd as isize,
            pending: Mutex::new(Vec::new()),
            rss_downloads: Mutex::new(Vec::new()),
        })
    }

    /// The queued changes, oldest first.
    pub fn take(&self) -> Vec<Change> {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The RSS downloads a browser asked for, oldest first.
    pub fn take_rss_downloads(&self) -> Vec<AddRequest> {
        std::mem::take(&mut *self.rss_downloads.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn post(&self, what: usize) {
        // SAFETY: a message to a window of this process; a destroyed window ignores it.
        unsafe { PostMessageW(self.hwnd as HWND, crate::app::WM_APP_WEBUI, what, 0) };
    }
}

impl Host for WindowHost {
    fn options_changed(&self, changes: Vec<Change>) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).extend(changes);
        self.post(POST_CHANGES);
    }

    fn shutdown(&self) {
        self.post(POST_SHUTDOWN);
    }

    fn rss_changed(&self) {
        self.post(POST_RSS);
    }

    fn rss_download(&self, reqs: Vec<AddRequest>) {
        self.rss_downloads.lock().unwrap_or_else(|e| e.into_inner()).extend(reqs);
        self.post(POST_RSS);
    }
}

/// Starts the server for `setup` over `handle`: `prefs` is mirrored into it (the login hash
/// included), `settings` and `save_path` are this window's, `dir` the settings folder (the
/// certificate, `ipfilter.dat`), `rss` the window's RSS Downloader, which the server's RSS
/// endpoints edit.
#[allow(clippy::too_many_arguments)]
pub fn start(
    setup: &Setup,
    prefs: &Prefs,
    settings: Settings,
    save_path: &str,
    dir: &Path,
    handle: SessionHandle,
    host: Arc<WindowHost>,
    rss: Arc<Mutex<Rss>>,
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
    c.hosted = Some(Hosted { host, options: prefs.clone(), rss });
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
        let s = Setup {
            enabled: true,
            username: "me".into(),
            port: 8443,
            remote: true,
            https: true,
            domains: "patorrent;*.lan".into(),
            api_key_enabled: true,
            subnets: "192.168.1.0/24;fd00::/8".into(),
        };
        assert!(!has_api_key(&p));
        s.store(&mut p, Some("s3cret"), Some("0123456789abcdef"));
        assert_eq!(p.get("webui.listen"), Some("0.0.0.0:8443"));
        assert_eq!(p.get("webui.domain_list"), Some("patorrent;*.lan"));
        assert_eq!(p.get("webui.enabled"), Some("1"));
        assert_eq!(p.get("webui.api_key_enabled"), Some("1"));
        assert_eq!(p.get("webui.bypass_auth_subnets"), Some("192.168.1.0/24;fd00::/8"));
        assert!(has_password(&p), "a hash, never the password");
        assert!(!p.get("webui.password").unwrap().contains("s3cret"));
        assert!(PasswordHash::parse(p.get("webui.password").unwrap()).unwrap().verify("s3cret"));
        // the key is hashed as the password is (the Web UI reads it so)
        assert!(has_api_key(&p));
        assert!(!p.get("webui.api_key").unwrap().contains("0123456789abcdef"));
        assert!(PasswordHash::parse(p.get("webui.api_key").unwrap()).unwrap().verify("0123456789abcdef"));
        assert_eq!(Setup::load(&p), s);
        assert_eq!(s.url(), "https://127.0.0.1:8443/");
        // the stored hashes stay without new ones; a local setup binds the loopback
        let local = Setup {
            remote: false,
            https: false,
            domains: String::new(),
            api_key_enabled: false,
            subnets: String::new(),
            ..s.clone()
        };
        local.store(&mut p, None, None);
        assert_eq!(p.get("webui.domain_list"), None);
        assert_eq!(p.get("webui.bypass_auth_subnets"), None);
        assert_eq!(p.get("webui.api_key_enabled"), Some("0"));
        assert!(has_password(&p) && has_api_key(&p));
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
    fn a_browsers_rss_downloads_wait_for_the_window_in_order() {
        // no window: the posts go nowhere
        let host = WindowHost::new(std::ptr::null_mut());
        let req = |title: &str| AddRequest {
            feed: 1,
            filter: None,
            item_id: title.into(),
            title: title.into(),
            url: format!("magnet:?dn={title}"),
            cookies: String::new(),
            feed_url: "http://feed/".into(),
            save_path: String::new(),
            label: String::new(),
            top_priority: false,
            paused: false,
        };
        host.rss_changed();
        assert!(host.take_rss_downloads().is_empty());
        host.rss_download(vec![req("a"), req("b")]);
        host.rss_download(vec![req("c")]);
        let titles: Vec<String> = host.take_rss_downloads().into_iter().map(|r| r.title).collect();
        assert_eq!(titles, ["a", "b", "c"]);
        assert!(host.take_rss_downloads().is_empty());
        // the preferences queue is its own
        assert!(host.take().is_empty());
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
