//! Remote mode's connect step, shared by the GUIs (PLAN.md Phase 6 and Phase 7 slice 5): what
//! the Connect to Remote dialog asks for and remembers, where a remote window keeps its own
//! files, how a server's certificate is checked, and what the user is told when connecting
//! fails. The dialogs are each GUI's (the Win32 `remote_ui`, the GTK `remote`); the session is
//! `patorrent_core::remote`.
//!
//! The connection details and the accepted certificates live in [`FILE_NAME`] beside the local
//! client's `patorrent.ini`; the password is never stored. A remote window's settings, labels
//! and RSS feeds live in a folder of their own per server ([`data_dir`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use patorrent_core::remote::{self, ConnectError, Target};
use patorrent_core::tls::{self, Fingerprint, Verify};

use crate::prefs::Prefs;
use crate::tr;

/// The connection details and pinned certificates, beside `patorrent.ini`.
pub const FILE_NAME: &str = "remote.ini";

/// How long connecting (and each certificate check) may take before it is given up.
pub const TIMEOUT: Duration = Duration::from_secs(15);

/// What the connect dialog shows and returns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Form {
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub https: bool,
}

impl Form {
    /// The last connection's details (no password).
    pub fn load(prefs: &Prefs) -> Form {
        Form {
            host: prefs.get("remote.host").unwrap_or_default().to_owned(),
            port: prefs.get("remote.port").unwrap_or("8080").to_owned(),
            user: prefs.get("remote.user").unwrap_or("admin").to_owned(),
            password: String::new(),
            // HTTPS unless the user turned it off before (SEC-14)
            https: prefs.get_bool("remote.https", true),
        }
    }

    /// Keeps everything but the password for the next time.
    pub fn store(&self, prefs: &mut Prefs) {
        prefs.set("remote.host", &self.host);
        prefs.set("remote.port", &self.port);
        prefs.set("remote.user", &self.user);
        prefs.set_bool("remote.https", self.https);
    }

    /// Where to connect, with no certificate check chosen yet, and its `host:port` label; `None`
    /// for a missing host or a bad port (`RemoteBadAddress`).
    pub fn target(&self) -> Option<(Target, String)> {
        let port = self.port.trim().parse::<u16>().ok().filter(|&p| p > 0)?;
        let host = self.host.trim().trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() {
            return None;
        }
        let target = Target {
            host: host.to_owned(),
            port,
            tls: None,
            username: self.user.clone(),
            password: self.password.clone(),
            allow_plaintext: false,
        };
        Some((target, label(host, port)))
    }

    /// Whether the user must agree (`RemotePlaintext`) before connecting: plain HTTP to another
    /// computer. On a yes, set [`Target::allow_plaintext`].
    pub fn needs_plaintext_consent(&self, target: &Target) -> bool {
        !self.https && !remote::is_loopback_host(&target.host)
    }
}

/// `host:port`, as the title bar shows it (IPv6 literals in brackets).
pub fn label(host: &str, port: u16) -> String {
    if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

/// The folder a remote window keeps its files in, inside the settings folder `base`.
pub fn data_dir(base: &Path, label: &str) -> PathBuf {
    let name: String =
        label.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' }).collect();
    base.join("remote").join(name)
}

fn pin_key(label: &str) -> String {
    format!("remote.pin.{label}")
}

/// The certificate the user accepted for the server `label`.
pub fn pinned(prefs: &Prefs, label: &str) -> Option<Fingerprint> {
    prefs.get(&pin_key(label)).and_then(Fingerprint::parse)
}

/// Remembers the certificate the user accepted for `label`.
pub fn pin(prefs: &mut Prefs, label: &str, fingerprint: Fingerprint) {
    prefs.set(&pin_key(label), fingerprint);
}

/// What the certificate check of an HTTPS server found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Probe {
    /// The system trusts the certificate for this host name: no question.
    Trusted,
    /// A certificate the system does not trust (a self-signed one): the user compares its
    /// fingerprint with the one nox printed (`RemoteTrust`) and [`pin`]s it.
    Unknown(Fingerprint),
}

/// Reads the certificate of the server at `target` and whether the system trusts it; nothing is
/// sent to the server. Blocks for up to twice [`TIMEOUT`], so GUIs call it off their main thread.
/// The error is the text for the user (`RemoteNoHttps`).
pub fn probe(target: &Target, label: &str) -> Result<Probe, String> {
    let presented = remote::probe_certificate(&target.host, target.port, TIMEOUT)
        .map_err(|e| tr!(RemoteNoHttps, name = label, error = e))?;
    Ok(if trusted(target) { Probe::Trusted } else { Probe::Unknown(presented) })
}

/// Whether a TLS handshake with the system's checks succeeds (nothing is sent after it).
fn trusted(target: &Target) -> bool {
    let Ok(sock) = std::net::TcpStream::connect((target.host.as_str(), target.port)) else { return false };
    let _ = sock.set_read_timeout(Some(TIMEOUT));
    let _ = sock.set_write_timeout(Some(TIMEOUT));
    tls::Stream::connect(sock, &target.host, Verify::System).and_then(|mut s| s.handshake()).is_ok()
}

/// What to tell the user when [`remote::connect`] failed.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub text: String,
    /// The pinned certificate no longer matches: the pin is dropped ([`Failure::apply`]), so the
    /// next attempt shows the new one.
    pub unpin: bool,
}

