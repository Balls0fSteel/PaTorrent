//! Frontend preferences in a small `key=value` text file (values verbatim, one per line) (window layout, columns, default save
//! folder). Unknown keys are kept, so older and newer builds can share one file. The engine's
//! own settings are kept here too, as `lt.<name>` lines (see [`crate::settings`]).
//!
//! The keys in [`SEALED`] (the proxy password) are secrets: [`Prefs::get`] gives their plain
//! value, while [`Prefs::entries`] and the file hold them sealed ([`crate::secret`]: DPAPI on
//! Windows, plain elsewhere). Every frontend's store (the GUIs' `patorrent.ini`, nox's
//! `webui.json` `options`) goes through `set` / `entries`, so each gets the sealed form.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::columns::Column;

/// The keys whose values are secrets, sealed in the file ([`crate::secret`]).
pub const SEALED: &[&str] = &["lt.proxy_password"];

#[derive(Clone, Debug, Default)]
pub struct Prefs {
    path: Option<PathBuf>,
    /// Plain values.
    values: BTreeMap<String, String>,
    /// The stored (file) form of the [`SEALED`] keys that are set.
    sealed: BTreeMap<String, String>,
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

    /// Writes the file (through a temporary file, so a crash never leaves half of it).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, path)
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

    /// Every key and value as stored (the [`SEALED`] keys sealed), sorted by key. [`Prefs::set`]
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

    /// Sets a key. For a [`SEALED`] key the value is the plain one or a sealed one (as
    /// [`Prefs::entries`] gives it); a sealed one that does not open here (another account's,
    /// corrupt) reads as empty.
    pub fn set(&mut self, key: &str, value: impl ToString) {
        let value = value.to_string();
        if SEALED.contains(&key) {
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
            self.values.insert(key.to_owned(), plain);
            return;
        }
        self.values.insert(key.to_owned(), value);
    }

    pub fn remove(&mut self, key: &str) {
        self.values.remove(key);
        self.sealed.remove(key);
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
}
