//! Frontend preferences in a small `key=value` text file (values verbatim, one per line) (window layout, columns, default save
//! folder). Unknown keys are kept, so older and newer builds can share one file. The engine's
//! own settings are kept here too, as `lt.<name>` lines (see [`crate::settings`]).
//!
//! The keys [`is_sealed_key`] names (the proxy password, the search sources' API keys) are
//! secrets: [`Prefs::get`] gives their plain
//! value, while [`Prefs::entries`] and the file hold them sealed ([`crate::secret`]: DPAPI on
//! Windows, plain elsewhere). Every frontend's store (the GUIs' `patorrent.ini`, nox's
//! `webui.json` `options`) goes through `set` / `entries`, so each gets the sealed form.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use patorrent_core::util::private_file;

use crate::columns::Column;

/// The fixed keys whose values are secrets, sealed in the file ([`crate::secret`]).
pub const SEALED: &[&str] = &["lt.proxy_password"];

/// Whether `key`'s value is a secret, sealed in the file: the [`SEALED`] keys and every
/// search source's API key (`search.source.<n>.key`, [`crate::search::source`]).
pub fn is_sealed_key(key: &str) -> bool {
    SEALED.contains(&key)
        || key
            .strip_prefix("search.source.")
            .and_then(|rest| rest.strip_suffix(".key"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

#[derive(Debug, Default)]
pub struct Prefs {
    path: Option<PathBuf>,
    /// Plain values.
    values: BTreeMap<String, String>,
    /// The stored (file) form of the sealed keys ([`is_sealed_key`]) that are set.
    sealed: BTreeMap<String, String>,
    /// Every `set` and `remove` since [`Prefs::take_changes`], once [`Prefs::track_changes`]
    /// turned it on: for a copy kept elsewhere (the Web UI a GUI hosts mirrors its
    /// preferences, D40).
    journal: Option<Vec<Change>>,
}

/// One recorded change: the key and its new plain value, `None` once removed.
pub type Change = (String, Option<String>);

impl Clone for Prefs {
    /// A copy starts with an empty journal (tracking stays on): the original's pending changes
    /// are the original's to hand on.
    fn clone(&self) -> Prefs {
        Prefs {
            path: self.path.clone(),
            values: self.values.clone(),
            sealed: self.sealed.clone(),
            journal: self.journal.as_ref().map(|_| Vec::new()),
        }
    }
}

impl Prefs {
    /// Loads `path`; a missing or unreadable file gives empty preferences that save to `path`.
    pub fn load(path: impl Into<PathBuf>) -> Prefs {
        let path = path.into();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        Prefs::from_raw(Some(path), parse(&text))
    }

    /// Preferences from `key=value` text, as [`Prefs::to_text`] writes it, with no file.
    pub fn from_text(text: &str) -> Prefs {
        Prefs::from_raw(None, parse(text))
    }

    fn from_raw(path: Option<PathBuf>, raw: BTreeMap<String, String>) -> Prefs {
        let mut p = Prefs { path, ..Prefs::default() };
        for (k, v) in raw {
            p.set(&k, v);
        }
        p
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Writes the file (through a temporary file, so a crash never leaves half of it), readable
    /// by its owner only on Unix, where the proxy password is plain ([`private_file`]).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            private_file::create_dir_all(dir)?;
        }
        private_file::write(path, self.to_text().as_bytes())
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# PaTorrent preferences\n");
        for (k, v) in self.entries() {
            s.push_str(k);
            s.push('=');
            // Verbatim, so Windows paths stay readable and hand-editable. Line breaks cannot
            // be represented and become spaces.
            s.push_str(&v.replace(['\r', '\n'], " "));
            s.push('\n');
        }
        s
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// Every key and value as stored (the [`is_sealed_key`] keys sealed), sorted by key. [`Prefs::set`]
    /// takes these back.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| {
            let v = self.sealed.get(k).unwrap_or(v);
            (k.as_str(), v.as_str())
        })
    }

    pub fn get_i32(&self, key: &str) -> Option<i32> {
        self.get(key)?.trim().parse().ok()
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some("1" | "true") => true,
            Some("0" | "false") => false,
            _ => default,
        }
    }

    /// Sets a key. For a sealed key ([`is_sealed_key`]) the value is the plain one or a sealed one (as
    /// [`Prefs::entries`] gives it); a sealed one that does not open here (another account's,
    /// corrupt) reads as empty.
    pub fn set(&mut self, key: &str, value: impl ToString) {
        let value = value.to_string();
        if is_sealed_key(key) {
            let plain = crate::secret::open(&value);
            let stored = if crate::secret::is_sealed(&value) {
                value
            } else if self.values.get(key) == Some(&plain) && self.sealed.contains_key(key) {
                // unchanged: no new DPAPI call, and the file does not change at every save
                return;
            } else {
                crate::secret::seal(&plain)
            };
            self.sealed.insert(key.to_owned(), stored);
            self.record(key, Some(&plain));
            self.values.insert(key.to_owned(), plain);
            return;
        }
        self.record(key, Some(&value));
        self.values.insert(key.to_owned(), value);
    }

    pub fn remove(&mut self, key: &str) {
        self.record(key, None);
        self.values.remove(key);
        self.sealed.remove(key);
    }

    /// Records a change for [`Prefs::take_changes`] when it is one (a value set to what it is
    /// already is not).
    fn record(&mut self, key: &str, value: Option<&str>) {
        let Some(j) = &mut self.journal else { return };
        if self.values.get(key).map(String::as_str) != value {
            j.push((key.to_owned(), value.map(str::to_owned)));
        }
    }

    /// Starts recording every change for a copy kept elsewhere ([`Prefs::take_changes`]).
    pub fn track_changes(&mut self) {
        if self.journal.is_none() {
            self.journal = Some(Vec::new());
        }
    }

    /// Stops recording (and drops what was recorded).
    pub fn stop_tracking(&mut self) {
        self.journal = None;
    }

    /// The changes since the last call, oldest first (empty unless [`Prefs::track_changes`]
    /// was called): the plain values, for the same process.
    pub fn take_changes(&mut self) -> Vec<Change> {
        self.journal.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Takes another copy's changes without recording them (they came from there).
    pub fn apply_changes(&mut self, changes: &[Change]) {
        let journal = self.journal.take();
        for (k, v) in changes {
            match v {
                Some(v) => self.set(k, v),
                None => self.remove(k),
            }
        }
        self.journal = journal;
    }

    pub fn set_bool(&mut self, key: &str, value: bool) {
        self.set(key, if value { "1" } else { "0" });
    }

    /// The torrent list's columns: `order` lists every column in display order with its width
    /// (96-DPI pixels, negative = hidden).
    pub fn columns(&self) -> Vec<(Column, i32)> {
        let mut out: Vec<(Column, i32)> = Vec::new();
        if let Some(s) = self.get("list.columns") {
            for item in s.split(',') {
                let Some((key, w)) = item.split_once(':') else { continue };
                let (Some(c), Ok(w)) = (Column::from_key(key.trim()), w.trim().parse::<i32>()) else {
                    continue;
                };
                if !out.iter().any(|(d, _)| *d == c) && w != 0 {
                    out.push((c, w));
                }
            }
        }
        // Columns added since the file was written go at the end with their defaults.
        for c in Column::ALL {
            if !out.iter().any(|(d, _)| *d == c) {
                let w = c.default_width();
                out.push((c, if c.default_visible() { w } else { -w }));
            }
        }
        out
    }

    pub fn set_columns(&mut self, cols: &[(Column, i32)]) {
        let s: Vec<String> = cols.iter().map(|(c, w)| format!("{}:{w}", c.key())).collect();
        self.set("list.columns", s.join(","));
    }

    /// `(column, descending)`.
    pub fn sort(&self) -> Option<(Column, bool)> {
        let s = self.get("list.sort")?;
        let (key, dir) = s.split_once(':').unwrap_or((s, "asc"));
        Some((Column::from_key(key)?, dir == "desc"))
    }

    pub fn set_sort(&mut self, sort: Option<(Column, bool)>) {
        match sort {
            Some((c, desc)) => {
                self.set("list.sort", format!("{}:{}", c.key(), if desc { "desc" } else { "asc" }))
            }
            None => {
                self.values.remove("list.sort");
            }
        }
    }
}

