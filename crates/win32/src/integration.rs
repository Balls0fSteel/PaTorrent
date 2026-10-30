//! Windows integration (Preferences > General): PaTorrent as the program for `.torrent` files
//! and `magnet:` links, and the "Start PaTorrent on system startup" entry.
//!
//! Everything lives under HKEY_CURRENT_USER, so no elevation is needed and a portable copy can
//! register itself. The state is read from the registry each time, never stored in
//! `patorrent.ini`: a box is checked when the registration names this very executable, so a
//! moved portable copy shows unchecked until it registers again.
//!
//! What is written, under `Software\Classes`:
//! - the ProgIDs `PaTorrent.torrent` and `PaTorrent.magnet` (`shell\open\command` is
//!   `"<exe>" --shell "%1"`, which a second instance forwards to the running one);
//! - `.torrent` naming `PaTorrent.torrent` (plus `OpenWithProgids` and the MIME type);
//! - the `magnet` URL scheme itself (`URL Protocol`), as µTorrent and qBittorrent do;
//! - `Software\PaTorrent\Capabilities` listed in `Software\RegisteredApplications`, so PaTorrent
//!   appears in Settings > Default apps.
//!
//! Since Windows 8 a choice the user made in Windows itself ("Always use this app", Default
//! apps) is kept in a hash-protected `UserChoice` key that programs cannot write; it wins over
//! the registration above. [`Status`] reports such a choice so the dialog can send the user to
//! Default apps.

use std::io;
use std::path::Path;
use std::ptr::{null, null_mut};

use patorrent_frontend::tr;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HWND, WIN32_ERROR};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteTreeW, RegDeleteValueW, RegGetValueW,
    RegOpenKeyExW, RegSetValueExW,
};
use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::util::wide;

pub use patorrent_frontend::integration::{Integration, MINIMIZED};

pub const PROGID_TORRENT: &str = "PaTorrent.torrent";
pub const PROGID_MAGNET: &str = "PaTorrent.magnet";
/// The `Run` value and the `RegisteredApplications` name.
const APP: &str = "PaTorrent";
const CLASSES: &str = r"Software\Classes";
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const TORRENT_CHOICE: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.torrent\UserChoice";
const MAGNET_CHOICE: &str =
    r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\magnet\UserChoice";
const APP_KEY: &str = r"Software\PaTorrent";
const CAPABILITIES: &str = r"Software\PaTorrent\Capabilities";
const REGISTERED: &str = r"Software\RegisteredApplications";

/// The registry as found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// What is registered for this executable.
    pub set: Integration,
    /// The ProgID of another program that Windows' own per-user choice opens `.torrent` files
    /// with although PaTorrent is registered.
    pub torrent_choice: Option<String>,
    /// The same for `magnet:` links.
    pub magnet_choice: Option<String>,
}

impl Status {
    /// PaTorrent opens `.torrent` files.
    pub fn opens_torrents(&self) -> bool {
        self.set.torrent && self.torrent_choice.is_none()
    }

    /// PaTorrent opens `magnet:` links.
    pub fn opens_magnets(&self) -> bool {
        self.set.magnet && self.magnet_choice.is_none()
    }

    /// One line for the General page when Windows' own choice overrides a registration.
    pub fn note(&self) -> String {
        match (&self.torrent_choice, &self.magnet_choice) {
            (None, None) => String::new(),
            (Some(t), None) => tr!(AssocTorrent, program = t),
            (None, Some(m)) => tr!(AssocMagnet, program = m),
            (Some(t), Some(m)) if t == m => tr!(AssocBoth, program = t),
            (Some(t), Some(m)) => tr!(AssocEach, torrent = t, magnet = m),
        }
    }
}

/// Where the keys are, relative to HKEY_CURRENT_USER: the real places, or the same layout under
/// a scratch key for tests.
#[derive(Clone, Debug, Default)]
pub struct Keys {
    prefix: String,
}

impl Keys {
    /// The current user's real registry.
    pub fn user() -> Keys {
        Keys { prefix: String::new() }
    }

    /// Everything under `HKCU\<root>` instead (tests).
    #[cfg(test)]
    pub fn under(root: &str) -> Keys {
        Keys { prefix: format!("{root}\\") }
    }

    fn path(&self, p: &str) -> String {
        format!("{}{p}", self.prefix)
    }

