//! Desktop integration (Preferences > General): PaTorrent as the program for `.torrent` files
//! and `magnet:` links, and the "Start PaTorrent on system startup" entry. [`Integration`] is
//! what the boxes say; each GUI reads and writes it in its system's own places (the Win32
//! client in the registry, the GTK one in XDG desktop entries and `mimeapps.list`), never in
//! `patorrent.ini`.
//!
//! The rest are the freedesktop.org desktop entries the GTK client writes, as text: [`entry`]
//! builds one and [`exec_args`] reads its `Exec` line back, so a box is checked only when the
//! entry starts this very program (a moved AppImage shows unchecked until it registers again).
//! Written from the Desktop Entry Specification 1.5 ("The Exec key", "Possible value types").

use crate::tr;

/// The GTK client's application id: its D-Bus name, desktop entry and icon name.
pub const APP_ID: &str = "io.github.balls0fsteel.PaTorrent";

/// The application's desktop entry as packages install it (AppImage, Flatpak, tarballs):
/// `contrib/linux/<APP_ID>.desktop`, which a test keeps equal to [`entry`]'s.
pub fn packaged_entry() -> String {
    entry(&["patorrent-gtk".to_owned()], APP_ID, EntryKind::App)
}

/// The content type of `.torrent` files and the scheme of `magnet:` links, as XDG names them.
pub const MIME_TORRENT: &str = "application/x-bittorrent";
pub const MIME_MAGNET: &str = "x-scheme-handler/magnet";

/// The command-line switch the autostart entry passes for "Start minimized".
pub const MINIMIZED: &str = "--minimized";

/// What PaTorrent is registered for (or is to be).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Integration {
    /// "Associate with .torrent files".
    pub torrent: bool,
    /// "Associate with magnet: links".
    pub magnet: bool,
    /// "Start PaTorrent on system startup".
    pub autostart: bool,
    /// "Start minimized" (only with `autostart`: the entry passes [`MINIMIZED`]).
    pub minimized: bool,
}

impl Integration {
    /// `minimized` means nothing without `autostart`.
    pub fn normalized(mut self) -> Integration {
        self.minimized &= self.autostart;
        self
    }
}

/// Which desktop entry [`entry`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// The application's entry in `applications/`: the menu item and the handler of the
    /// content types (`Exec` ends in `%U`).
    App,
    /// The entry in `autostart/`, started at login.
    Autostart { minimized: bool },
}

/// A desktop entry for `command` (the program and any arguments it always takes, as plain
/// text) with icon `icon` (a theme name). A command with `--profile <name>` (D42) makes a named
/// profile's entry: `Name=PaTorrent (name)` and, for its application entry, `NoDisplay=true`
/// (the desktop matches the profile's windows to it; File > Profiles makes it a menu item).
pub fn entry(command: &[String], icon: &str, kind: EntryKind) -> String {
    entry_shown(command, icon, kind, false)
}

/// [`entry`], with a named profile's application entry in the menu when `shown`.
pub fn entry_shown(command: &[String], icon: &str, kind: EntryKind, shown: bool) -> String {
    let profile = command_profile(command);
    let mut args: Vec<String> = command.iter().map(|a| exec_arg(a)).collect();
    match kind {
        EntryKind::App => args.push("%U".into()),
        EntryKind::Autostart { minimized: true } => args.push(MINIMIZED.into()),
        EntryKind::Autostart { minimized: false } => {}
    }
    let name = profile.as_ref().map_or("PaTorrent".to_owned(), |p| format!("PaTorrent ({p})"));
    let mut s = format!("[Desktop Entry]\nType=Application\nVersion=1.5\nName={}\n", string_value(&name));
    s += "GenericName=BitTorrent Client\n";
    s += &format!("Comment={}\n", string_value(tr!(AboutShort)));
    s += &format!("Exec={}\n", args.join(" "));
    s += &format!("Icon={}\n", string_value(icon));
    s += "Terminal=false\nCategories=Network;FileTransfer;P2P;GTK;\n";
    match kind {
        EntryKind::App => {
            s += &format!("MimeType={MIME_TORRENT};{MIME_MAGNET};\nStartupNotify=true\n");
            if profile.is_some() && !shown {
                s += "NoDisplay=true\n";
            }
        }
        EntryKind::Autostart { .. } => s += "X-GNOME-Autostart-enabled=true\n",
    }
    s
}

