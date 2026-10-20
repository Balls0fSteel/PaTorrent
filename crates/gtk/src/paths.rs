//! Where the GTK client keeps its files, after the XDG Base Directory spec (as nox does):
//!
//! | | default |
//! |---|---|
//! | settings (`patorrent.ini`, `rss.dat`) | `$XDG_CONFIG_HOME/patorrent/gtk`, else `~/.config/patorrent/gtk` |
//! | torrent library (`resume.db`, `metadata.pack`, `dht_state.dat`) | `$XDG_DATA_HOME/patorrent/gtk`, else `~/.local/share/patorrent/gtk` |
//!
//! Relative or empty XDG variables are ignored, as the spec says.

use std::ffi::OsString;
use std::path::PathBuf;

const APP: [&str; 2] = ["patorrent", "gtk"];

/// The settings folder.
pub fn config_dir() -> PathBuf {
    dir(&|k| std::env::var_os(k), "XDG_CONFIG_HOME", &[".config"])
}

/// The torrent library's folder.
pub fn data_dir() -> PathBuf {
    dir(&|k| std::env::var_os(k), "XDG_DATA_HOME", &[".local", "share"])
}

fn dir(env: &dyn Fn(&str) -> Option<OsString>, var: &str, fallback: &[&str]) -> PathBuf {
    let base = match env(var).map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => {
            let home = env("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            fallback.iter().fold(home, |p, c| p.join(c))
        }
    };
    APP.iter().fold(base, |p, c| p.join(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_variables_win_when_absolute() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
        };
        let e = env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/cfg")]);
        assert_eq!(dir(&e, "XDG_CONFIG_HOME", &[".config"]), PathBuf::from("/cfg/patorrent/gtk"));
        let e = env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "rel")]);
        assert_eq!(
            dir(&e, "XDG_DATA_HOME", &[".local", "share"]),
            PathBuf::from("/home/u/.local/share/patorrent/gtk")
        );
        let e = env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "")]);
        assert_eq!(dir(&e, "XDG_CONFIG_HOME", &[".config"]), PathBuf::from("/home/u/.config/patorrent/gtk"));
    }
}
