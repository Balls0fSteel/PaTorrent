//! macOS's desktop integration as text, for the Mac GUI (`crates/cocoa/src/mac/`, D61; the GTK
//! bundle wrote the same agents before): the per-profile LaunchAgent property list that "Start
//! Rookparcel on system startup" writes ([`launch_agent`]) and reads back
//! ([`launch_agent_arguments`]), where it lives ([`agent_label`], [`agent_path`]), and what an
//! opened URL or a command-line argument names ([`target`]). Plain Rust, built and tested on
//! every system.

use std::path::{Path, PathBuf};

use super::APP_ID;

/// The `.torrent` type's UTI when LaunchServices does not know one for the extension: what the
/// bundle's `Info.plist` imports (`.github/scripts/macos_bundle.py`), as Transmission exports it.
pub const TORRENT_UTI: &str = "org.bittorrent.torrent";

/// The URL scheme of magnet links, as LaunchServices names it.
pub const MAGNET_SCHEME: &str = "magnet";

/// The LaunchAgent's label: the application id, `<APP_ID>.<name>` for a named profile, so each
/// profile starts at login apart from the others (the GTK bundle's labels, kept so its agents
/// keep working).
pub fn agent_label(profile: Option<&str>) -> String {
    match profile {
        Some(n) => format!("{APP_ID}.{n}"),
        None => APP_ID.to_owned(),
    }
}

/// `~/Library/LaunchAgents/<label>.plist` below `home`.
pub fn agent_path(home: &Path, profile: Option<&str>) -> PathBuf {
    home.join("Library").join("LaunchAgents").join(format!("{}.plist", agent_label(profile)))
}

/// A LaunchAgent property list that starts `program` (the executable and its arguments) at
/// login: `RunAtLoad`, in the graphical (Aqua) session only, as an interactive process (no
/// background throttling), with no `KeepAlive` (quitting Rookparcel is not a crash). launchd
/// reads `~/Library/LaunchAgents` when the user logs in; writing the file starts nothing now.
pub fn launch_agent(label: &str, program: &[String]) -> String {
    let mut s = String::from(concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
        "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        "<plist version=\"1.0\">\n<dict>\n",
    ));
    s.push_str(&format!("\t<key>Label</key>\n\t<string>{}</string>\n", escape(label)));
    s.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
    for arg in program {
        s.push_str(&format!("\t\t<string>{}</string>\n", escape(arg)));
    }
    s.push_str("\t</array>\n");
    s.push_str("\t<key>RunAtLoad</key>\n\t<true/>\n");
    s.push_str("\t<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>\n");
    s.push_str("\t<key>ProcessType</key>\n\t<string>Interactive</string>\n");
    s.push_str("</dict>\n</plist>\n");
    s
}

/// What a LaunchAgent written by [`launch_agent`] (or by hand, in XML) starts at login: its
/// `ProgramArguments`, or `Program` alone; `None` when it does not run at load or is
/// `Disabled`, or is not an XML plist (a binary one is not read: we never write one).
pub fn launch_agent_arguments(text: &str) -> Option<Vec<String>> {
    if !value_after_key(text, "RunAtLoad").is_some_and(|v| v.starts_with("<true/>")) {
        return None;
    }
    if value_after_key(text, "Disabled").is_some_and(|v| v.starts_with("<true/>")) {
        return None;
    }
    if let Some(v) = value_after_key(text, "ProgramArguments") {
        let body = v.strip_prefix("<array>")?;
        let body = &body[..body.find("</array>")?];
        let mut out = Vec::new();
        let mut rest = body.trim_start();
        while !rest.is_empty() {
            let (s, after) = string_value(rest)?;
            out.push(s);
            rest = after.trim_start();
        }
        return (!out.is_empty()).then_some(out);
    }
    let (program, _) = string_value(value_after_key(text, "Program")?)?;
    Some(vec![program])
}

/// The text right after `<key>key</key>` and the blanks that follow it.
fn value_after_key<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let tag = format!("<key>{key}</key>");
    let at = text.find(&tag)?;
    Some(text[at + tag.len()..].trim_start())
}

/// A `<string>` element at the start of `text`: its value and what follows it.
fn string_value(text: &str) -> Option<(String, &str)> {
    if let Some(after) = text.strip_prefix("<string/>") {
        return Some((String::new(), after));
    }
    let body = text.strip_prefix("<string>")?;
    let end = body.find("</string>")?;
    Some((unescape(&body[..end]), &body[end + "</string>".len()..]))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The five predefined entities and numeric references (what an XML plist may contain).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16).ok())
                .unwrap_or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A torrent the system or the command line hands over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A `.torrent` file.
    File(PathBuf),
    /// A `magnet:` link or the `http(s)://` URL of a `.torrent`.
    Link(String),
}

/// What `arg` names: a `magnet:`, `http://` or `https://` link as it is, a `file://` URL (as
/// `application:openURLs:` gives files) as its path, a plain path relative to `cwd`; `None` for
/// a switch (`--minimized`), an empty text and other URL schemes.
pub fn target(arg: &str, cwd: &Path) -> Option<Target> {
    let lower = arg.get(..8).unwrap_or(arg).to_ascii_lowercase();
    if lower.starts_with("magnet:") || lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(Target::Link(arg.to_owned()));
    }
    if let Some(rest) = arg.strip_prefix("file://") {
        // `file:///path` or `file://localhost/path`
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        if !path.starts_with('/') {
            return None;
        }
        let bytes = percent_decode(path);
        return String::from_utf8(bytes).ok().map(|p| Target::File(PathBuf::from(p)));
    }
    if arg.is_empty() || arg.starts_with("--") {
        return None;
    }
    // another scheme (`x-foo:...`), not a path: a path's first part has no `:` before a `/`
    let scheme = arg.find(':').is_some_and(|c| {
        c > 1
            && !arg[..c].contains('/')
            && arg[..c].chars().all(|ch| ch.is_ascii_alphanumeric() || "+-.".contains(ch))
    });
    if scheme {
        return None;
    }
    Some(Target::File(cwd.join(arg)))
}

