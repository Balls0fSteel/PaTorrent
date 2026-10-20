//! String tables (D20): every user-visible string of the Win32 GUI, the GTK GUI and the Web UI, by
//! key, in one table per language. English ([`en`]) is the source and the fallback: a key a
//! translation lacks shows its English text.
//!
//! * Rust code takes a string with [`tr!`](crate::tr): `tr!(MenuFile)` is `&'static str`,
//!   `tr!(LogAdded, name = t.name)` fills `{name}` and gives a `String`.
//! * Texts keep µTorrent's `&` mnemonics (`&&` is a literal `&`). The Win32 GUI uses them as
//!   they are, GTK through [`gtk_mnemonic`], the Web UI through [`plain`].
//! * A translated table is a `.lng` text file: `Key=Text` lines, `#` comments, `\n` for a line
//!   break and `\\` for a backslash. `crates/frontend/lang/en.lng` is the English table written
//!   out for translators; a test keeps it equal to [`en`] (`PATORRENT_WRITE_LNG=1` rewrites it).
//! * The language is process-wide, picked once at start by [`init`]; tables are leaked, so
//!   texts are `&'static str`.

use std::fmt::Display;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

mod en;

pub use en::S;

/// `tr!(Key)` is the current language's text for [`S::Key`](S) (`&'static str`);
/// `tr!(Key, a = x, b)` fills the text's `{a}` and `{b}` placeholders (a `String`), `b` alone
/// meaning `b = b`.
#[macro_export]
macro_rules! tr {
    ($key:ident) => {
        $crate::lang::text($crate::lang::S::$key)
    };
    ($key:ident, $($name:ident $(= $value:expr)?),+ $(,)?) => {
        $crate::lang::fill(
            $crate::lang::text($crate::lang::S::$key),
            &[$((stringify!($name), &$crate::__tr_arg!($name $(= $value)?) as &dyn ::std::fmt::Display)),+],
        )
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __tr_arg {
    ($name:ident) => {
        $name
    };
    ($name:ident = $value:expr) => {
        $value
    };
}

/// A language PaTorrent ships.
pub struct Language {
    /// A BCP 47 tag: `de`, `pt-BR`, `zh-Hans`; what `ui.language` stores.
    pub code: &'static str,
    /// The language's own name for itself, as the Preferences list shows it (not translated).
    pub name: &'static str,
    /// Its `.lng` table; empty for English, whose texts are [`en`].
    pub lng: &'static str,
}

/// The languages PaTorrent ships, English first. A language is listed once its table is in.
pub const LANGUAGES: &[Language] = &[Language { code: "en", name: "English", lng: "" }];

/// The operating system's preferred user-interface languages, most preferred first, as it names
/// them (`de-DE`, `pt_BR.UTF-8`): Windows' preferred UI languages; elsewhere gettext's order,
/// `LANGUAGE` (unless the locale is `C`), then the first of `LC_ALL`, `LC_MESSAGES` and `LANG`.
pub fn system_languages() -> Vec<String> {
    #[cfg(windows)]
    {
        windows_languages()
    }
    #[cfg(not(windows))]
    {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().find_map(var);
        let mut out = Vec::new();
        if locale.as_deref().is_some_and(|l| l != "C" && l != "POSIX") {
            if let Some(list) = var("LANGUAGE") {
                out.extend(list.split(':').filter(|s| !s.is_empty()).map(str::to_owned));
            }
        }
        out.extend(locale);
        out
    }
}

#[cfg(windows)]
fn windows_languages() -> Vec<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let (mut count, mut len) = (0u32, 0u32);
    // SAFETY: a null buffer asks for its length; the second call gets a buffer of that length.
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, std::ptr::null_mut(), &mut len) == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut len) == 0 {
            return Vec::new();
        }
        // NUL-separated, ending with two NULs
        buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    }
}

/// The shipped language for the first of `preferred` (OS locale names: `de_AT.UTF-8`, `zh-TW`,
/// `pt`) that has one, or English.
pub fn best_match(preferred: &[String]) -> &'static str {
    let codes: Vec<&'static str> = LANGUAGES.iter().map(|l| l.code).collect();
    best_match_in(preferred, &codes).unwrap_or("en")
}

/// [`best_match`] among `available` codes; `None` when none fits.
fn best_match_in<'a>(preferred: &[String], available: &[&'a str]) -> Option<&'a str> {
    preferred.iter().find_map(|p| {
        let tag = canonical(p)?;
        let primary = tag.split('-').next().unwrap_or_default();
        available
            .iter()
            .find(|a| a.eq_ignore_ascii_case(&tag))
            .or_else(|| available.iter().find(|a| a.eq_ignore_ascii_case(primary)))
            .copied()
    })
}

