//! Profiles and portable mode (D42, docs/PROFILES_PLAN.md).
//!
//! Portable mode (D7): a `patorrent.ini` file beside the executable (beside the `.AppImage`)
//! makes its folder the application root, so settings and state travel with the program, as
//! µTorrent's `settings.dat` beside `uTorrent.exe` did. One test for every frontend:
//! [`portable_root`]. The paths inside the folder are written relative to it
//! (`patorrent_core::util::portable`: [`crate::prefs`], `rss.dat`, the engine's `resume.db`),
//! so it can move to another drive or computer; the first start there downloads to its
//! `Downloads` folder ([`first_run_settings`]).
//!
//! A **profile** is a whole PaTorrent of its own: `--profile NAME` inserts `profiles/<name>`
//! between the application root and the program's folder, so a profile looks exactly like a
//! fresh install (`%APPDATA%\PaTorrent\profiles\work\`, `~/.config/patorrent/profiles/work/gtk`,
//! `.../profiles/work/nox`). No flag, or `--profile default`, is the default profile: today's
//! folders, unchanged. The choice is made once at start ([`init`]) and read everywhere through
//! [`current`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The file whose presence beside the executable means portable mode.
pub const PORTABLE_MARKER: &str = "patorrent.ini";

/// The command-line switch: `--profile NAME` or `--profile=NAME`.
pub const SWITCH: &str = "--profile";

/// The name that means the default profile.
pub const DEFAULT_NAME: &str = "default";

/// The folder below the application root that holds the named profiles.
pub const FOLDER: &str = "profiles";

/// The longest name.
pub const MAX_LEN: usize = 32;

/// The Windows shell's `--shell`, after which nothing is ours (SEC-18): a profile switch must
/// come before it.
const STOP: &str = "--shell";

/// The portable root: the folder [`portable_candidate`] names, when [`PORTABLE_MARKER`] is a
/// file there.
pub fn portable_root() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k);
    portable_root_in(&portable_candidate(&env, std::env::current_exe().ok()?.parent()?)?)
}

/// Where a portable marker would make this program portable: the executable's folder (Windows,
/// a Linux or BSD tarball); for an AppImage the folder holding the `.AppImage` file (`$APPIMAGE`'s,
/// as the executable is inside its read-only mount); none in a Flatpak, whose sandbox cannot see
/// beside itself, nor on macOS yet (slice 7: the folder holding `PaTorrent.app`).
pub fn portable_candidate(
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    exe_dir: &Path,
) -> Option<PathBuf> {
    if cfg!(target_os = "macos") || env("FLATPAK_ID").is_some() {
        return None;
    }
    match env("APPIMAGE").filter(|a| !cfg!(windows) && !a.is_empty()) {
        Some(image) => Path::new(&image).parent().map(Path::to_path_buf),
        None => Some(exe_dir.to_path_buf()),
    }
}

/// `dir` when it holds the portable marker as a file (a folder of that name does not count).
pub fn portable_root_in(dir: &Path) -> Option<PathBuf> {
    dir.join(PORTABLE_MARKER).is_file().then(|| dir.to_path_buf())
}

/// A profile name that cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameError {
    /// `--profile` came last, without a name.
    Missing,
    /// Not 1-32 characters from `a-z`, `0-9` and `-` starting with a letter (as given).
    Invalid(String),
}

impl NameError {
    /// What was given, for the message (empty for [`NameError::Missing`]).
    pub fn given(&self) -> &str {
        match self {
            NameError::Missing => "",
            NameError::Invalid(n) => n,
        }
    }
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::Missing => write!(f, "{SWITCH} needs a name"),
            NameError::Invalid(n) => write!(
                f,
                "{SWITCH} {n:?}: a profile name has 1 to {MAX_LEN} characters, letters a-z, digits and \
                 hyphens, and starts with a letter"
            ),
        }
    }
}

impl std::error::Error for NameError {}