/// The profile a command names with `--profile` (`None`: the default profile, or a bad name).
fn command_profile(command: &[String]) -> Option<String> {
    crate::profile::split_args(command).ok().and_then(|(name, _)| name)
}

/// Whether a desktop entry's text is a menu item (not `NoDisplay=true`).
pub fn shown(text: &str) -> bool {
    !text.lines().any(|l| l.trim() == "NoDisplay=true")
}

/// A value of type string: backslash, newline, tab and carriage return escaped.
fn string_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Characters that make an `Exec` argument need quotes.
const RESERVED: &str = " \t\n\"'\\><~|&;$*?#()`";

/// One argument of an `Exec` line as it is written in the file: `%` doubled, quoted when it has
/// a reserved character (`"`, `` ` ``, `$` and `\` escaped inside the quotes), then escaped
/// again as a string value.
pub fn exec_arg(arg: &str) -> String {
    let a = arg.replace('%', "%%");
    let a = if a.is_empty() || a.chars().any(|c| RESERVED.contains(c)) {
        let mut q = String::from("\"");
        for c in a.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                q.push('\\');
            }
            q.push(c);
        }
        q.push('"');
        q
    } else {
        a
    };
    string_value(&a)
}

/// The `Exec` line's arguments in a desktop entry's text, as plain text (field codes such as
/// `%U` kept as they are, `%%` as `%`); `None` without one.
pub fn exec_args(text: &str) -> Option<Vec<String>> {
    let mut in_entry = false;
    let mut value = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry {
            if let Some(v) = line.strip_prefix("Exec").and_then(|r| r.trim_start().strip_prefix('=')) {
                value = Some(v.trim_start().to_owned());
                break;
            }
        }
    }
    let value = unescape_string(&value?);
    let mut args = Vec::new();
    let mut chars = value.chars().peekable();
    loop {
        while chars.next_if(|c| *c == ' ' || *c == '\t').is_some() {}
        let Some(&first) = chars.peek() else { break };
        let mut arg = String::new();
        if first == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => arg.extend(chars.next()),
                    c => arg.push(c),
                }
            }
        } else {
            while let Some(c) = chars.next_if(|c| *c != ' ' && *c != '\t') {
                arg.push(c);
            }
        }
        args.push(arg.replace("%%", "%"));
    }
    Some(args)
}

fn unescape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(c) => out.push(c),
            None => out.push('\\'),
        }
    }
    out
}