/// An OS locale name as the tag PaTorrent's tables use: `de_AT.UTF-8@euro` is `de-AT`, Chinese
/// by script (`zh-Hans`, `zh-Hant`), Portuguese by country (`pt-BR` unless `pt-PT`), Norwegian
/// as Bokmål (`nb`), the old `iw` / `in` as `he` / `id`. `None` for `C` and `POSIX`.
fn canonical(name: &str) -> Option<String> {
    let name = name.split(['.', '@']).next().unwrap_or_default().trim().replace('_', "-");
    if name.is_empty() || name.eq_ignore_ascii_case("C") || name.eq_ignore_ascii_case("POSIX") {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let mut parts = lower.split('-');
    let primary = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    let has = |s: &str| rest.contains(&s);
    Some(match primary {
        "zh" if has("hant") || has("tw") || has("hk") || has("mo") => "zh-Hant".to_owned(),
        "zh" => "zh-Hans".to_owned(),
        "pt" if has("pt") => "pt-PT".to_owned(),
        "pt" => "pt-BR".to_owned(),
        "no" | "nn" | "nb" => "nb".to_owned(),
        "iw" => "he".to_owned(),
        "in" => "id".to_owned(),
        _ => name,
    })
}

/// What `ui.language` names: its code, or for an empty choice the system's language.
pub fn resolve(choice: &str, system: &[String]) -> String {
    match choice.trim() {
        "" => best_match(system).to_owned(),
        c => c.to_owned(),
    }
}

/// The Preferences list as `(ui.language value, text)`: "System default (<its language>)",
/// each shipped language in its own name, and `current` when it is none of them (a value
/// written by hand, such as `pseudo`), so it survives the dialog.
pub fn choices(current: &str, system: &[String]) -> Vec<(String, String)> {
    let system_name = LANGUAGES.iter().find(|l| l.code == best_match(system)).map_or("English", |l| l.name);
    let mut out = vec![(String::new(), crate::tr!(LangSystemDefault, language = system_name))];
    out.extend(LANGUAGES.iter().map(|l| (l.code.to_owned(), l.name.to_owned())));
    let current = current.trim();
    if !current.is_empty() && !out.iter().any(|(c, _)| c == current) {
        out.push((current.to_owned(), current.to_owned()));
    }
    out
}

/// The current language's text for `s`.
pub fn text(s: S) -> &'static str {
    let t = CURRENT.load(Ordering::Acquire);
    if !t.is_null() {
        // Only `set` stores here, and only pointers from `Box::leak`, which are never freed.
        let t: &'static Table = unsafe { &*t };
        if let Some(x) = t.texts[s as usize].as_deref() {
            return x;
        }
    }
    en::ENGLISH[s as usize]
}

/// `text` with each `{name}` replaced by its value; unknown placeholders stay as they are.
pub fn fill(text: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}').map(|close| (close, &after[..close])) {
            Some((close, name)) if is_name(name) => {
                match args.iter().find(|(n, _)| *n == name) {
                    Some((_, v)) => {
                        use std::fmt::Write;
                        let _ = write!(out, "{v}");
                    }
                    None => {
                        debug_assert!(false, "no value for {{{name}}} in {text:?}");
                        out.push_str(&rest[open..open + close + 2]);
                    }
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The `{name}` placeholders in `text`, in order.
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_name(&after[..close]) => {
                out.push(&after[..close]);
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    out
}

/// `text` without its `&` mnemonics (`&&` stays one `&`): menus and buttons in the Web UI.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '&' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `text` with GTK's `_` mnemonics in place of `&` (and a literal `_` doubled).
pub fn gtk_mnemonic(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '&' => match chars.next() {
                Some('&') => out.push('&'),
                Some(n) => {
                    out.push('_');
                    if n == '_' {
                        out.push('_');
                    }
                    out.push(n);
                }
                None => {}
            },
            '_' => out.push_str("__"),
            c => out.push(c),
        }
    }
    out
}

/// Every key with its current text, in table order (the Web UI's `lang.js`).
pub fn entries() -> impl Iterator<Item = (&'static str, &'static str)> {
    S::ALL.iter().map(|&s| (en::KEYS[s as usize], text(s)))
}