/// `%XX` escapes decoded (a broken one is kept as it is).
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_launch_agent_is_read_back() {
        let program = args(&["/Applications/Rookparcel.app/Contents/MacOS/Rookparcel", "--minimized"]);
        let text = launch_agent("io.github.balls0fsteel.Rookparcel", &program);
        assert!(text.contains("<key>Label</key>\n\t<string>io.github.balls0fsteel.Rookparcel</string>"));
        assert!(text.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert_eq!(launch_agent_arguments(&text), Some(program));
    }

    #[test]
    fn odd_characters_survive_the_round_trip() {
        let program = args(&["/Users/a&b/<Rookparcel> \"x\"/Rookparcel", ""]);
        let text = launch_agent("l", &program);
        assert!(text.contains("a&amp;b/&lt;Rookparcel&gt;"));
        assert_eq!(launch_agent_arguments(&text), Some(program));
    }

    #[test]
    fn agents_that_do_not_start_at_login_are_not_ours() {
        let text = launch_agent("l", &args(&["/p"]));
        let off = text.replace("<key>RunAtLoad</key>\n\t<true/>", "<key>RunAtLoad</key>\n\t<false/>");
        assert_eq!(launch_agent_arguments(&off), None);
        let disabled = text.replace("<dict>\n", "<dict>\n\t<key>Disabled</key>\n\t<true/>\n");
        assert_eq!(launch_agent_arguments(&disabled), None);
        assert_eq!(launch_agent_arguments("bplist00\u{1}"), None);
        assert_eq!(launch_agent_arguments(""), None);
    }

    #[test]
    fn hand_written_agents_are_read() {
        // a `Program` key alone, entities and blanks as other tools write them
        let text = "<plist version=\"1.0\"><dict><key>Label</key><string>x</string>\
            <key>Program</key>  <string>/Applications/P&#x26;T.app/Contents/MacOS/Rookparcel</string>\
            <key>RunAtLoad</key> <true/></dict></plist>";
        assert_eq!(
            launch_agent_arguments(text),
            Some(args(&["/Applications/P&T.app/Contents/MacOS/Rookparcel"]))
        );
        let text = "<dict><key>RunAtLoad</key><true/><key>ProgramArguments</key>\n<array>\n\
            <string>/a</string><string/>\n<string>&#45;&#45;minimized &amp; &quot;more&quot;</string></array></dict>";
        assert_eq!(launch_agent_arguments(text), Some(args(&["/a", "", "--minimized & \"more\""])));
        // an empty list, or something other than strings in it
        let text = "<key>RunAtLoad</key><true/><key>ProgramArguments</key><array></array>";
        assert_eq!(launch_agent_arguments(text), None);
        let text =
            "<key>RunAtLoad</key><true/><key>ProgramArguments</key><array><integer>1</integer></array>";
        assert_eq!(launch_agent_arguments(text), None);
    }

    #[test]
    fn unknown_entities_are_left_alone() {
        assert_eq!(unescape("a &bogus; b & c &#xZZ;"), "a &bogus; b & c &#xZZ;");
        assert_eq!(unescape("&lt;&gt;&apos;&#169;"), "<>'\u{a9}");
    }

    #[test]
    fn agents_per_profile() {
        assert_eq!(agent_label(None), "io.github.balls0fsteel.Rookparcel");
        assert_eq!(agent_label(Some("work")), "io.github.balls0fsteel.Rookparcel.work");
        assert_eq!(
            agent_path(Path::new("/Users/u"), Some("vpn")),
            Path::new("/Users/u/Library/LaunchAgents/io.github.balls0fsteel.Rookparcel.vpn.plist")
        );
    }

    #[test]
    fn opened_urls_and_arguments() {
        let cwd = Path::new("/Users/u/Downloads");
        let file = |p: &str| Some(Target::File(PathBuf::from(p)));
        let link = |l: &str| Some(Target::Link(l.to_owned()));
        assert_eq!(
            target("file:///Users/u/My%20Files/x%C3%A9.torrent", cwd),
            file("/Users/u/My Files/xé.torrent")
        );
        assert_eq!(target("file://localhost/tmp/a.torrent", cwd), file("/tmp/a.torrent"));
        assert_eq!(
            target("file:///tmp/100%.torrent", cwd),
            file("/tmp/100%.torrent"),
            "a broken escape stays"
        );
        assert_eq!(target("magnet:?xt=urn:btih:abc", cwd), link("magnet:?xt=urn:btih:abc"));
        assert_eq!(target("MAGNET:?xt=urn:btih:abc", cwd), link("MAGNET:?xt=urn:btih:abc"));
        assert_eq!(target("https://example.org/a.torrent", cwd), link("https://example.org/a.torrent"));
        assert_eq!(target("/tmp/b.torrent", cwd), file("/tmp/b.torrent"));
        assert_eq!(target("c.torrent", cwd), Some(Target::File(cwd.join("c.torrent"))));
        assert_eq!(target("sub/d:e.torrent", cwd), Some(Target::File(cwd.join("sub/d:e.torrent"))));
        assert_eq!(target("--minimized", cwd), None);
        assert_eq!(target("", cwd), None);
        assert_eq!(target("x-other:thing", cwd), None);
        assert_eq!(target("file://server/share/a.torrent", cwd), None);
    }
}