/// `name` in lower case when it is a usable profile name: 1-32 characters from `a-z`, `0-9` and
/// `-`, starting with a letter, compared without case. The narrow set needs no encoding in
/// folder names, service names, systemd instances, rc.d variables and D-Bus names.
pub fn validate(name: &str) -> Result<String, NameError> {
    let lower = name.to_ascii_lowercase();
    let ok = (1..=MAX_LEN).contains(&lower.len())
        && lower.starts_with(|c: char| c.is_ascii_lowercase())
        && lower.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok { Ok(lower) } else { Err(NameError::Invalid(name.to_owned())) }
}

/// The profile named among `args` (`None`: the default profile) and the arguments without the
/// switch and its value, which are never a torrent. The switch may come anywhere before
/// `--shell`; the last one wins.
pub fn split_args(args: &[String]) -> Result<(Option<String>, Vec<String>), NameError> {
    let mut name = None;
    let mut rest = Vec::with_capacity(args.len());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let value = if a == SWITCH {
            it.next().ok_or(NameError::Missing)?.as_str()
        } else if let Some(v) = a.strip_prefix(SWITCH).and_then(|v| v.strip_prefix('=')) {
            v
        } else {
            rest.push(a.clone());
            if a == STOP {
                rest.extend(it.cloned());
                break;
            }
            continue;
        };
        name = match validate(value)? {
            n if n == DEFAULT_NAME => None,
            n => Some(n),
        };
    }
    Ok((name, rest))
}

/// `root` for the default profile, `root/profiles/<name>` for a named one.
pub fn within(root: &Path, name: Option<&str>) -> PathBuf {
    match name {
        Some(n) => root.join(FOLDER).join(n),
        None => root.to_path_buf(),
    }
}

/// The named profiles under `root` (the application root, or a base folder that holds
/// `profiles/`): folders with a usable name, sorted.
pub fn list(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join(FOLDER)) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|n| validate(n).is_ok_and(|v| v == *n && v != DEFAULT_NAME))
        .collect();
    names.sort();
    names
}

/// File > Profiles' list: the default profile (`None`), then the named ones under `root`, each
/// with whether it is this process's.
pub fn menu(root: &Path) -> Vec<(Option<String>, bool)> {
    let current = current().name.clone();
    std::iter::once(None)
        .chain(list(root).into_iter().map(Some))
        .map(|n| {
            let me = n == current;
            (n, me)
        })
        .collect()
}

/// The arguments that start another process in profile `name` (`None`: the default one).
pub fn args_for(name: Option<&str>) -> Vec<String> {
    name.map_or(Vec::new(), |n| vec![SWITCH.to_owned(), n.to_owned()])
}

/// This process's profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    /// `None` for the default profile.
    pub name: Option<String>,
    /// The portable root ([`portable_root`]) when the program runs in portable mode.
    pub portable_root: Option<PathBuf>,
}

impl Profile {
    pub fn is_default(&self) -> bool {
        self.name.is_none()
    }

    /// `" [work]"` for a named profile (titles, tooltips), empty for the default one.
    pub fn tag(&self) -> String {
        self.name.as_ref().map_or(String::new(), |n| format!(" [{n}]"))
    }

    /// `"PaTorrent (work)"`: a name for this profile in other programs' lists (the router's port
    /// mappings, the autostart list); `"PaTorrent"` for the default one.
    pub fn label(&self) -> String {
        self.name.as_ref().map_or("PaTorrent".to_owned(), |n| format!("PaTorrent ({n})"))
    }

    /// The engine's `SessionParams::label` (the routers' port mapping description): the
    /// [`label`](Self::label) for a named profile, `None` (the user agent) for the default one.
    pub fn engine_label(&self) -> Option<String> {
        self.name.as_ref().map(|_| self.label())
    }

    /// The arguments that start another process of this profile: `--profile <name>`, none for
    /// the default one.
    pub fn args(&self) -> Vec<String> {
        self.name.as_ref().map_or(Vec::new(), |n| vec![SWITCH.to_owned(), n.clone()])
    }

    /// `base` for the default profile; `base.profile_<name with - as _>` for a named one: GTK's
    /// application id, unique per profile, with D-Bus's allowed characters.
    pub fn app_id(&self, base: &str) -> String {
        match &self.name {
            Some(n) => format!("{base}.profile_{}", n.replace('-', "_")),
            None => base.to_owned(),
        }
    }
}

