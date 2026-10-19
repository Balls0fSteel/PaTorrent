//! Where `patorrent-nox` keeps its files (PLAN.md Phase 6; D7 on Windows).
//!
//! Two directories:
//!
//! - the **state** directory holds the torrent library: the session store (`resume.db`,
//!   `metadata.pack`) and the DHT state (`dht_state.dat`);
//! - the **config** directory holds what the user set: `webui.json` (the Web UI login, its
//!   Preferences, labels), `rss.dat` (the RSS Downloader) and the default `ipfilter.dat`.
//!
//! On Linux and the BSDs they follow the XDG Base Directory spec. The library is user data
//! that is not easily rebuilt, so it goes to `$XDG_DATA_HOME` (as qBittorrent's `BT_backup`
//! does), not `$XDG_STATE_HOME`, which the spec keeps for history and layout that can be lost:
//!
//! | | default |
//! |---|---|
//! | state | `$XDG_DATA_HOME/patorrent/nox`, else `~/.local/share/patorrent/nox` |
//! | config | `$XDG_CONFIG_HOME/patorrent/nox`, else `~/.config/patorrent/nox` |
//!
//! Nothing is cached, so `$XDG_CACHE_HOME` is not used. Relative or empty XDG variables are
//! ignored, as the spec says. Under systemd, `StateDirectory=` and `ConfigurationDirectory=`
//! (`$STATE_DIRECTORY`, `$CONFIGURATION_DIRECTORY`) come before XDG.
//!
//! On Windows both are `%APPDATA%\PaTorrent\nox`, or `nox` beside the exe in portable mode
//! (`patorrent.ini` beside the exe, the GUI's rule).
//!
//! `--state-dir` (or `--state`) and `--config-dir` override; a state directory given on the
//! command line or by systemd holds the config too unless a config directory is also given,
//! so `--state DIR` keeps everything in one folder as before.
//!
//! Before 2026-10-19 Unix kept `webui.json` in the state directory; [`migrate`] moves it.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// The folder below the XDG base directories (and `%APPDATA%\PaTorrent`).
const APP: [&str; 2] = ["patorrent", "nox"];

/// The files that moved from the state to the config directory on Unix.
const MOVED: [&str; 2] = [patorrent_nox::webui::STATE_FILE, "ipfilter.dat"];

/// Where the state and config live; `None` when neither a flag nor the environment names a
/// place (no `HOME`, no `APPDATA`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dirs {
    pub state: Option<PathBuf>,
    pub config: Option<PathBuf>,
    /// Where `webui.json` and `ipfilter.dat` lived before they moved to the config directory,
    /// when that differs from it (the XDG defaults on Unix).
    pub legacy_config: Option<PathBuf>,
}

/// The directories for this OS from the command line's choices and the process environment.
pub fn resolve(state: Option<PathBuf>, config: Option<PathBuf>) -> Dirs {
    let env = |k: &str| std::env::var_os(k);
    if cfg!(windows) {
        resolve_windows(state, config, env, portable_dir())
    } else {
        resolve_unix(state, config, env)
    }
}

/// The exe's folder when `patorrent.ini` is beside it (portable mode, D7).
fn portable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    dir.join("patorrent.ini").is_file().then(|| dir.to_path_buf())
}

/// Windows: the flags, else portable mode, else `%APPDATA%\PaTorrent\nox`; the config follows
/// the state.
pub fn resolve_windows(
    state: Option<PathBuf>,
    config: Option<PathBuf>,
    env: impl Fn(&str) -> Option<OsString>,
    portable: Option<PathBuf>,
) -> Dirs {
    let default = portable.map(|d| d.join(APP[1])).or_else(|| {
        env("APPDATA").filter(|v| !v.is_empty()).map(|d| PathBuf::from(d).join("PaTorrent").join(APP[1]))
    });
    let state = state.or(default);
    let config = config.or_else(|| state.clone());
    Dirs { state, config, legacy_config: None }
}