impl S {
    /// The key's name as it appears in `.lng` files and the Web UI (`MenuFile`).
    pub fn key(self) -> &'static str {
        en::KEYS[self as usize]
    }

    pub fn from_key(key: &str) -> Option<S> {
        en::KEYS.iter().position(|k| *k == key).map(|i| S::ALL[i])
    }

    /// The English text.
    pub fn english(self) -> &'static str {
        en::ENGLISH[self as usize]
    }
}

/// One language's texts by key; `None` falls back to English.
pub struct Table {
    code: String,
    texts: Box<[Option<Box<str>>]>,
}

impl Table {
    /// Reads a `.lng` file's text. Lines naming unknown keys, and translations whose
    /// placeholders differ from the English text's, are left out and reported.
    pub fn parse(code: &str, lng: &str) -> (Table, Vec<String>) {
        let mut texts: Vec<Option<Box<str>>> = vec![None; S::ALL.len()];
        let mut problems = Vec::new();
        for (n, line) in lng.lines().enumerate() {
            let line = line.strip_prefix('\u{feff}').unwrap_or(line);
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                problems.push(format!("line {}: no '='", n + 1));
                continue;
            };
            let Some(s) = S::from_key(key.trim()) else {
                problems.push(format!("line {}: unknown key {}", n + 1, key.trim()));
                continue;
            };
            let value = unescape(value);
            let (mut want, mut got) = (placeholders(s.english()), placeholders(&value));
            want.sort_unstable();
            got.sort_unstable();
            if want != got {
                problems.push(format!(
                    "line {}: {} has placeholders {got:?}, English has {want:?}",
                    n + 1,
                    s.key()
                ));
                continue;
            }
            texts[s as usize] = Some(value.into_boxed_str());
        }
        (Table { code: code.to_owned(), texts: texts.into_boxed_slice() }, problems)
    }

    /// Pseudo-localised English: accented letters, wrapped in `[` `]` and a third longer, so a
    /// screen shows at a glance which strings bypass the tables and where long translations
    /// get cut.
    pub fn pseudo() -> Table {
        let texts = S::ALL.iter().map(|s| Some(pseudo_text(s.english()).into_boxed_str())).collect();
        Table { code: "pseudo".to_owned(), texts }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn get(&self, s: S) -> &str {
        self.texts[s as usize].as_deref().unwrap_or(en::ENGLISH[s as usize])
    }

    /// How many keys have their own text.
    pub fn translated(&self) -> usize {
        self.texts.iter().filter(|t| t.is_some()).count()
    }
}

static CURRENT: AtomicPtr<Table> = AtomicPtr::new(std::ptr::null_mut());
/// Bumped by every [`set`] and [`reset`], so a cache of texts knows to rebuild.
static GENERATION: AtomicUsize = AtomicUsize::new(0);