static CURRENT: OnceLock<Profile> = OnceLock::new();

/// Fixes this process's profile, once, at start (a later call keeps the first choice); the
/// portable test is made here, once.
pub fn init(name: Option<String>) -> &'static Profile {
    CURRENT.get_or_init(|| Profile { name, portable_root: portable_root() })
}

/// This process's profile; the default one if [`init`] was not called.
pub fn current() -> &'static Profile {
    init(None)
}

/// The first start of a named profile or of a portable folder, whose settings file has no keys
/// yet: a random listen port (10000-60000) goes into `s` and `prefs`, so it does not take the
/// 6881 another PaTorrent may already listen on (D42). The default profile keeps 6881 (firewall
/// rules name it). True when it did.
pub fn first_run_settings(prefs: &mut crate::prefs::Prefs, s: &mut patorrent_core::Settings) -> bool {
    let p = current();
    if (p.is_default() && p.portable_root.is_none()) || prefs.entries().next().is_some() {
        return false;
    }
    crate::settings::set_listen_port(s, crate::options::random_port());
    crate::settings::store(prefs, s);
    // a portable folder downloads into itself (`./Downloads` in the file)
    if let Some(root) = &p.portable_root {
        prefs.set("dir.download", root.join("Downloads").to_string_lossy());
    }
    true
}

/// The keys of `patorrent.ini` whose values are paths, written relative to a portable folder
/// ([`crate::prefs`]): the folders of Preferences > Directories, the IP filter's file, the Add
/// dialog's recent folders (`|`-separated) and the save folders of magnets awaiting metadata.
pub fn is_path_key(key: &str) -> bool {
    matches!(
        key,
        "dir.download"
            | "dir.completed"
            | "dir.torrents"
            | "dir.finished_torrents"
            | "dir.autoload_dir"
            | "ipfilter.path"
            | "add.recent_dirs"
    ) || (key.starts_with("torrent.") && key.ends_with(".awaiting"))
}

/// A listen port of a nox profile's own, from its name (10000-60000): nox keeps no settings
/// file outside the Web UI, so the port must come out the same at every start.
pub fn name_port(name: &str) -> u16 {
    let hash =
        name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    10_000 + (hash % 50_001) as u16
}

/// [`crate::BUILD`] with the profile's tag: the `{version}` of titles and tooltips.
pub fn version() -> String {
    format!("{}{}", crate::BUILD, current().tag())
}