    fn class(&self, p: &str) -> String {
        self.path(&format!("{CLASSES}\\{p}"))
    }

    fn is_real(&self) -> bool {
        self.prefix.is_empty()
    }
}

/// `exe` as the registry holds it (no `\\?\` prefix).
fn exe_text(exe: &Path) -> String {
    let s = exe.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned()
}

/// The switch before the one argument the shell passes (`%1`). A raw `"` in a link splits it
/// into more arguments; everything after [`SHELL`]'s one is dropped, so a link cannot add
/// switches or files (SEC-18).
pub const SHELL: &str = "--shell";

/// The command that opens a `.torrent` file or a link with `exe`.
pub fn open_command(exe: &Path) -> String {
    format!("\"{}\" {SHELL} \"%1\"", exe_text(exe))
}

/// Rewrites our open commands that run `exe` but predate [`SHELL`] (`"<exe>" "%1"`). Nothing
/// else changes: what is not registered stays so.
pub fn upgrade(keys: &Keys, exe: &Path) -> io::Result<()> {
    let open = open_command(exe);
    for progid in [PROGID_TORRENT, PROGID_MAGNET, "magnet"] {
        let key = keys.class(&format!(r"{progid}\shell\open\command"));
        if get(&key, None).is_some_and(|c| runs(&c, exe) && c != open) {
            set(&key, None, &open)?;
        }
    }
    Ok(())
}

/// The autostart entry's command.
pub fn run_command(exe: &Path, minimized: bool) -> String {
    let mut s = format!("\"{}\"", exe_text(exe));
    if minimized {
        s.push(' ');
        s.push_str(MINIMIZED);
    }
    s
}

/// Whether a command line starts `exe` (quoted, compared without regard to case).
fn runs(command: &str, exe: &Path) -> bool {
    command.to_lowercase().contains(&format!("\"{}\"", exe_text(exe).to_lowercase()))
}

/// A ProgID Windows' choice may name that is PaTorrent too: ours, or the "Open with > Choose
/// another app" entry for our executable (`Applications\patorrent.exe`).
fn is_ours(progid: &str, ours: &str, exe: &Path) -> bool {
    let name = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    progid.eq_ignore_ascii_case(ours)
        || (!name.is_empty() && progid.to_lowercase().ends_with(&format!("\\{name}")))
}

/// Reads what is registered for `exe`.
pub fn read(keys: &Keys, exe: &Path) -> Status {
    let command = |progid: &str| get(&keys.class(&format!(r"{progid}\shell\open\command")), None);
    let ours = |progid: &str| command(progid).is_some_and(|c| runs(&c, exe));
    let torrent = get(&keys.class(".torrent"), None).is_some_and(|p| p.eq_ignore_ascii_case(PROGID_TORRENT))
        && ours(PROGID_TORRENT);
    let magnet = ours("magnet");
    let run = get(&keys.path(RUN), Some(APP)).filter(|c| runs(c, exe));
    let set = Integration {
        torrent,
        magnet,
        autostart: run.is_some(),
        minimized: run.is_some_and(|c| c.split_whitespace().any(|a| a.eq_ignore_ascii_case(MINIMIZED))),
    };
    let choice = |path: &str, progid: &str| {
        get(&keys.path(path), Some("ProgId")).filter(|p| !p.is_empty() && !is_ours(p, progid, exe))
    };
    Status {
        set,
        torrent_choice: if torrent { choice(TORRENT_CHOICE, PROGID_TORRENT) } else { None },
        // `magnet` itself is the scheme's own key, which now runs PaTorrent
        magnet_choice: if magnet {
            choice(MAGNET_CHOICE, PROGID_MAGNET).filter(|p| !p.eq_ignore_ascii_case("magnet"))
        } else {
            None
        },
    }
}

