//! The search helper's folder and settings: where it is unpacked, what we keep about it
//! (`helper.ini`: the version, the port, and the API key and admin password we generated,
//! sealed like a search source's key), and the `ServerConfig.json` we write into its data
//! folder before every start.
//!
//! `ServerConfig.json` (Jackett's `src/Jackett.Common/Models/Config/ServerConfig.cs`, written by
//! Newtonsoft with its property names; `ConfigurationService.cs` keeps a file that parses and
//! has an `APIKey`): we set the port, loopback only (`AllowExternal` false, `LocalBindAddress`
//! `127.0.0.1`; on Linux and macOS its own default listens on every interface), the API key,
//! the admin password's hash ([`super::sha512::jackett_password_hash`]), updates off and the
//! proxy from ours, and keep every other field as the helper or its own page left it.

use std::path::{Path, PathBuf};

use rookparcel_core::util::private_file;
use rookparcel_core::{ProxyType, Settings};

use super::json::{self, Json};
use super::sha512::jackett_password_hash;
use crate::prefs::Prefs;

/// The helper's folder in the profile folder.
pub const FOLDER: &str = "search-helper";

/// Where the helper's files are, below a profile folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// `<profile>/search-helper`.
    pub root: PathBuf,
}

impl Layout {
    pub fn new(profile_dir: &Path) -> Layout {
        Layout { root: profile_dir.join(FOLDER) }
    }

    /// The unpacked release.
    pub fn app(&self) -> PathBuf {
        self.root.join("app")
    }

    /// Jackett's `--DataFolder`: its settings, indexers and log.
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    pub fn state_file(&self) -> PathBuf {
        self.root.join("helper.ini")
    }

    /// The program to start, in the unpacked release: `Jackett/JackettConsole.exe` on
    /// Windows, `Jackett/jackett` elsewhere (`src/Jackett.Server/Jackett.Server.csproj`'s
    /// assembly names; the archives have a `Jackett/` folder at the top).
    pub fn exe(&self) -> PathBuf {
        let name = if cfg!(windows) { "JackettConsole.exe" } else { "jackett" };
        let app = self.app();
        let nested = app.join("Jackett").join(name);
        if nested.is_file() { nested } else { app.join(name) }
    }

    /// The release's Cardigann definitions (`Definitions/<id>.yml`).
    pub fn definitions(&self) -> PathBuf {
        self.exe().parent().map_or_else(|| self.app(), Path::to_path_buf).join("Definitions")
    }

    pub fn installed(&self) -> bool {
        self.exe().is_file() && self.state_file().is_file()
    }
}

/// What we keep about the installed helper (`helper.ini`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// The release's tag.
    pub version: String,
    /// The port it was last given.
    pub port: u16,
    /// Generated at the install; the Torznab source's key.
    pub api_key: String,
    /// Generated at the install; the helper's own page asks for it.
    pub password: String,
}

/// The keys of `helper.ini`; the secret ones are sealed ([`crate::prefs::is_sealed_key`]).
pub const KEY_VERSION: &str = "helper.version";
pub const KEY_PORT: &str = "helper.port";
pub const KEY_API_KEY: &str = "helper.api_key";
pub const KEY_PASSWORD: &str = "helper.password";

impl State {
    pub fn load(l: &Layout) -> Option<State> {
        if !l.state_file().is_file() {
            return None;
        }
        let p = Prefs::load_in(l.state_file(), None);
        let s = State {
            version: p.get(KEY_VERSION).unwrap_or("").to_owned(),
            port: p.get(KEY_PORT).and_then(|v| v.parse().ok()).unwrap_or(0),
            api_key: p.get(KEY_API_KEY).unwrap_or("").to_owned(),
            password: p.get(KEY_PASSWORD).unwrap_or("").to_owned(),
        };
        (!s.api_key.is_empty()).then_some(s)
    }

    pub fn save(&self, l: &Layout) -> Result<(), String> {
        let mut p = Prefs::load_in(l.state_file(), None);
        p.set(KEY_VERSION, &self.version);
        p.set(KEY_PORT, self.port);
        p.set(KEY_API_KEY, &self.api_key);
        p.set(KEY_PASSWORD, &self.password);
        p.save().map_err(|e| format!("{}: {e}", l.state_file().display()))
    }