/// Makes `table` the current language. Meant for start-up: windows already built keep their
/// texts.
pub fn set(table: Table) {
    CURRENT.store(Box::into_raw(Box::new(table)), Ordering::Release);
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Changes with every language switch (nox's `lang.js` is rebuilt when it does).
pub fn generation() -> usize {
    GENERATION.load(Ordering::Acquire)
}

/// The current language's code (`en` until [`init`] picks another).
pub fn current() -> &'static str {
    let t = CURRENT.load(Ordering::Acquire);
    if t.is_null() { "en" } else { unsafe { &*t }.code.as_str() }
}

/// Back to English.
fn reset() {
    CURRENT.store(std::ptr::null_mut(), Ordering::Release);
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Picks the language from `choice`, the frontend's `ui.language` preference, unless the
/// `PATORRENT_LANG` environment variable overrides it: empty is the system's language
/// ([`system_languages`], [`best_match`]), a code one of [`LANGUAGES`], `pseudo`
/// pseudo-localised English, and a path to a `.lng` file loads that file (for translators).
/// Returns what went wrong, if anything; the language is then English, or the table without
/// its bad lines. Called at start; nox calls it again when the Web UI's choice changes.
pub fn init(choice: &str) -> Vec<String> {
    let env = std::env::var("PATORRENT_LANG").ok();
    let choice = resolve(env.as_deref().unwrap_or(choice), &system_languages());
    match choice.as_str() {
        "pseudo" => {
            set(Table::pseudo());
            Vec::new()
        }
        path if path.ends_with(".lng") => match std::fs::read_to_string(path) {
            Ok(text) => {
                let code =
                    std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("custom");
                let (table, problems) = Table::parse(code, &text);
                set(table);
                problems.into_iter().map(|p| format!("{path}: {p}")).collect()
            }
            Err(e) => {
                reset();
                vec![format!("{path}: {e}")]
            }
        },
        code => match LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code)) {
            Some(l) if l.lng.is_empty() => {
                reset();
                Vec::new()
            }
            Some(l) => {
                let (table, problems) = Table::parse(l.code, l.lng);
                set(table);
                problems.into_iter().map(|p| format!("{}.lng: {p}", l.code)).collect()
            }
            None => {
                reset();
                vec![format!("no language \"{code}\"; using English")]
            }
        },
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t")
}

/// The English table as a `.lng` file, with each key's translator notes as comments.
pub fn english_lng() -> String {
    let mut out = String::from(
        "# PaTorrent string table: English, the source of every translation.\n\
         # Written from crates/frontend/src/lang/en.rs; do not edit by hand.\n\
         # Key=Text. Keep {placeholders} as they are; & marks the next letter as the keyboard\n\
         # mnemonic (&& is a literal &); \\n is a line break.\n",
    );
    for &s in S::ALL {
        let note = en::NOTES[s as usize];
        if !note.is_empty() {
            out.push('\n');
            for l in note.lines() {
                out.push('#');
                out.push_str(l);
                out.push('\n');
            }
        }
        out.push_str(s.key());
        out.push('=');
        out.push_str(&escape(s.english()));
        out.push('\n');
    }
    out
}

fn pseudo_text(en: &str) -> String {
    const FROM: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const TO: &str = "àƀçđéƒĝĥíĵķĺɱñóþǫŕšţúṽŵẋýžÀßÇĐÉƑĜĤÍĴĶĹḾÑÓÞǪŔŠŢÚṼŴẊÝŽ";
    let map: Vec<(char, char)> = FROM.chars().zip(TO.chars()).collect();
    let mut out = String::from("[");
    let mut in_placeholder = false;
    let mut letters = 0usize;
    for c in en.chars() {
        match c {
            '{' => in_placeholder = true,
            '}' => in_placeholder = false,
            _ => {}
        }
        if in_placeholder || c == '}' {
            out.push(c);
            continue;
        }
        if c.is_ascii_alphabetic() {
            letters += 1;
        }
        out.push(map.iter().find(|(f, _)| *f == c).map_or(c, |(_, t)| *t));
    }
    // Longer by about a third, as German and French often are.
    for _ in 0..letters.div_ceil(3) {
        out.push('·');
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_texts_set() {
        let mut keys: Vec<&str> = en::KEYS.to_vec();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate key");
        assert_eq!(en::KEYS.len(), S::ALL.len());
        assert_eq!(en::ENGLISH.len(), S::ALL.len());
        for &s in S::ALL {
            assert!(!s.english().is_empty(), "{} is empty", s.key());
            assert_eq!(s.english().trim(), s.english(), "{} has spaces around it", s.key());
            assert_eq!(S::from_key(s.key()), Some(s));
            // Placeholders are lower-case names: `{Name}` or `{0}`-style would not be filled.
            let t = s.english();
            assert_eq!(t.matches('{').count(), placeholders(t).len(), "{}: odd braces", s.key());
        }
    }

    #[test]
    fn the_english_lng_file_matches_the_table() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lang").join("en.lng");
        let want = english_lng();
        if std::env::var_os("PATORRENT_WRITE_LNG").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert!(have == want, "{} is stale: run the tests with PATORRENT_WRITE_LNG=1", path.display());
        // And it reads back as the English table.
        let (t, problems) = Table::parse("en", &want);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(S::ALL.iter().all(|&s| t.get(s) == s.english()));
    }

    #[test]
    fn fills_placeholders() {
        assert_eq!(fill("{a} of {b}", &[("a", &1), ("b", &"two")]), "1 of two");
        assert_eq!(fill("{a}{a}", &[("a", &'x')]), "xx");
        assert_eq!(fill("no { braces } {x y}", &[]), "no { braces } {x y}");
        assert_eq!(placeholders("Remove {name}? {count} {x y} {}"), ["name", "count"]);
        let name = "ubuntu.iso";
        assert_eq!(fill(S::MenuFile.english(), &[]), S::MenuFile.english());
        assert_eq!(crate::tr!(LogAdded, name), format!("{name}: added"));
        assert_eq!(crate::tr!(LogAdded, name = "x"), "x: added");
    }

    #[test]
    fn os_locale_names_find_the_shipped_language() {
        let available = ["en", "de", "pt-BR", "pt-PT", "zh-Hans", "zh-Hant", "nb", "he"];
        let m = |names: &[&str]| {
            let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
            best_match_in(&names, &available)
        };
        assert_eq!(m(&["de_AT.UTF-8@euro"]), Some("de"));
        assert_eq!(m(&["de-DE", "en-US"]), Some("de"));
        // the first preference with a table, English included
        assert_eq!(m(&["fr-FR", "de"]), Some("de"));
        assert_eq!(m(&["en-US", "de-DE"]), Some("en"));
        assert_eq!(m(&["pt_BR.UTF-8"]), Some("pt-BR"));
        assert_eq!(m(&["pt-PT"]), Some("pt-PT"));
        assert_eq!(m(&["pt"]), Some("pt-BR"));
        assert_eq!(m(&["zh_TW"]), Some("zh-Hant"));
        assert_eq!(m(&["zh-Hant-HK"]), Some("zh-Hant"));
        assert_eq!(m(&["zh-Hans-CN"]), Some("zh-Hans"));
        assert_eq!(m(&["zh_CN.GB2312"]), Some("zh-Hans"));
        assert_eq!(m(&["no_NO", "nn"]), Some("nb"));
        assert_eq!(m(&["iw_IL"]), Some("he"));
        assert_eq!(m(&["C", "POSIX", ""]), None);
        assert_eq!(m(&["fr"]), None);
        assert_eq!(best_match(&["fr_FR".to_owned()]), "en");
        assert_eq!(best_match(&[]), "en");
    }

    /// Windows always names at least one user-interface language.
    #[cfg(windows)]
    #[test]
    fn windows_names_its_languages() {
        let names = system_languages();
        assert!(!names.is_empty() && names.iter().all(|n| canonical(n).is_some()), "{names:?}");
        println!("system languages: {names:?}");
    }

    #[test]
    fn the_preferences_list() {
        let system = vec!["de-DE".to_owned()];
        let c = choices("", &system);
        assert_eq!(c[0], (String::new(), "System default (English)".to_owned()));
        assert_eq!(c[1], ("en".to_owned(), "English".to_owned()));
        assert_eq!(c.len(), LANGUAGES.len() + 1);
        // a value written by hand stays a choice, so the dialog does not drop it
        assert_eq!(choices("pseudo", &system).last().unwrap(), &("pseudo".to_owned(), "pseudo".to_owned()));
        assert_eq!(choices("en", &system).len(), c.len());
        assert_eq!(resolve("", &system), "en");
        assert_eq!(resolve(" pseudo ", &system), "pseudo");
        assert!(LANGUAGES[0].code == "en" && LANGUAGES[0].lng.is_empty());
        for l in LANGUAGES.iter().skip(1) {
            let (_, problems) = Table::parse(l.code, l.lng);
            assert!(problems.is_empty(), "{}: {problems:?}", l.code);
        }
    }

    #[test]
    fn mnemonics() {
        assert_eq!(plain("&File"), "File");
        assert_eq!(plain("Save && E&xit"), "Save & Exit");
        assert_eq!(gtk_mnemonic("&File"), "_File");
        assert_eq!(gtk_mnemonic("Save && E&xit"), "Save & E_xit");
        assert_eq!(gtk_mnemonic("a_b &_c"), "a__b ___c");
    }

    #[test]
    fn parses_lng_files() {
        let lng = "\u{feff}# comment\n\nMenuFile=&Fichier\nLogAdded={name} ajouté\nNope=x\nbroken\n\
                   CatTorrents=Line\\nbreak \\\\ back\nLogFinished=sans nom\n";
        let (t, problems) = Table::parse("fr", lng);
        assert_eq!(t.code(), "fr");
        assert_eq!(t.get(S::MenuFile), "&Fichier");
        assert_eq!(t.get(S::LogAdded), "{name} ajouté");
        assert_eq!(t.get(S::CatTorrents), "Line\nbreak \\ back");
        // The placeholder check drops it, so English shows.
        assert_eq!(t.get(S::LogFinished), S::LogFinished.english());
        assert_eq!(t.translated(), 3);
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    #[test]
    fn pseudo_keeps_placeholders_and_mnemonics() {
        let p = pseudo_text("&Remove {name} now");
        assert!(p.starts_with("[&Ŕéɱóṽé {name} ñóŵ"), "{p}");
        assert!(p.ends_with("·]"));
        let t = Table::pseudo();
        assert_eq!(t.translated(), S::ALL.len());
        for &s in S::ALL {
            assert_eq!(placeholders(t.get(s)), placeholders(s.english()), "{}", s.key());
        }
    }
}