/// Changes the registration from `old` (as [`read`] found it) to `want`, then reads it back.
/// Only what differs is written; turning an association off removes only what names PaTorrent.
pub fn write(keys: &Keys, exe: &Path, old: &Integration, want: &Integration) -> io::Result<Status> {
    let want = want.normalized();
    let old = old.normalized();
    let exe_s = exe_text(exe);
    let icon = format!("\"{exe_s}\",0");
    let open = open_command(exe);
    let mut assoc_changed = false;

    if want.torrent != old.torrent {
        assoc_changed = true;
        let progid = keys.class(PROGID_TORRENT);
        let ext = keys.class(".torrent");
        if want.torrent {
            set(&progid, None, tr!(AssocTorrentType))?;
            set(&format!(r"{progid}\DefaultIcon"), None, &icon)?;
            set(&format!(r"{progid}\shell\open\command"), None, &open)?;
            set(&ext, None, PROGID_TORRENT)?;
            set(&ext, Some("Content Type"), "application/x-bittorrent")?;
            set(&format!(r"{ext}\OpenWithProgids"), Some(PROGID_TORRENT), "")?;
        } else {
            if get(&ext, None).is_some_and(|p| p.eq_ignore_ascii_case(PROGID_TORRENT)) {
                delete_value(&ext, None)?;
            }
            delete_value(&format!(r"{ext}\OpenWithProgids"), Some(PROGID_TORRENT))?;
            delete_tree(&progid)?;
        }
    }

    if want.magnet != old.magnet {
        assoc_changed = true;
        let progid = keys.class(PROGID_MAGNET);
        let scheme = keys.class("magnet");
        if want.magnet {
            for key in [&progid, &scheme] {
                set(key, None, &format!("URL:{}", tr!(AssocMagnetType)))?;
                set(key, Some("URL Protocol"), "")?;
                set(&format!(r"{key}\DefaultIcon"), None, &icon)?;
                set(&format!(r"{key}\shell\open\command"), None, &open)?;
            }
        } else {
            if get(&format!(r"{scheme}\shell\open\command"), None).is_some_and(|c| runs(&c, exe)) {
                delete_tree(&scheme)?;
            }
            delete_tree(&progid)?;
        }
    }

    if assoc_changed {
        // Default apps lists the program while it handles anything
        let caps = keys.path(CAPABILITIES);
        if want.torrent || want.magnet {
            set(&caps, Some("ApplicationName"), APP)?;
            set(&caps, Some("ApplicationDescription"), tr!(AppDescription))?;
            set(&caps, Some("ApplicationIcon"), &icon)?;
            let files = format!(r"{caps}\FileAssociations");
            let urls = format!(r"{caps}\URLAssociations");
            if want.torrent {
                set(&files, Some(".torrent"), PROGID_TORRENT)?;
            } else {
                delete_value(&files, Some(".torrent"))?;
            }
            if want.magnet {
                set(&urls, Some("magnet"), PROGID_MAGNET)?;
            } else {
                delete_value(&urls, Some("magnet"))?;
            }
            // the value names the key relative to the hive
            set(&keys.path(REGISTERED), Some(APP), &keys.path(CAPABILITIES))?;
        } else {
            delete_value(&keys.path(REGISTERED), Some(APP))?;
            delete_tree(&caps)?;
            // only if nothing else is kept there
            delete_empty(&keys.path(APP_KEY));
        }
    }

    if want.autostart != old.autostart || want.minimized != old.minimized {
        let run = keys.path(RUN);
        if want.autostart {
            set(&run, Some(APP), &run_command(exe, want.minimized))?;
        } else if get(&run, Some(APP)).is_some_and(|c| runs(&c, exe)) {
            // another copy's entry stays
            delete_value(&run, Some(APP))?;
        }
    }

    if assoc_changed && keys.is_real() {
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, null(), null()) };
    }
    Ok(read(keys, exe))
}

/// The installer's `--associate`: makes `exe` the program for `.torrent` files and/or `magnet:`
/// links, as Preferences > General does; what is not asked for stays as it is.
pub fn associate(keys: &Keys, exe: &Path, torrent: bool, magnet: bool) -> io::Result<Status> {
    let old = read(keys, exe).set;
    let want = Integration { torrent: old.torrent || torrent, magnet: old.magnet || magnet, ..old };
    write(keys, exe, &old, &want)
}

/// The uninstaller's `--unregister`: removes the associations, the Default apps entry and the
/// autostart entry where they name `exe`. Unlike [`read`], a ProgID of ours counts although
/// `.torrent` or the scheme now names another program, so nothing is left behind that starts
/// the deleted executable; another copy's registration stays.
pub fn unregister(keys: &Keys, exe: &Path) -> io::Result<Status> {
    let ours = |key: &str| {
        get(&keys.class(&format!(r"{key}\shell\open\command")), None).is_some_and(|c| runs(&c, exe))
    };
    let old = Integration {
        torrent: ours(PROGID_TORRENT),
        magnet: ours(PROGID_MAGNET) || ours("magnet"),
        autostart: get(&keys.path(RUN), Some(APP)).is_some_and(|c| runs(&c, exe)),
        minimized: false,
    };
    write(keys, exe, &old, &Integration::default())
}