    /// A new state with a generated key and password.
    pub fn generate(version: &str) -> State {
        State { version: version.to_owned(), port: 0, api_key: random_token(32), password: random_token(24) }
    }
}

/// `n` random characters from `[a-z0-9]` (as Jackett's own `StringUtil.GenerateRandom`), from
/// the OS's cryptographic generator.
pub fn random_token(n: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(n);
    let mut buf = [0u8; 64];
    while out.len() < n {
        rookparcel_core::util::random::crypto_random_bytes(&mut buf);
        // 252 = 7 * 36: no bias
        for b in buf.iter().filter(|&&b| b < 252) {
            if out.len() < n {
                out.push(ALPHABET[(*b % 36) as usize] as char);
            }
        }
    }
    out
}

/// The helper's proxy (Jackett's `ProxyType`: `Disabled = -1, Http = 0, Socks4 = 1, Socks5 =
/// 2`, `src/Jackett.Common/Models/Config/ProxyType.cs`), from the engine's proxy settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperProxy {
    pub kind: i32,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

impl HelperProxy {
    /// The session's proxy whenever one is set, as the fetches use it ([`rookparcel_core::fetch::FetchProxy::from_settings`]).
    pub fn from_settings(s: &Settings) -> Option<HelperProxy> {
        let kind = match s.proxy_type {
            ProxyType::None => return None,
            ProxyType::Http | ProxyType::HttpPw => 0,
            ProxyType::Socks4 => 1,
            ProxyType::Socks5 | ProxyType::Socks5Pw => 2,
        };
        if s.proxy_hostname.trim().is_empty() {
            return None;
        }
        let auth = matches!(s.proxy_type, ProxyType::HttpPw | ProxyType::Socks5Pw | ProxyType::Socks4);
        Some(HelperProxy {
            kind,
            host: s.proxy_hostname.trim().to_owned(),
            port: s.proxy_port,
            username: if auth { s.proxy_username.clone() } else { String::new() },
            password: if auth && s.proxy_type != ProxyType::Socks4 {
                s.proxy_password.clone()
            } else {
                String::new()
            },
        })
    }
}

