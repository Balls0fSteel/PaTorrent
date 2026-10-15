//! Frontend preferences in a small `key=value` text file (window layout, columns, default save
//! folder). Unknown keys are kept, so older and newer builds can share one file. The engine's
//! own settings join with the Preferences dialog (Phase 5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::columns::Column;

#[derive(Clone, Debug, Default)]
pub struct Prefs {
    path: Option<PathBuf>,
    values: BTreeMap<String, String>,
}

impl Prefs {
    /// Loads `path`; a missing or unreadable file gives empty preferences that save to `path`.
    pub fn load(path: impl Into<PathBuf>) -> Prefs {
        let path = path.into();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        Prefs { values: parse(&text), path: Some(path) }
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
        let mut s = String::from("# LibreTorrent preferences\n");
        for (k, v) in &self.values {
            s.push_str(k);
            s.push('=');
            s.push_str(&v.replace('\\', "\\\\").replace('\n', "\\n"));
            s.push('\n');
        }
        s
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
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

    pub fn set(&mut self, key: &str, value: impl ToString) {
        self.values.insert(key.to_owned(), value.to_string());
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
            m.insert(k.trim().to_owned(), unescape(v));
        }
    }
    m
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
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
        let q = Prefs { path: None, values: parse(&p.to_text()) };
        assert_eq!(q.get("save_path"), Some(r"C:\Users\x\Downloads"));
        assert_eq!(q.get("multi"), Some("a\nb"));
        assert!(q.get_bool("tray", false));
        assert_eq!(q.get_i32("w"), Some(640));
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