/// Opens Settings > Default apps, on PaTorrent's own page where Windows has one (Windows 11).
pub fn open_default_apps(owner: HWND) {
    let target = wide(&format!("ms-settings:defaultapps?registeredAppUser={APP}"));
    let verb = wide("open");
    unsafe { ShellExecuteW(owner, verb.as_ptr(), target.as_ptr(), null(), null(), SW_SHOWNORMAL) };
}

fn check(e: WIN32_ERROR) -> io::Result<()> {
    if e == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(e as i32)) }
}

/// Like [`check`], but a missing key or value is fine.
fn check_gone(e: WIN32_ERROR) -> io::Result<()> {
    if e == ERROR_FILE_NOT_FOUND { Ok(()) } else { check(e) }
}

fn opt_wide(s: Option<&str>) -> Option<Vec<u16>> {
    s.map(wide)
}

fn ptr(w: &Option<Vec<u16>>) -> *const u16 {
    w.as_ref().map_or(null(), |v| v.as_ptr())
}

/// A string value (`None` = the key's default value).
fn get(path: &str, name: Option<&str>) -> Option<String> {
    get_in(HKEY_CURRENT_USER, path, name)
}

/// A string value under `root` (`None` = the key's default value); the 64-bit view, as the
/// process is 64-bit.
pub(crate) fn get_in(root: HKEY, path: &str, name: Option<&str>) -> Option<String> {
    let p = wide(path);
    let n = opt_wide(name);
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    unsafe {
        let mut bytes = 0u32;
        let e = RegGetValueW(root, p.as_ptr(), ptr(&n), flags, null_mut(), null_mut(), &mut bytes);
        if e != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; (bytes as usize).div_ceil(2) + 1];
        bytes = (buf.len() * 2) as u32;
        let e =
            RegGetValueW(root, p.as_ptr(), ptr(&n), flags, null_mut(), buf.as_mut_ptr().cast(), &mut bytes);
        (e == ERROR_SUCCESS).then(|| crate::util::from_wide(&buf))
    }
}

/// Sets a string value, creating the key.
fn set(path: &str, name: Option<&str>, value: &str) -> io::Result<()> {
    let p = wide(path);
    let n = opt_wide(name);
    let v = wide(value);
    unsafe {
        let mut key: HKEY = null_mut();
        check(RegCreateKeyExW(
            HKEY_CURRENT_USER,
            p.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            null(),
            &mut key,
            null_mut(),
        ))?;
        let r = RegSetValueExW(key, ptr(&n), 0, REG_SZ, v.as_ptr().cast(), (v.len() * 2) as u32);
        RegCloseKey(key);
        check(r)
    }
}

fn delete_value(path: &str, name: Option<&str>) -> io::Result<()> {
    let p = wide(path);
    let n = opt_wide(name);
    unsafe {
        let mut key: HKEY = null_mut();
        let e = RegOpenKeyExW(HKEY_CURRENT_USER, p.as_ptr(), 0, KEY_WRITE, &mut key);
        if e != ERROR_SUCCESS {
            return check_gone(e);
        }
        let r = RegDeleteValueW(key, ptr(&n));
        RegCloseKey(key);
        check_gone(r)
    }
}

/// Deletes a key and everything under it.
fn delete_tree(path: &str) -> io::Result<()> {
    let p = wide(path);
    unsafe {
        check_gone(RegDeleteTreeW(HKEY_CURRENT_USER, p.as_ptr()))?;
        // RegDeleteTreeW empties the key; the key itself goes with RegDeleteKeyW
        check_gone(RegDeleteKeyW(HKEY_CURRENT_USER, p.as_ptr()))
    }
}