/// Writes `<data>/ServerConfig.json`: the fields this module's documentation names set, every
/// other field of an existing file kept. Readable by its owner only on Unix (it holds the API
/// key and a proxy password).
pub fn write_server_config(
    data: &Path,
    port: u16,
    state: &State,
    proxy: Option<&HelperProxy>,
) -> Result<(), String> {
    let path = data.join("ServerConfig.json");
    let mut cfg = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| json::parse(&t).ok())
        .filter(|v| matches!(v, Json::Object(_)))
        .unwrap_or(Json::Object(Vec::new()));
    cfg.set("Port", Json::num(port));
    cfg.set("LocalBindAddress", Json::string("127.0.0.1"));
    cfg.set("AllowExternal", Json::Bool(false));
    cfg.set("AllowCORS", Json::Bool(false));
    cfg.set("APIKey", Json::string(&state.api_key));
    cfg.set("AdminPassword", Json::string(jackett_password_hash(&state.password, &state.api_key)));
    if cfg.str("InstanceId").is_none_or(str::is_empty) {
        cfg.set("InstanceId", Json::string(random_token(64)));
    }
    cfg.set("UpdateDisabled", Json::Bool(true));
    cfg.set("UpdatePrerelease", Json::Bool(false));
    match proxy {
        Some(p) => {
            cfg.set("ProxyType", Json::num(p.kind));
            cfg.set("ProxyUrl", Json::string(&p.host));
            cfg.set("ProxyPort", Json::num(p.port));
            cfg.set("ProxyUsername", Json::string(&p.username));
            cfg.set("ProxyPassword", Json::string(&p.password));
        }
        None => {
            cfg.set("ProxyType", Json::num(-1));
            cfg.set("ProxyUrl", Json::string(""));
            cfg.set("ProxyPort", Json::Null);
            cfg.set("ProxyUsername", Json::string(""));
            cfg.set("ProxyPassword", Json::string(""));
        }
    }
    private_file::create_dir_all(data).map_err(|e| format!("{}: {e}", data.display()))?;
    private_file::write(&path, cfg.to_pretty().as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::helper::archive::tests::TempDir;

    #[test]
    fn state_round_trips_sealed() {
        let t = TempDir::new("state");
        let l = Layout::new(&t.0);
        assert!(State::load(&l).is_none());
        let mut s = State::generate("v0.24.3009");
        s.port = 49152;
        assert_eq!(s.api_key.len(), 32);
        assert_eq!(s.password.len(), 24);
        assert!(s.api_key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
        assert_ne!(s.api_key, State::generate("x").api_key);
        s.save(&l).unwrap();
        assert_eq!(State::load(&l), Some(s.clone()));
        let text = std::fs::read_to_string(l.state_file()).unwrap();
        if cfg!(windows) {
            assert!(!text.contains(&s.api_key) && !text.contains(&s.password), "{text}");
        }
        assert!(!l.installed(), "no program yet");
    }

    #[test]
    fn server_config_is_ours_where_it_matters() {
        let t = TempDir::new("servercfg");
        let data = t.0.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(
            data.join("ServerConfig.json"),
            r#"{"Port":9117,"AllowExternal":true,"FlareSolverrUrl":"http://127.0.0.1:8191","InstanceId":"keep-me","ProxyType":2}"#,
        )
        .unwrap();
        let s = State { version: "v1".into(), port: 0, api_key: "k".repeat(32), password: "pw".into() };
        let proxy = HelperProxy {
            kind: 2,
            host: "proxy.example".into(),
            port: 1080,
            username: "u".into(),
            password: "p".into(),
        };
        write_server_config(&data, 50123, &s, Some(&proxy)).unwrap();
        let v = json::parse(&std::fs::read_to_string(data.join("ServerConfig.json")).unwrap()).unwrap();
        assert_eq!(v.get("Port").and_then(Json::as_u64), Some(50123));
        assert_eq!(v.get("AllowExternal").and_then(Json::as_bool), Some(false));
        assert_eq!(v.str("LocalBindAddress"), Some("127.0.0.1"));
        assert_eq!(v.str("APIKey"), Some(s.api_key.as_str()));
        assert_eq!(v.str("AdminPassword").map(str::len), Some(128));
        assert_eq!(v.str("InstanceId"), Some("keep-me"));
        assert_eq!(v.str("FlareSolverrUrl"), Some("http://127.0.0.1:8191"), "the user's own settings stay");
        assert_eq!(v.get("UpdateDisabled").and_then(Json::as_bool), Some(true));
        assert_eq!(v.str("ProxyUrl"), Some("proxy.example"));
        write_server_config(&data, 50124, &s, None).unwrap();
        let v = json::parse(&std::fs::read_to_string(data.join("ServerConfig.json")).unwrap()).unwrap();
        assert_eq!(v.get("ProxyType"), Some(&Json::num(-1)));
        assert_eq!(v.str("ProxyPassword"), Some(""));
        // a fresh folder gets an instance id of its own
        let fresh = t.0.join("fresh");
        write_server_config(&fresh, 1, &s, None).unwrap();
        let v = json::parse(&std::fs::read_to_string(fresh.join("ServerConfig.json")).unwrap()).unwrap();
        assert_eq!(v.str("InstanceId").map(str::len), Some(64));
    }

    #[test]
    fn the_proxy_follows_ours() {
        let mut s = Settings::default();
        assert_eq!(HelperProxy::from_settings(&s), None);
        s.proxy_type = ProxyType::Socks5Pw;
        s.proxy_hostname = " 10.0.0.1 ".into();
        s.proxy_port = 1080;
        s.proxy_username = "u".into();
        s.proxy_password = "p".into();
        let p = HelperProxy::from_settings(&s).unwrap();
        assert_eq!((p.kind, p.host.as_str(), p.port, p.password.as_str()), (2, "10.0.0.1", 1080, "p"));
        s.proxy_type = ProxyType::Http;
        let p = HelperProxy::from_settings(&s).unwrap();
        assert_eq!((p.kind, p.username.as_str()), (0, ""));
        s.proxy_hostname.clear();
        assert_eq!(HelperProxy::from_settings(&s), None);
    }
}