/// [`crate::NAME_BUILD`] with the profile's tag: `PaTorrent 2.0.0 [work]`.
pub fn name_build() -> String {
    format!("{}{}", crate::NAME_BUILD, current().tag())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_marker_must_be_a_file() {
        let dir = std::env::temp_dir().join(format!("pa-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(portable_root_in(&dir), None);
        std::fs::create_dir(dir.join(PORTABLE_MARKER)).unwrap();
        assert_eq!(portable_root_in(&dir), None, "a folder named patorrent.ini");
        std::fs::remove_dir(dir.join(PORTABLE_MARKER)).unwrap();
        std::fs::write(dir.join(PORTABLE_MARKER), "").unwrap();
        assert_eq!(portable_root_in(&dir), Some(dir.clone()));

        // the profiles found under a root: usable names only, sorted
        for d in ["work", "b-2", "Upper", "9lives", "default", "x y"] {
            std::fs::create_dir_all(dir.join(FOLDER).join(d)).unwrap();
        }
        std::fs::write(dir.join(FOLDER).join("file"), "").unwrap();
        // `Upper` is not stored lower case: not one PaTorrent made
        assert_eq!(list(&dir), ["b-2", "work"]);
        assert!(list(&dir.join("none")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names() {
        for ok in ["work", "a", "vpn-1", "WORK", "a-", &"x".repeat(32)] {
            assert_eq!(validate(ok), Ok(ok.to_ascii_lowercase()), "{ok}");
        }
        for bad in ["", "1work", "-a", "wo rk", "work!", "wörk", "a_b", "a.b", &"x".repeat(33)] {
            assert_eq!(validate(bad), Err(NameError::Invalid(bad.to_owned())), "{bad}");
        }
        assert!(NameError::Invalid("Work!".into()).to_string().contains("\"Work!\""));
    }

    #[test]
    fn arguments() {
        let split = |a: &[&str]| split_args(&args(a));
        assert_eq!(split(&[]), Ok((None, vec![])));
        assert_eq!(split(&["a.torrent"]), Ok((None, args(&["a.torrent"]))));
        // anywhere, either form, never a source; the value's case does not matter
        assert_eq!(
            split(&["a.torrent", "--profile", "Work", "--minimized"]),
            Ok((Some("work".into()), args(&["a.torrent", "--minimized"])))
        );
        assert_eq!(split(&["--profile=vpn", "--remote"]), Ok((Some("vpn".into()), args(&["--remote"]))));
        // `default` is the default profile; the last one wins
        assert_eq!(split(&["--profile", "default"]), Ok((None, vec![])));
        assert_eq!(split(&["--profile", "a", "--profile", "b"]), Ok((Some("b".into()), vec![])));
        // after --shell everything is the link's (SEC-18)
        assert_eq!(
            split(&["--profile", "w", "--shell", "magnet:?x", "--profile", "z"]),
            Ok((Some("w".into()), args(&["--shell", "magnet:?x", "--profile", "z"])))
        );
        assert_eq!(split(&["--profile"]), Err(NameError::Missing));
        assert_eq!(split(&["--profile", "Work!"]), Err(NameError::Invalid("Work!".into())));
        assert_eq!(split(&["--profile="]), Err(NameError::Invalid(String::new())));
        // a longer switch is not ours
        assert_eq!(split(&["--profiles"]), Ok((None, args(&["--profiles"]))));
    }

    #[test]
    fn folders_and_names() {
        let root = Path::new("/r");
        assert_eq!(within(root, None), PathBuf::from("/r"));
        assert_eq!(within(root, Some("work")), Path::new("/r").join("profiles").join("work"));

        let default = Profile::default();
        assert_eq!(
            (default.tag(), default.label(), default.args()),
            (String::new(), "PaTorrent".into(), vec![])
        );
        assert_eq!(default.app_id("io.x.PaTorrent"), "io.x.PaTorrent");
        let work = Profile { name: Some("my-work".into()), portable_root: None };
        assert_eq!(work.tag(), " [my-work]");
        assert_eq!(work.label(), "PaTorrent (my-work)");
        assert_eq!(work.args(), args(&["--profile", "my-work"]));
        assert_eq!(work.app_id("io.x.PaTorrent"), "io.x.PaTorrent.profile_my_work");
        assert_eq!(work.engine_label().as_deref(), Some("PaTorrent (my-work)"));
        assert_eq!(default.engine_label(), None);
    }

    #[test]
    fn where_portable_mode_is_looked_for() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| std::ffi::OsString::from(v))
        };
        let exe_dir = Path::new("/opt/patorrent");
        if cfg!(target_os = "macos") {
            assert_eq!(portable_candidate(&env(&[]), exe_dir), None);
            return;
        }
        assert_eq!(portable_candidate(&env(&[]), exe_dir), Some(exe_dir.to_path_buf()));
        // a Flatpak never; an AppImage beside its file, not in its mount
        assert_eq!(portable_candidate(&env(&[("FLATPAK_ID", "io.x")]), exe_dir), None);
        let image = env(&[("APPIMAGE", "/home/u/Apps/PaTorrent.AppImage")]);
        let want = if cfg!(windows) { exe_dir.to_path_buf() } else { PathBuf::from("/home/u/Apps") };
        assert_eq!(portable_candidate(&image, exe_dir), Some(want));
    }

    #[test]
    fn a_nox_profiles_port_comes_from_its_name() {
        assert_eq!(name_port("work"), name_port("work"));
        assert_ne!(name_port("work"), name_port("vpn"));
        for n in ["a", "work", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"] {
            assert!((10_000..=60_000).contains(&name_port(n)), "{n}");
        }
    }
}