/// Deletes a key that has no subkeys left (a key with subkeys stays).
fn delete_empty(path: &str) {
    let p = wide(path);
    unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, p.as_ptr()) };
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A scratch tree under HKCU, removed on drop.
    struct Scratch {
        root: String,
        keys: Keys,
    }

    impl Scratch {
        fn new() -> Scratch {
            static N: AtomicU32 = AtomicU32::new(0);
            let root = format!(
                r"Software\PaTorrent-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            );
            Scratch { keys: Keys::under(&root), root }
        }

        fn get(&self, path: &str, name: Option<&str>) -> Option<String> {
            get(&format!(r"{}\{path}", self.root), name)
        }

        fn set(&self, path: &str, name: Option<&str>, value: &str) {
            set(&format!(r"{}\{path}", self.root), name, value).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = delete_tree(&self.root);
        }
    }

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files\PaTorrent\patorrent.exe")
    }

    const ALL: Integration = Integration { torrent: true, magnet: true, autostart: true, minimized: true };

    #[test]
    fn nothing_registered_reads_as_off() {
        let s = Scratch::new();
        assert_eq!(read(&s.keys, &exe()), Status::default());
    }

    #[test]
    fn everything_on_and_off_again() {
        let s = Scratch::new();
        let st = write(&s.keys, &exe(), &Integration::default(), &ALL).unwrap();
        assert_eq!(st, Status { set: ALL, ..Status::default() });
        assert!(st.opens_torrents() && st.opens_magnets());

        let open = r#""C:\Program Files\PaTorrent\patorrent.exe" --shell "%1""#;
        assert_eq!(s.get(r"Software\Classes\.torrent", None).as_deref(), Some(PROGID_TORRENT));
        assert_eq!(
            s.get(r"Software\Classes\.torrent", Some("Content Type")).as_deref(),
            Some("application/x-bittorrent")
        );
        assert_eq!(
            s.get(r"Software\Classes\.torrent\OpenWithProgids", Some(PROGID_TORRENT)).as_deref(),
            Some("")
        );
        assert_eq!(
            s.get(r"Software\Classes\PaTorrent.torrent\shell\open\command", None).as_deref(),
            Some(open)
        );
        assert_eq!(s.get(r"Software\Classes\magnet", Some("URL Protocol")).as_deref(), Some(""));
        assert_eq!(s.get(r"Software\Classes\magnet\shell\open\command", None).as_deref(), Some(open));
        assert_eq!(
            s.get(r"Software\Classes\PaTorrent.magnet\shell\open\command", None).as_deref(),
            Some(open)
        );
        assert_eq!(
            s.get(r"Software\PaTorrent\Capabilities\FileAssociations", Some(".torrent")).as_deref(),
            Some(PROGID_TORRENT)
        );
        assert_eq!(
            s.get(r"Software\PaTorrent\Capabilities\URLAssociations", Some("magnet")).as_deref(),
            Some(PROGID_MAGNET)
        );
        assert_eq!(
            s.get(r"Software\RegisteredApplications", Some("PaTorrent")),
            Some(format!(r"{}\Software\PaTorrent\Capabilities", s.root))
        );
        assert_eq!(
            s.get(r"Software\Microsoft\Windows\CurrentVersion\Run", Some("PaTorrent")).as_deref(),
            Some(r#""C:\Program Files\PaTorrent\patorrent.exe" --minimized"#)
        );

        let st = write(&s.keys, &exe(), &ALL, &Integration::default()).unwrap();
        assert_eq!(st, Status::default());
        assert_eq!(s.get(r"Software\Classes\.torrent", None), None);
        assert_eq!(s.get(r"Software\Classes\PaTorrent.torrent", None), None);
        assert_eq!(s.get(r"Software\Classes\magnet", None), None);
        assert_eq!(s.get(r"Software\RegisteredApplications", Some("PaTorrent")), None);
        assert_eq!(s.get(r"Software\PaTorrent\Capabilities", Some("ApplicationName")), None);
        assert_eq!(s.get(r"Software\Microsoft\Windows\CurrentVersion\Run", Some("PaTorrent")), None);
    }

    #[test]
    fn only_what_changed_is_written() {
        let s = Scratch::new();
        let torrent = Integration { torrent: true, ..Integration::default() };
        write(&s.keys, &exe(), &Integration::default(), &torrent).unwrap();
        // another client took the magnet scheme meanwhile; turning autostart on leaves it alone
        s.set(r"Software\Classes\magnet\shell\open\command", None, r#""C:\qbt\qbittorrent.exe" "%1""#);
        let st = write(&s.keys, &exe(), &torrent, &Integration { autostart: true, ..torrent }).unwrap();
        assert_eq!(st.set, Integration { torrent: true, autostart: true, ..Integration::default() });
        assert_eq!(
            s.get(r"Software\Classes\magnet\shell\open\command", None).as_deref(),
            Some(r#""C:\qbt\qbittorrent.exe" "%1""#)
        );
        // and turning magnets off does not delete the other client's scheme
        write(&s.keys, &exe(), &Integration { magnet: true, ..torrent }, &torrent).unwrap();
        assert!(s.get(r"Software\Classes\magnet\shell\open\command", None).is_some());
    }

    #[test]
    fn minimized_follows_autostart() {
        let s = Scratch::new();
        let want = Integration { autostart: true, ..Integration::default() };
        let st = write(&s.keys, &exe(), &Integration::default(), &want).unwrap();
        assert_eq!(st.set, want);
        assert_eq!(
            s.get(r"Software\Microsoft\Windows\CurrentVersion\Run", Some("PaTorrent")).as_deref(),
            Some(r#""C:\Program Files\PaTorrent\patorrent.exe""#)
        );
        let min = Integration { minimized: true, ..want };
        assert_eq!(write(&s.keys, &exe(), &want, &min).unwrap().set, min);
        // without autostart the switch means nothing
        let st =
            write(&s.keys, &exe(), &min, &Integration { minimized: true, ..Integration::default() }).unwrap();
        assert_eq!(st.set, Integration::default());
    }

    #[test]
    fn another_copy_is_not_this_one() {
        let s = Scratch::new();
        write(&s.keys, Path::new(r"D:\portable\patorrent.exe"), &Integration::default(), &ALL).unwrap();
        assert_eq!(read(&s.keys, &exe()).set, Integration::default());
        // so unchecking here (nothing to do) and checking again takes them over
        let st = write(&s.keys, &exe(), &Integration::default(), &ALL).unwrap();
        assert_eq!(st.set, ALL);
        // the paths compare without regard to case
        let upper = PathBuf::from(exe().to_string_lossy().to_uppercase());
        assert_eq!(read(&s.keys, &upper).set, ALL);
        // turning autostart off in another copy leaves this copy's entry
        let other = Path::new(r"D:\portable\patorrent.exe");
        let back = Integration { autostart: false, minimized: false, ..ALL };
        write(&s.keys, other, &ALL, &back).unwrap();
        assert!(read(&s.keys, &exe()).set.autostart);
    }

    #[test]
    fn windows_own_choice_is_reported() {
        let s = Scratch::new();
        let st = write(&s.keys, &exe(), &Integration::default(), &ALL).unwrap();
        assert_eq!(st.note(), "");
        s.set(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.torrent\UserChoice",
            Some("ProgId"),
            "qBittorrent.File.Torrent",
        );
        let st = read(&s.keys, &exe());
        assert_eq!(st.torrent_choice.as_deref(), Some("qBittorrent.File.Torrent"));
        assert!(!st.opens_torrents() && st.opens_magnets());
        assert_eq!(st.note(), "Windows still opens .torrent files with qBittorrent.File.Torrent.");
        // our own ProgIDs, the scheme's key and "Open with" on our executable are PaTorrent
        for ours in [PROGID_TORRENT, r"Applications\patorrent.exe"] {
            s.set(
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.torrent\UserChoice",
                Some("ProgId"),
                ours,
            );
            assert!(read(&s.keys, &exe()).opens_torrents(), "{ours}");
        }
        for (choice, ours) in [(PROGID_MAGNET, true), ("magnet", true), ("qBittorrent.Url.Magnet", false)] {
            s.set(
                r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\magnet\UserChoice",
                Some("ProgId"),
                choice,
            );
            assert_eq!(read(&s.keys, &exe()).opens_magnets(), ours, "{choice}");
        }
        // a choice is only reported while PaTorrent is registered
        let st = write(&s.keys, &exe(), &ALL, &Integration::default()).unwrap();
        assert_eq!(st, Status::default());
    }

    #[test]
    fn associate_adds_to_what_is_there() {
        let s = Scratch::new();
        let autostart = Integration { autostart: true, ..Integration::default() };
        write(&s.keys, &exe(), &Integration::default(), &autostart).unwrap();
        let st = associate(&s.keys, &exe(), true, false).unwrap();
        assert_eq!(st.set, Integration { torrent: true, ..autostart });
        assert_eq!(s.get(r"Software\Classes\magnet", None), None);
        // asking for magnets later keeps the torrent association
        let st = associate(&s.keys, &exe(), false, true).unwrap();
        assert_eq!(st.set, Integration { torrent: true, magnet: true, ..autostart });
        // and another program's `.torrent` is taken over, as the General page does
        s.set(r"Software\Classes\.torrent", None, "qBittorrent.File.Torrent");
        assert!(associate(&s.keys, &exe(), true, false).unwrap().set.torrent);
    }

    #[test]
    fn unregister_removes_what_names_this_copy() {
        let s = Scratch::new();
        write(&s.keys, &exe(), &Integration::default(), &ALL).unwrap();
        // another client took `.torrent` and the scheme since: read() no longer counts them
        s.set(r"Software\Classes\.torrent", None, "qBittorrent.File.Torrent");
        s.set(r"Software\Classes\magnet\shell\open\command", None, r#""C:\qbt\qbittorrent.exe" "%1""#);
        assert_eq!(
            read(&s.keys, &exe()).set,
            Integration { autostart: true, minimized: true, ..Integration::default() }
        );
        assert_eq!(unregister(&s.keys, &exe()).unwrap(), Status::default());
        // our ProgIDs, the Default apps entry and autostart are gone ...
        assert_eq!(s.get(r"Software\Classes\PaTorrent.torrent\shell\open\command", None), None);
        assert_eq!(s.get(r"Software\Classes\PaTorrent.magnet\shell\open\command", None), None);
        assert_eq!(s.get(r"Software\Classes\.torrent\OpenWithProgids", Some(PROGID_TORRENT)), None);
        assert_eq!(s.get(r"Software\RegisteredApplications", Some("PaTorrent")), None);
        assert_eq!(s.get(r"Software\PaTorrent\Capabilities", Some("ApplicationName")), None);
        assert_eq!(s.get(r"Software\Microsoft\Windows\CurrentVersion\Run", Some("PaTorrent")), None);
        // ... the other client's are not
        assert_eq!(s.get(r"Software\Classes\.torrent", None).as_deref(), Some("qBittorrent.File.Torrent"));
        assert_eq!(
            s.get(r"Software\Classes\magnet\shell\open\command", None).as_deref(),
            Some(r#""C:\qbt\qbittorrent.exe" "%1""#)
        );
        // nothing left: a second run changes nothing
        assert_eq!(unregister(&s.keys, &exe()).unwrap(), Status::default());
    }

    #[test]
    fn unregister_leaves_another_copy() {
        let s = Scratch::new();
        let other = Path::new(r"D:\portable\patorrent.exe");
        write(&s.keys, other, &Integration::default(), &ALL).unwrap();
        assert_eq!(unregister(&s.keys, &exe()).unwrap(), Status::default());
        assert_eq!(read(&s.keys, other).set, ALL);
        assert!(s.get(r"Software\RegisteredApplications", Some("PaTorrent")).is_some());
    }

    /// SEC-18: an association from before `--shell` is rewritten; anyone else's stays.
    #[test]
    fn old_open_commands_are_upgraded() {
        let s = Scratch::new();
        let old = r#""C:\Program Files\PaTorrent\patorrent.exe" "%1""#;
        let qbt = r#""C:\qbt\qbittorrent.exe" "%1""#;
        s.set(r"Software\Classes\PaTorrent.torrent\shell\open\command", None, old);
        s.set(r"Software\Classes\magnet\shell\open\command", None, qbt);
        upgrade(&s.keys, &exe()).unwrap();
        assert_eq!(
            s.get(r"Software\Classes\PaTorrent.torrent\shell\open\command", None),
            Some(open_command(&exe()))
        );
        assert_eq!(s.get(r"Software\Classes\magnet\shell\open\command", None).as_deref(), Some(qbt));
        assert_eq!(s.get(r"Software\Classes\PaTorrent.magnet\shell\open\command", None), None);
    }

    #[test]
    fn commands() {
        assert_eq!(
            open_command(Path::new(r"\\?\C:\a b\patorrent.exe")),
            r#""C:\a b\patorrent.exe" --shell "%1""#
        );
        assert_eq!(run_command(&exe(), false), r#""C:\Program Files\PaTorrent\patorrent.exe""#);
        assert!(runs(r#""c:\program files\patorrent\PATORRENT.EXE" "%1""#, &exe()));
        assert!(!runs(r#""C:\Program Files\PaTorrent\patorrent.exe.old" "%1""#, &exe()));
    }
}