/// Linux and the BSDs: the flags, else systemd's directories, else XDG (see the module docs).
pub fn resolve_unix(
    state: Option<PathBuf>,
    config: Option<PathBuf>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Dirs {
    let explicit_state = state.or_else(|| systemd_dir(&env, "STATE_DIRECTORY"));
    let explicit_config = config.or_else(|| systemd_dir(&env, "CONFIGURATION_DIRECTORY"));
    let xdg_state = xdg_dir(&env, "XDG_DATA_HOME", &[".local", "share"]);
    let legacy_config = match (&explicit_state, &explicit_config) {
        // webui.json used to sit in the default state directory
        (None, None) => xdg_state.clone(),
        _ => None,
    };
    let config = explicit_config
        .or_else(|| explicit_state.clone())
        .or_else(|| xdg_dir(&env, "XDG_CONFIG_HOME", &[".config"]));
    let state = explicit_state.or(xdg_state);
    let legacy_config = legacy_config.filter(|l| Some(l) != config.as_ref());
    Dirs { state, config, legacy_config }
}

/// An absolute path in the Unix sense (the spec ignores relative ones). Checked by hand so the
/// Unix rules can be tested on Windows too, where `/home` is not absolute.
fn unix_absolute(v: &OsStr) -> bool {
    v.as_encoded_bytes().first() == Some(&b'/')
}

/// `$XDG_*_HOME/patorrent/nox`, else `$HOME/<fallback>/patorrent/nox`.
fn xdg_dir(env: &impl Fn(&str) -> Option<OsString>, var: &str, fallback: &[&str]) -> Option<PathBuf> {
    let base = env(var).filter(|v| unix_absolute(v)).map(PathBuf::from).or_else(|| {
        let home = env("HOME").filter(|v| unix_absolute(v))?;
        Some(fallback.iter().fold(PathBuf::from(home), |p, c| p.join(c)))
    })?;
    Some(APP.iter().fold(base, |p, c| p.join(c)))
}

/// systemd's `$STATE_DIRECTORY` / `$CONFIGURATION_DIRECTORY`: a `:`-separated list of the
/// unit's directories (absolute); the first is ours.
fn systemd_dir(env: &impl Fn(&str) -> Option<OsString>, var: &str) -> Option<PathBuf> {
    let v = env(var)?;
    let first = v.as_encoded_bytes().split(|&b| b == b':').next()?;
    // SAFETY: split at an ASCII byte of a string that came from `OsString`
    let first = unsafe { OsStr::from_encoded_bytes_unchecked(first) };
    unix_absolute(first).then(|| PathBuf::from(first))
}

/// Moves `webui.json` from the old place to the config directory when only the old place has
/// one, and copies `ipfilter.dat` likewise (a `--ip-filter` path may still name the old copy).
/// Returns the new files, as `(file, moved)` (`false`: copied).
pub fn migrate(from: &Path, to: &Path) -> io::Result<Vec<(PathBuf, bool)>> {
    let mut done = Vec::new();
    for (i, name) in MOVED.iter().enumerate() {
        let (old, new) = (from.join(name), to.join(name));
        if !old.is_file() || new.exists() {
            continue;
        }
        std::fs::create_dir_all(to)?;
        let moved = i == 0;
        if moved {
            // a rename across file systems fails: copy, then remove
            if std::fs::rename(&old, &new).is_err() {
                std::fs::copy(&old, &new)?;
                std::fs::remove_file(&old)?;
            }
        } else {
            std::fs::copy(&old, &new)?;
        }
        done.push((new, moved));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| vars.iter().find(|(n, _)| n == k).map(|(_, v)| OsString::from(v))
    }

    fn p(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    fn join(base: &str, parts: &[&str]) -> Option<PathBuf> {
        Some(parts.iter().fold(PathBuf::from(base), |p, c| p.join(c)))
    }

    #[test]
    fn unix_defaults_follow_xdg() {
        // HOME only: the spec's fallbacks
        let d = resolve_unix(None, None, env(&[("HOME", "/home/u")]));
        assert_eq!(d.state, join("/home/u", &[".local", "share", "patorrent", "nox"]));
        assert_eq!(d.config, join("/home/u", &[".config", "patorrent", "nox"]));
        assert_eq!(d.legacy_config, d.state);

        // both XDG variables set
        let d = resolve_unix(
            None,
            None,
            env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/d"), ("XDG_CONFIG_HOME", "/c")]),
        );
        assert_eq!(d.state, join("/d", &["patorrent", "nox"]));
        assert_eq!(d.config, join("/c", &["patorrent", "nox"]));
        assert_eq!(d.legacy_config, join("/d", &["patorrent", "nox"]));

        // one set, the other falls back; empty and relative values are ignored
        for data in ["", "rel/data"] {
            let d = resolve_unix(
                None,
                None,
                env(&[("HOME", "/h"), ("XDG_DATA_HOME", data), ("XDG_CONFIG_HOME", "/c")]),
            );
            assert_eq!(d.state, join("/h", &[".local", "share", "patorrent", "nox"]), "{data:?}");
            assert_eq!(d.config, join("/c", &["patorrent", "nox"]));
        }

        // nothing at all: no directories, as before (the caller warns)
        assert_eq!(resolve_unix(None, None, env(&[])), Dirs::default());
        assert_eq!(resolve_unix(None, None, env(&[("HOME", "relative")])), Dirs::default());
        // XDG alone works without HOME
        let d = resolve_unix(None, None, env(&[("XDG_CONFIG_HOME", "/c")]));
        assert_eq!((d.state, d.config), (None, join("/c", &["patorrent", "nox"])));
    }

    #[test]
    fn unix_flags_and_systemd_win() {
        let base = [("HOME", "/h"), ("XDG_DATA_HOME", "/d"), ("XDG_CONFIG_HOME", "/c")];
        // --state alone keeps everything in that folder, as before the split
        let d = resolve_unix(p("/s"), None, env(&base));
        assert_eq!(d, Dirs { state: p("/s"), config: p("/s"), legacy_config: None });
        // --config-dir alone: the state stays at its default, nothing to migrate
        let d = resolve_unix(None, p("/cfg"), env(&base));
        assert_eq!(
            d,
            Dirs { state: join("/d", &["patorrent", "nox"]), config: p("/cfg"), legacy_config: None }
        );
        let d = resolve_unix(p("/s"), p("/cfg"), env(&base));
        assert_eq!(d, Dirs { state: p("/s"), config: p("/cfg"), legacy_config: None });

        // systemd's StateDirectory= (several are `:`-separated; the first is ours)
        let mut vars = base.to_vec();
        vars.push(("STATE_DIRECTORY", "/var/lib/patorrent-nox:/var/lib/other"));
        let d = resolve_unix(None, None, env(&vars));
        assert_eq!(d.state, p("/var/lib/patorrent-nox"));
        assert_eq!(d.config, p("/var/lib/patorrent-nox"));
        assert_eq!(d.legacy_config, None);
        vars.push(("CONFIGURATION_DIRECTORY", "/etc/patorrent-nox"));
        let d = resolve_unix(None, None, env(&vars));
        assert_eq!((d.state, d.config), (p("/var/lib/patorrent-nox"), p("/etc/patorrent-nox")));
        // the flags still win over systemd
        let d = resolve_unix(p("/s"), None, env(&vars));
        assert_eq!((d.state, d.config), (p("/s"), p("/etc/patorrent-nox")));
        // a relative systemd value is ignored
        let d = resolve_unix(None, None, env(&[("HOME", "/h"), ("STATE_DIRECTORY", "lib/x")]));
        assert_eq!(d.state, join("/h", &[".local", "share", "patorrent", "nox"]));
    }

    #[test]
    fn windows_appdata_and_portable() {
        let appdata = [("APPDATA", r"C:\Users\u\AppData\Roaming"), ("XDG_CONFIG_HOME", "/c")];
        let nox = join(r"C:\Users\u\AppData\Roaming", &["PaTorrent", "nox"]);
        let d = resolve_windows(None, None, env(&appdata), None);
        assert_eq!(d, Dirs { state: nox.clone(), config: nox.clone(), legacy_config: None });
        // portable mode wins over %APPDATA%
        let d = resolve_windows(None, None, env(&appdata), p(r"D:\pa"));
        assert_eq!((d.state, d.config), (join(r"D:\pa", &["nox"]), join(r"D:\pa", &["nox"])));
        // flags
        let d = resolve_windows(p(r"E:\s"), None, env(&appdata), None);
        assert_eq!((d.state, d.config), (p(r"E:\s"), p(r"E:\s")));
        let d = resolve_windows(None, p(r"E:\c"), env(&appdata), None);
        assert_eq!((d.state, d.config), (nox, p(r"E:\c")));
        assert_eq!(resolve_windows(None, None, env(&[("APPDATA", "")]), None), Dirs::default());
    }

    #[test]
    fn migrate_moves_webui_json_and_copies_the_block_list() {
        let base = std::env::temp_dir().join(format!("nox-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (old, new) = (base.join("data"), base.join("config").join("nox"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("webui.json"), "{}").unwrap();
        std::fs::write(old.join("ipfilter.dat"), "1.2.3.4 - 1.2.3.5 , 0 , x").unwrap();
        std::fs::write(old.join("resume.db"), "x").unwrap();

        let done = migrate(&old, &new).unwrap();
        assert_eq!(done, [(new.join("webui.json"), true), (new.join("ipfilter.dat"), false)]);
        assert!(!old.join("webui.json").exists());
        assert!(old.join("ipfilter.dat").exists());
        assert!(old.join("resume.db").exists() && !new.join("resume.db").exists());
        assert_eq!(std::fs::read_to_string(new.join("webui.json")).unwrap(), "{}");

        // a second run finds nothing to do; a newer file in the config directory is kept
        std::fs::write(old.join("webui.json"), "old").unwrap();
        assert!(migrate(&old, &new).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(new.join("webui.json")).unwrap(), "{}");
        // nothing there at all
        assert!(migrate(&base.join("none"), &new).unwrap().is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