impl Failure {
    pub fn new(e: &ConnectError, label: &str, https: bool) -> Failure {
        if let ConnectError::Io(io) = e {
            // the TLS layers' text for a pin that does not match
            if io.to_string().contains("not the accepted one") {
                return Failure { text: tr!(RemoteCertChanged, name = label, error = e), unpin: true };
            }
        }
        let hint = match e {
            ConnectError::Refused { status: 404, .. } => Some(tr!(RemoteHintNotNox)),
            ConnectError::Io(_) if !https => Some(tr!(RemoteHintHttps)),
            _ => None,
        };
        let mut text = tr!(RemoteCannotConnect, name = label, error = e);
        if let Some(h) = hint {
            text.push_str("\n\n");
            text.push_str(h);
        }
        Failure { text, unpin: false }
    }

    /// Drops the pin when it no longer matches.
    pub fn apply(&self, prefs: &mut Prefs, label: &str) {
        if self.unpin {
            prefs.remove(&pin_key(label));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_and_labels() {
        let base = Path::new("/home/u/.config/patorrent/gtk");
        assert_eq!(data_dir(base, "nas.lan:8080"), base.join("remote").join("nas.lan_8080"));
        assert_eq!(data_dir(base, "[fe80::1%3]:8443"), base.join("remote").join("_fe80__1_3__8443"));
        assert_eq!(label("::1", 80), "[::1]:80");
        assert_eq!(label("10.0.0.2", 80), "10.0.0.2:80");
    }

    #[test]
    fn the_form_gives_a_target() {
        let form = |host: &str, port: &str| Form {
            host: host.into(),
            port: port.into(),
            user: "admin".into(),
            password: "pw".into(),
            https: false,
        };
        let (t, label) = form(" nas.lan ", "8080").target().unwrap();
        assert_eq!((t.host.as_str(), t.port, label.as_str()), ("nas.lan", 8080, "nas.lan:8080"));
        assert_eq!((t.username.as_str(), t.password.as_str()), ("admin", "pw"));
        assert!(t.tls.is_none());
        let (t, label) = form("[::1]", "8443").target().unwrap();
        assert_eq!((t.host.as_str(), label.as_str()), ("::1", "[::1]:8443"));
        for (host, port) in
            [("", "8080"), ("[]", "8080"), ("nas", ""), ("nas", "0"), ("nas", "65536"), ("nas", "x")]
        {
            assert!(form(host, port).target().is_none(), "{host:?} {port:?}");
        }
    }

    /// SEC-14: HTTPS is the default; plain HTTP to another computer is asked about first.
    #[test]
    fn plain_http_off_this_computer_is_asked_about() {
        let fresh = Form::load(&Prefs::default());
        assert!(fresh.https, "HTTPS by default");
        let mut prefs = Prefs::default();
        Form { https: false, ..fresh.clone() }.store(&mut prefs);
        assert!(!Form::load(&prefs).https, "a choice made before is kept");

        let ask = |host: &str, https: bool| {
            let form = Form { host: host.into(), port: "8080".into(), https, ..Form::default() };
            let (target, _) = form.target().unwrap();
            assert!(!target.allow_plaintext);
            form.needs_plaintext_consent(&target)
        };
        assert!(ask("nas.lan", false));
        assert!(ask("10.0.0.2", false));
        assert!(!ask("nas.lan", true));
        assert!(!ask("localhost", false));
        assert!(!ask("127.0.0.1", false));
        assert!(!ask("[::1]", false));
    }

    #[test]
    fn the_form_and_pins_are_remembered_without_the_password() {
        let dir = std::env::temp_dir().join(format!("pa-remote-form-{}", std::process::id()));
        let path = dir.join(FILE_NAME);
        let mut prefs = Prefs::load(&path);
        assert_eq!(
            Form::load(&prefs),
            Form { port: "8080".into(), user: "admin".into(), https: true, ..Form::default() }
        );
        let form = Form {
            host: "nas.lan".into(),
            port: "8443".into(),
            user: "me".into(),
            password: "secret".into(),
            https: true,
        };
        form.store(&mut prefs);
        let print = Fingerprint([7; 32]);
        pin(&mut prefs, "nas.lan:8443", print);
        std::fs::create_dir_all(&dir).unwrap();
        prefs.save().unwrap();
        let prefs = Prefs::load(&path);
        assert_eq!(Form::load(&prefs), Form { password: String::new(), ..form });
        assert_eq!(pinned(&prefs, "nas.lan:8443"), Some(print));
        assert_eq!(pinned(&prefs, "nas.lan:8080"), None);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("secret"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failures_name_the_server_and_drop_a_changed_pin() {
        let changed = ConnectError::Io(std::io::Error::other(
            "TLS handshake failed: the server's certificate is not the accepted one (it presented ab:cd)",
        ));
        let f = Failure::new(&changed, "nas:8443", true);
        assert!(f.unpin && f.text.contains("nas:8443"), "{f:?}");
        let mut prefs = Prefs::load(std::env::temp_dir().join("pa-remote-never-saved.ini"));
        pin(&mut prefs, "nas:8443", Fingerprint([1; 32]));
        f.apply(&mut prefs, "nas:8443");
        assert_eq!(pinned(&prefs, "nas:8443"), None);

        let refused = ConnectError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionRefused));
        let plain = Failure::new(&refused, "nas:8080", false);
        assert!(!plain.unpin && plain.text.contains(tr!(RemoteHintHttps)), "{plain:?}");
        assert!(!Failure::new(&refused, "nas:8080", true).text.contains(tr!(RemoteHintHttps)));
        let not_nox = ConnectError::Refused { status: 404, message: String::new() };
        assert!(Failure::new(&not_nox, "nas:8080", false).text.contains(tr!(RemoteHintNotNox)));
        let login = ConnectError::Refused { status: 401, message: String::new() };
        let f = Failure::new(&login, "nas:8080", false);
        assert!(f.text.contains("wrong user name or password") && !f.unpin, "{f:?}");
    }
}