fn parse(text: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}');
        if line.trim_start().starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim().to_owned(), v.to_owned());
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut p = Prefs::default();
        p.set("save_path", r"C:\Users\x\Downloads");
        p.set("multi", "a\nb");
        p.set_bool("tray", true);
        p.set("w", 640);
        let q = Prefs::from_text(&p.to_text());
        assert_eq!(q.get("save_path"), Some(r"C:\Users\x\Downloads"));
        assert_eq!(q.get("multi"), Some("a b"));
        // Hand-written files use single backslashes, including before letters like `n`.
        let hand = parse(r"dir=\\server\new\x");
        assert_eq!(hand.get("dir").map(String::as_str), Some(r"\\server\new\x"));
        assert!(q.get_bool("tray", false));
        assert_eq!(q.get_i32("w"), Some(640));
    }

    #[test]
    fn the_proxy_password_is_sealed() {
        let mut p = Prefs::default();
        p.set("lt.proxy_password", "s3cret");
        p.set("lt.proxy_username", "me");
        assert_eq!(p.get("lt.proxy_password"), Some("s3cret"));
        let text = p.to_text();
        assert!(
            text.contains(
                "lt.proxy_username=me
"
            ),
            "{text}"
        );
        let stored = p.entries().find(|(k, _)| *k == "lt.proxy_password").unwrap().1.to_owned();
        if cfg!(windows) {
            assert!(stored.starts_with("dpapi:"), "{stored}");
            assert!(!text.contains("s3cret"), "{text}");
        } else {
            assert_eq!(stored, "s3cret");
        }
        // read back from the text, and from the entries (nox's webui.json)
        let q = Prefs::from_text(&text);
        assert_eq!(q.get("lt.proxy_password"), Some("s3cret"));
        let mut r = Prefs::default();
        for (k, v) in q.entries() {
            r.set(k, v);
        }
        assert_eq!(r.get("lt.proxy_password"), Some("s3cret"));
        // setting the same value again keeps the stored form
        r.set("lt.proxy_password", "s3cret");
        assert_eq!(r.entries().find(|(k, _)| *k == "lt.proxy_password").unwrap().1, stored);
        r.remove("lt.proxy_password");
        assert!(!r.to_text().contains("proxy_password"));
    }

    #[test]
    fn search_source_keys_are_sealed_too() {
        assert!(is_sealed_key("lt.proxy_password"));
        assert!(is_sealed_key("search.source.0.key") && is_sealed_key("search.source.17.key"));
        for k in ["search.source.0.url", "search.source..key", "search.source.x.key", "search.source.0.keys"]
        {
            assert!(!is_sealed_key(k), "{k}");
        }
        let mut p = Prefs::default();
        p.set("search.source.3.key", "0123abcd");
        p.set("search.source.3.url", "http://localhost:9696/1/api");
        assert_eq!(p.get("search.source.3.key"), Some("0123abcd"));
        let text = p.to_text();
        if cfg!(windows) {
            assert!(!text.contains("0123abcd"), "{text}");
        }
        assert_eq!(Prefs::from_text(&text).get("search.source.3.key"), Some("0123abcd"));
    }

    #[test]
    fn a_plain_legacy_password_is_read_and_a_bad_blob_is_empty() {
        let p = Prefs::from_text(
            "lt.proxy_password=old plain
",
        );
        assert_eq!(p.get("lt.proxy_password"), Some("old plain"));
        if cfg!(windows) {
            assert!(!p.to_text().contains("old plain"), "sealed at the next save");
        }
        // another account's blob, or a corrupt one: empty, no error
        let q = Prefs::from_text(
            "lt.proxy_password=dpapi:AQIDBAUGBwgJCgsMDQ4PEA==
x=1
",
        );
        assert_eq!(q.get("lt.proxy_password"), Some(""));
        assert_eq!(q.get("x"), Some("1"));
        let q = Prefs::from_text(
            "lt.proxy_password=dpapi:%%%
",
        );
        assert_eq!(q.get("lt.proxy_password"), Some(""));
    }

    #[test]
    fn columns_and_sort() {
        let mut p = Prefs::default();
        let cols = p.columns();
        assert_eq!(cols.len(), Column::ALL.len());
        assert!(cols.iter().any(|&(c, w)| c == Column::Wasted && w < 0));

        p.set("list.columns", "status:100,name:300,bogus:5,name:7");
        let cols = p.columns();
        assert_eq!(cols[0], (Column::Status, 100));
        assert_eq!(cols[1], (Column::Name, 300));
        assert_eq!(cols.len(), Column::ALL.len());
        p.set_columns(&cols);
        assert_eq!(p.columns(), cols);

        p.set_sort(Some((Column::DownSpeed, true)));
        assert_eq!(p.sort(), Some((Column::DownSpeed, true)));
        p.set_sort(None);
        assert_eq!(p.sort(), None);
    }

    #[test]
    fn the_journal_records_changes_for_a_mirror() {
        let mut p = Prefs::default();
        p.set("a", "1");
        assert!(p.take_changes().is_empty(), "nothing is recorded until asked");
        p.track_changes();
        p.set("a", "1");
        p.set("a", "2");
        p.set("b", "x");
        p.remove("b");
        p.remove("missing");
        let changes = p.take_changes();
        assert_eq!(
            changes,
            [
                ("a".to_owned(), Some("2".to_owned())),
                ("b".to_owned(), Some("x".to_owned())),
                ("b".to_owned(), None)
            ]
        );
        assert!(p.take_changes().is_empty());

        // the other side takes them without recording them again; a copy starts empty
        let mut mirror = Prefs::default();
        mirror.track_changes();
        mirror.set("c", "3");
        mirror.apply_changes(&changes);
        assert_eq!(mirror.get("a"), Some("2"));
        assert_eq!(mirror.get("b"), None);
        assert_eq!(mirror.take_changes(), [("c".to_owned(), Some("3".to_owned()))]);
        p.set("d", "4");
        let copy = p.clone();
        assert_eq!(copy.get("d"), Some("4"));
        let mut copy = copy;
        assert!(copy.take_changes().is_empty());
        copy.set("e", "5");
        assert_eq!(copy.take_changes().len(), 1);
        assert_eq!(p.take_changes().len(), 1);
    }
}
