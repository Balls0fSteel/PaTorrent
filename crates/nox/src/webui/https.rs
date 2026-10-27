//! HTTPS for the Web UI (PLAN.md Phase 6): the certificate the server presents. Either the
//! user's PEM files (`--webui-cert` / `--webui-key`), or a self-signed certificate made on
//! first use and kept in the config directory (`--webui-https` alone), renewed a month before
//! it expires. Browsers warn about a self-signed certificate until the user accepts it; the
//! fingerprint printed at start lets them check it is this one.

use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use patorrent_core::tls::{self, Identity};

/// The generated certificate, in the config directory.
pub const CERT_FILE: &str = "webui-cert.pem";
/// The generated certificate's private key, beside it.
pub const KEY_FILE: &str = "webui-key.pem";
/// A generated certificate's lifetime: 825 days, the most Apple's platforms accept for a TLS
/// server certificate.
pub const GENERATED_DAYS: u32 = 825;
/// A generated certificate is replaced once less than this is left of it.
const RENEW_BEFORE: i64 = 30 * 86_400;

/// Where the certificate comes from.
#[derive(Clone, Debug)]
pub enum Certificate {
    /// The user's PEM files; `key` may name the certificate file itself (both in one file).
    Files { cert: PathBuf, key: PathBuf },
    /// A self-signed certificate for `names`, kept in `dir` ([`CERT_FILE`], [`KEY_FILE`]);
    /// without a directory (`--no-state`) a new one each start, in memory only.
    Generated { dir: Option<PathBuf>, names: Vec<String> },
}

/// What [`load`] found or made.
pub struct Loaded {
    pub identity: Identity,
    /// The certificate file (none for an in-memory one).
    pub cert_path: Option<PathBuf>,
    /// A new certificate was made: there was none, or the old one was about to expire.
    pub generated: bool,
}

/// Loads the certificate (making or renewing a generated one first). `now` is Unix seconds.
pub fn load(source: &Certificate, now: i64) -> io::Result<Loaded> {
    match source {
        Certificate::Files { cert, key } => Ok(Loaded {
            identity: Identity::load(cert, key)?,
            cert_path: Some(cert.clone()),
            generated: false,
        }),
        Certificate::Generated { dir: None, names } => {
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let pem = tls::generate_self_signed(&names, GENERATED_DAYS)?;
            let identity = Identity::from_pem(pem.cert_pem.as_bytes(), pem.key_pem.as_bytes())?;
            Ok(Loaded { identity, cert_path: None, generated: true })
        }
        Certificate::Generated { dir: Some(dir), names } => {
            let (cert, key) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
            let current = match std::fs::read(&cert) {
                Ok(text) if key.exists() => {
                    let info = tls::certificate_info(&text).map_err(|e| regenerate_hint(&cert, &key, e))?;
                    info.not_after - now > RENEW_BEFORE
                }
                Ok(_) => false,
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", cert.display()))),
            };
            if !current {
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                let pem = tls::generate_self_signed(&names, GENERATED_DAYS)?;
                patorrent_core::util::private_file::create_dir_all(dir)?;
                write_file(&key, pem.key_pem.as_bytes(), true)?;
                write_file(&cert, pem.cert_pem.as_bytes(), false)?;
            }
            let identity = Identity::load(&cert, &key).map_err(|e| regenerate_hint(&cert, &key, e))?;
            Ok(Loaded { identity, cert_path: Some(cert), generated: !current })
        }
    }
}

fn regenerate_hint(cert: &Path, key: &Path, e: io::Error) -> io::Error {
    io::Error::new(
        e.kind(),
        format!("{e} (delete {} and {} to make a new certificate)", cert.display(), key.display()),
    )
}

/// Writes through a temporary file and a rename, so a crash leaves the old file or the new one.
/// A private file (the key) is readable by its owner only on Unix; on Windows it inherits the
/// config directory's permissions, which are the user's own.
fn write_file(path: &Path, data: &[u8], private: bool) -> io::Result<()> {
    let tmp = path.with_extension("pem.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(patorrent_core::util::private_file::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    let _ = private;
    let named = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display()));
    let mut f = options.open(&tmp).map_err(named)?;
    #[cfg(unix)]
    if private {
        // the mode above applies only to a new file, not to one a crash left behind
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(named)?;
    }
    f.write_all(data).and_then(|()| f.sync_all()).map_err(named)?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(named)
}