/// Whether a desktop entry's text starts `command` (its first arguments are `command`'s) in the
/// same profile: the default profile's command does not start a named profile's entry.
pub fn starts(text: &str, command: &[String]) -> bool {
    !command.is_empty()
        && exec_args(text)
            .is_some_and(|a| a.starts_with(command) && command_profile(&a) == command_profile(command))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn exec_arguments_are_quoted_and_read_back() {
        assert_eq!(exec_arg("/usr/bin/patorrent-gtk"), "/usr/bin/patorrent-gtk");
        assert_eq!(exec_arg("/opt/Pa Torrent/run"), "\"/opt/Pa Torrent/run\"");
        assert_eq!(exec_arg("100%"), "100%%");
        // a backslash: escaped inside the quotes, then again as a string value
        assert_eq!(exec_arg("a\\b"), "\"a\\\\\\\\b\"");
        assert_eq!(exec_arg(""), "\"\"");
        for weird in [
            "/home/x/Apps/PaTorrent x86_64.AppImage",
            "/tmp/$HOME/`id`/\"q\"/back\\slash/50%/tab\there",
            "/plain/path",
        ] {
            let text = entry(&cmd(&[weird]), "icon", EntryKind::App);
            assert_eq!(exec_args(&text), Some(cmd(&[weird, "%U"])), "{weird}");
            assert!(starts(&text, &cmd(&[weird])));
            assert!(!starts(&text, &cmd(&["/other"])));
        }
    }

    #[test]
    fn entries_for_the_menu_and_autostart() {
        let flatpak = cmd(&["flatpak", "run", "io.github.balls0fsteel.PaTorrent"]);
        let app = entry(&flatpak, "io.github.balls0fsteel.PaTorrent", EntryKind::App);
        assert!(app.starts_with("[Desktop Entry]\nType=Application\n"));
        assert!(app.contains("\nMimeType=application/x-bittorrent;x-scheme-handler/magnet;\n"));
        assert!(app.contains("\nExec=flatpak run io.github.balls0fsteel.PaTorrent %U\n"));
        let auto = entry(&flatpak, "x", EntryKind::Autostart { minimized: true });
        assert!(auto.contains("\nExec=flatpak run io.github.balls0fsteel.PaTorrent --minimized\n"));
        assert!(!auto.contains("MimeType"));
        assert!(exec_args(&auto).unwrap().iter().any(|a| a == MINIMIZED));
        let plain = entry(&flatpak, "x", EntryKind::Autostart { minimized: false });
        assert!(!exec_args(&plain).unwrap().iter().any(|a| a == MINIMIZED));
        assert!(starts(&plain, &flatpak));
    }

    /// A named profile's entry (D42): its own name, hidden from the menu until asked, and
    /// told apart from the default profile's.
    #[test]
    fn a_profiles_entry() {
        let work = cmd(&["/usr/bin/patorrent-gtk", "--profile", "work"]);
        let default = cmd(&["/usr/bin/patorrent-gtk"]);
        let app = entry(&work, APP_ID, EntryKind::App);
        assert!(app.contains("\nName=PaTorrent (work)\n") && app.contains("\nNoDisplay=true\n"));
        assert!(!shown(&app) && shown(&entry_shown(&work, APP_ID, EntryKind::App, true)));
        assert!(app.contains("\nExec=/usr/bin/patorrent-gtk --profile work %U\n"));
        assert!(starts(&app, &work) && !starts(&app, &default));
        let plain = entry(&default, APP_ID, EntryKind::App);
        assert!(!starts(&plain, &work) && starts(&plain, &default) && shown(&plain));
        let auto = entry(&work, APP_ID, EntryKind::Autostart { minimized: true });
        assert!(!auto.contains("NoDisplay") && auto.contains("\nName=PaTorrent (work)\n"));
    }

    #[test]
    fn the_packaged_desktop_entry_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contrib/linux")
            .join(format!("{APP_ID}.desktop"));
        let want = packaged_entry();
        if std::env::var_os("PATORRENT_WRITE_DESKTOP").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert!(have == want, "{} is stale: run the tests with PATORRENT_WRITE_DESKTOP=1", path.display());
        assert_eq!(exec_args(&have), Some(cmd(&["patorrent-gtk", "%U"])));
    }

    #[test]
    fn exec_lines_found_in_their_group_only() {
        let text = "[Desktop Action New]\nExec=/wrong\n\n[Desktop Entry]\nName=X\nExec = /right %f\n";
        assert_eq!(exec_args(text), Some(cmd(&["/right", "%f"])));
        assert_eq!(exec_args("[Desktop Entry]\nName=X\n"), None);
        assert!(!starts("[Desktop Entry]\nExec=/a\n", &[]));
        assert_eq!(
            Integration { minimized: true, ..Integration::default() }.normalized(),
            Integration::default()
        );
    }
}