/// The names a generated certificate covers: `localhost` and the loopback addresses, the
/// machine's host name, and the address the Web UI listens on when it is a specific one.
pub fn default_names(listen: SocketAddr) -> Vec<String> {
    let mut names: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    if let Some(h) = host_name() {
        names.push(h);
    }
    let ip = listen.ip();
    if !ip.is_unspecified() && !ip.is_loopback() {
        names.push(ip.to_string());
    }
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| seen.insert(n.to_ascii_lowercase()));
    names
}

/// This machine's host name, when it is a usable DNS name.
fn host_name() -> Option<String> {
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok()?;
    #[cfg(unix)]
    let name = {
        unsafe extern "C" {
            fn gethostname(name: *mut std::ffi::c_char, len: usize) -> std::ffi::c_int;
        }
        let mut buf = [0u8; 256];
        // SAFETY: `buf` has room for `buf.len()` bytes; gethostname writes at most that many.
        if unsafe { gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0)?;
        String::from_utf8(buf[..end].to_vec()).ok()?
    };
    #[cfg(not(any(windows, unix)))]
    let name = String::new();
    let name = name.to_ascii_lowercase();
    let ok = !name.is_empty()
        && name.len() <= 253
        && name
            .split('.')
            .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    ok.then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("patorrent-https-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_generated_certificate_is_kept_then_renewed() {
        let dir = scratch("renew");
        let source = Certificate::Generated {
            dir: Some(dir.clone()),
            names: vec!["localhost".into(), "127.0.0.1".into()],
        };
        let now = crate::webui::unix_now();
        let first = load(&source, now).unwrap();
        assert!(first.generated);
        assert_eq!(first.cert_path, Some(dir.join(CERT_FILE)));
        let info = first.identity.info();
        assert!((info.not_after - now - i64::from(GENERATED_DAYS) * 86_400).abs() < 60);
        // the next start keeps it
        let again = load(&source, now + 86_400).unwrap();
        assert!(!again.generated);
        assert_eq!(again.identity.fingerprint(), info.fingerprint);
        // a month before it ends, a new one
        let later = load(&source, info.not_after - RENEW_BEFORE + 1).unwrap();
        assert!(later.generated);
        assert_ne!(later.identity.fingerprint(), info.fingerprint);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(KEY_FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // a missing key makes a new pair; a broken certificate is an error that says what to do
        std::fs::remove_file(dir.join(KEY_FILE)).unwrap();
        assert!(load(&source, now).unwrap().generated);
        std::fs::write(dir.join(CERT_FILE), "garbage").unwrap();
        let e = load(&source, now).err().unwrap().to_string();
        assert!(e.contains("delete") && e.contains(CERT_FILE), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
        // without a directory: a new one in memory
        let memory =
            load(&Certificate::Generated { dir: None, names: vec!["localhost".into()] }, now).unwrap();
        assert!(memory.generated && memory.cert_path.is_none());
    }

    #[test]
    fn the_users_files_are_used_as_they_are() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/tls");
        let source =
            Certificate::Files { cert: fixtures.join("ec-cert.pem"), key: fixtures.join("ec-sec1.pem") };
        let loaded = load(&source, 0).unwrap();
        assert!(!loaded.generated);
        assert_eq!(loaded.identity.fingerprint().to_string()[..8], *"78:B1:27");
        let bad = Certificate::Files { cert: fixtures.join("ec-cert.pem"), key: fixtures.join("nope.pem") };
        assert!(load(&bad, 0).err().unwrap().to_string().contains("nope.pem"));
    }

    #[test]
    fn names_cover_loopback_and_a_specific_listen_address() {
        let names = default_names("0.0.0.0:8080".parse().unwrap());
        assert_eq!(names[..3], ["localhost", "127.0.0.1", "::1"]);
        assert!(!names.contains(&"0.0.0.0".to_owned()));
        let names = default_names("192.168.1.20:8080".parse().unwrap());
        assert_eq!(names.last().unwrap(), "192.168.1.20");
        let names = default_names("127.0.0.1:8080".parse().unwrap());
        assert_eq!(names.iter().filter(|n| *n == "127.0.0.1").count(), 1);
    }
}
