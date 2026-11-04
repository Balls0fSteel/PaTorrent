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
//! * The translations, `crates/frontend/lang/<code>.lng`, are compiled into every binary,
//!   deflated by `build.rs` (D26) as their texts alone in the English table's key order;
//!   [`init`] inflates the chosen one only.
//! * The language is process-wide, picked once at start by [`init`]; tables are leaked, so
//!   texts are `&'static str`.
//! * Help texts (D55) are a second table, [`H`], keyed like the strings: `help!(PrefMaxConnections)`
//!   is the help for the control labelled `S::PrefMaxConnections`. Their English is in
//!   `help_en.rs` and goes into `en.lng` as `help.<Key>=` lines after the strings; `build.rs`
//!   deflates it from there and [`help`] inflates it on first use. A translation's help lines
//!   are optional (none has them yet): a missing one shows the English text.

use std::fmt::Display;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

use patorrent_core::util::puff;

mod en;
mod help_en;

pub use en::S;
pub use help_en::H;

/// `help!(Key)` is the current language's help text for [`H::Key`](H) (`&'static str`), English
/// where the language has none.
#[macro_export]
macro_rules! help {
    ($key:ident) => {
        $crate::lang::help($crate::lang::H::$key)
    };
}

/// The prefix of a help text's key in `.lng` files: `help.PrefMaxConnections=...`.
pub const HELP_PREFIX: &str = "help.";

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
    /// A BCP 47 tag: `de`, `pt-BR`, `zh-Hans`; what `ui.language` stores, and the table's file
    /// name (`lang/<code>.lng`).
    pub code: &'static str,
    /// The language's own name for itself, as the Preferences list shows it (not translated).
    pub name: &'static str,
    /// Written right to left (Arabic, Hebrew, Persian): the GUIs mirror their layouts.
    pub rtl: bool,
    /// Its texts as `build.rs` deflated them: one `.lng`-escaped line per key in [`S::ALL`]
    /// order, then one per help key in [`H::ALL`] order, empty where the table has none. Empty
    /// for English, whose texts are [`en`] and whose help is [`ENGLISH_HELP`].
    deflated: &'static [u8],
}

/// The most a table may inflate to; the biggest is under a tenth of it.
const MAX_TABLE: usize = 1 << 20;

impl Language {
    /// Its table, inflated, with what was wrong with it (texts whose placeholders differ from
    /// the English ones are left out); `None` for English.
    pub fn table(&self) -> Result<Option<(Table, Vec<String>)>, String> {
        if self.deflated.is_empty() {
            return Ok(None);
        }
        let (bytes, _) = puff::inflate(self.deflated, MAX_TABLE).map_err(|e| e.to_string())?;
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        let lines: Vec<&str> = text.split('\n').collect();
        if lines.len() != S::ALL.len() + H::ALL.len() {
            return Err(format!("{} texts for {} keys", lines.len(), S::ALL.len() + H::ALL.len()));
        }
        let (strings, helps) = lines.split_at(S::ALL.len());
        let mut problems = Vec::new();
        let texts = S::ALL
            .iter()
            .zip(strings)
            .map(|(&s, line)| match *line {
                "" => None,
                l => checked(s, unescape(l)).map_err(|e| problems.push(e)).ok(),
            })
            .collect();
        let help = helps.iter().map(|l| (!l.is_empty()).then(|| unescape(l).into_boxed_str())).collect();
        Ok(Some((Table { code: self.code.to_owned(), texts, help }, problems)))
    }

    /// How many bytes its table adds to a binary.
    pub fn deflated_len(&self) -> usize {
        self.deflated.len()
    }
}

/// A translated language, its table included from `build.rs`'s output.
macro_rules! translated {
    ($code:literal, $name:literal) => {
        translated!(@ $code, $name, false)
    };
    ($code:literal, $name:literal, rtl) => {
        translated!(@ $code, $name, true)
    };
    (@ $code:literal, $name:literal, $rtl:literal) => {
        Language {
            code: $code,
            name: $name,
            rtl: $rtl,
            deflated: include_bytes!(concat!(env!("OUT_DIR"), "/", $code, ".lng.z")),
        }
    };
}

/// The languages PaTorrent ships (D24), English first, then by code. Every `lang/<code>.lng`
/// but English's is listed here, and every listed table is complete (tests).
pub const LANGUAGES: &[Language] = &[
    Language { code: "en", name: "English", rtl: false, deflated: &[] },
    translated!("ar", "العربية", rtl),
    translated!("bg", "Български"),
    translated!("ca", "Català"),
    translated!("cs", "Čeština"),
    translated!("da", "Dansk"),
    translated!("de", "Deutsch"),
    translated!("el", "Ελληνικά"),
    translated!("es", "Español"),
    translated!("fa", "فارسی", rtl),
    translated!("fi", "Suomi"),
    translated!("fr", "Français"),
    translated!("he", "עברית", rtl),
    translated!("hr", "Hrvatski"),
    translated!("hu", "Magyar"),
    translated!("id", "Bahasa Indonesia"),
    translated!("it", "Italiano"),
    translated!("ja", "日本語"),
    translated!("ko", "한국어"),
    translated!("nb", "Norsk bokmål"),
    translated!("nl", "Nederlands"),
    translated!("pl", "Polski"),
    translated!("pt-BR", "Português (Brasil)"),
    translated!("pt-PT", "Português (Portugal)"),
    translated!("ro", "Română"),
    translated!("ru", "Русский"),
    translated!("sk", "Slovenčina"),
    translated!("sr", "Српски"),
    translated!("sv", "Svenska"),
    translated!("th", "ไทย"),
    translated!("tr", "Türkçe"),
    translated!("uk", "Українська"),
    translated!("vi", "Tiếng Việt"),
    translated!("zh-Hans", "简体中文"),
    translated!("zh-Hant", "繁體中文"),
];

/// The operating system's preferred user-interface languages, most preferred first, as it names
/// them (`de-DE`, `pt_BR.UTF-8`): Windows' preferred UI languages; elsewhere gettext's order,
/// `LANGUAGE` (unless the locale is `C`), then the first of `LC_ALL`, `LC_MESSAGES` and `LANG`.
/// On macOS, where an application started from the Finder has no `LANG`, the system's list
/// (System Settings > General > Language & Region, `AppleLanguages`) comes before `LANG`, which
/// Terminal sets from the region rather than the language; `LANGUAGE`, `LC_ALL` and
/// `LC_MESSAGES` still win, being set on purpose.
pub fn system_languages() -> Vec<String> {
    #[cfg(windows)]
    {
        windows_languages()
    }
    #[cfg(not(windows))]
    {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        #[cfg(target_os = "macos")]
        let system = macos::preferred_languages();
        #[cfg(not(target_os = "macos"))]
        let system = Vec::new();
        posix_languages(var, system)
    }
}

/// [`system_languages`] off Windows, from the environment `var` and the system's own list
/// (`system`, empty but on macOS), which goes after the variables set on purpose and before `LANG`.
#[cfg_attr(windows, allow(dead_code))]
fn posix_languages(var: impl Fn(&str) -> Option<String>, system: Vec<String>) -> Vec<String> {
    let explicit = ["LC_ALL", "LC_MESSAGES"].into_iter().find_map(&var);
    let locale = explicit.clone().or_else(|| var("LANG"));
    let mut out = Vec::new();
    // gettext ignores LANGUAGE under the C locale; with no locale at all only the system's
    // list counts (macOS)
    if locale.as_deref().is_some_and(|l| l != "C" && l != "POSIX") {
        if let Some(list) = var("LANGUAGE") {
            out.extend(list.split(':').filter(|s| !s.is_empty()).map(str::to_owned));
        }
    }
    if system.is_empty() {
        out.extend(locale);
    } else {
        out.extend(explicit);
        out.extend(system);
        out.extend(var("LANG"));
    }
    out
}

/// The system's preferred languages through Core Foundation.
#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{c_char, c_void};

    type CFIndex = isize;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> *const c_void;
        fn CFArrayGetCount(array: *const c_void) -> CFIndex;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: CFIndex) -> *const c_void;
        fn CFStringGetCString(s: *const c_void, buf: *mut c_char, size: CFIndex, encoding: u32) -> u8;
        fn CFRelease(cf: *const c_void);
    }
    const UTF8: u32 = 0x0800_0100;

    /// BCP 47 tags (`en-US`, `zh-Hans-CN`), most preferred first.
    pub fn preferred_languages() -> Vec<String> {
        // SAFETY: the array follows the Copy rule and is released here; its values follow the
        // Get rule and are only read while it lives; the buffer's length is passed.
        unsafe {
            let array = CFLocaleCopyPreferredLanguages();
            if array.is_null() {
                return Vec::new();
            }
            let mut out = Vec::new();
            for i in 0..CFArrayGetCount(array) {
                let s = CFArrayGetValueAtIndex(array, i);
                let mut buf = [0 as c_char; 128];
                if !s.is_null() && CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as CFIndex, UTF8) != 0 {
                    let tag = std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
                    if !tag.is_empty() {
                        out.push(tag);
                    }
                }
            }
            CFRelease(array);
            out
        }
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

/// `text` without its `&` mnemonics (`&&` stays one `&`), and without the `(&F)` that Chinese,
/// Japanese, Korean and Thai menus put after a text with no Latin letters: menus and buttons in
/// the Web UI.
pub fn plain(text: &str) -> String {
    let b = text.as_bytes();
    if let Some(i) = text.find("(&") {
        if b.len() >= i + 4 && b[i + 2].is_ascii_alphanumeric() && b[i + 3] == b')' {
            return plain(&format!("{}{}", text[..i].trim_end(), &text[i + 4..]));
        }
    }
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

/// `text` as `s`'s translation, unless its placeholders differ from the English text's.
fn checked(s: S, text: String) -> Result<Box<str>, String> {
    let (mut want, mut got) = (placeholders(s.english()), placeholders(&text));
    want.sort_unstable();
    got.sort_unstable();
    if want != got {
        return Err(format!("{} has placeholders {got:?}, English has {want:?}", s.key()));
    }
    Ok(text.into_boxed_str())
}

/// One language's texts and help texts by key; `None` falls back to English.
pub struct Table {
    code: String,
    texts: Box<[Option<Box<str>>]>,
    help: Box<[Option<Box<str>>]>,
}

impl Table {
    /// Reads a `.lng` file's text. Lines naming unknown keys, and translations whose
    /// placeholders differ from the English text's, are left out and reported.
    pub fn parse(code: &str, lng: &str) -> (Table, Vec<String>) {
        let mut texts: Vec<Option<Box<str>>> = vec![None; S::ALL.len()];
        let mut help: Vec<Option<Box<str>>> = vec![None; H::ALL.len()];
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
            if let Some(name) = key.trim().strip_prefix(HELP_PREFIX) {
                match H::from_key(name) {
                    Some(h) => help[h as usize] = Some(unescape(value).into_boxed_str()),
                    None => problems.push(format!("line {}: unknown key {}", n + 1, key.trim())),
                }
                continue;
            }
            let Some(s) = S::from_key(key.trim()) else {
                problems.push(format!("line {}: unknown key {}", n + 1, key.trim()));
                continue;
            };
            match checked(s, unescape(value)) {
                Ok(text) => texts[s as usize] = Some(text),
                Err(e) => problems.push(format!("line {}: {e}", n + 1)),
            }
        }
        let table =
            Table { code: code.to_owned(), texts: texts.into_boxed_slice(), help: help.into_boxed_slice() };
        (table, problems)
    }

    /// Pseudo-localised English: accented letters, wrapped in `[` `]` and a third longer, so a
    /// screen shows at a glance which strings bypass the tables and where long translations
    /// get cut.
    pub fn pseudo() -> Table {
        let texts = S::ALL.iter().map(|s| Some(pseudo_text(s.english()).into_boxed_str())).collect();
        let help = H::ALL.iter().map(|h| Some(pseudo_text(h.english()).into_boxed_str())).collect();
        Table { code: "pseudo".to_owned(), texts, help }
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

    /// The help text for `h`, English where the table has none.
    pub fn get_help(&self, h: H) -> &str {
        self.help[h as usize].as_deref().unwrap_or(h.english())
    }

    /// How many help keys have their own text.
    pub fn help_translated(&self) -> usize {
        self.help.iter().filter(|t| t.is_some()).count()
    }
}

/// The English help texts, deflated by `build.rs` from `en.lng`'s `help.` lines: one
/// `.lng`-escaped line per key in [`H::ALL`] order.
pub const ENGLISH_HELP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/help-en.z"));

/// [`ENGLISH_HELP`] inflated, at the first help text asked for.
fn english_help() -> &'static [Box<str>] {
    static TEXTS: std::sync::OnceLock<Box<[Box<str>]>> = std::sync::OnceLock::new();
    TEXTS.get_or_init(|| {
        let text = puff::inflate(ENGLISH_HELP, MAX_TABLE)
            .ok()
            .and_then(|(bytes, _)| String::from_utf8(bytes).ok())
            .unwrap_or_default();
        let mut lines: Vec<Box<str>> = text.split('\n').map(|l| unescape(l).into_boxed_str()).collect();
        // a build out of step with en.lng shows nothing rather than a wrong text (a test fails)
        if lines.len() != H::ALL.len() {
            lines = vec![Box::from(""); H::ALL.len()];
        }
        lines.into_boxed_slice()
    })
}

/// The current language's help text for `h`, English where it has none.
pub fn help(h: H) -> &'static str {
    let t = CURRENT.load(Ordering::Acquire);
    if !t.is_null() {
        // as in `text`: only leaked tables are ever stored
        let t: &'static Table = unsafe { &*t };
        if let Some(x) = t.help[h as usize].as_deref() {
            return x;
        }
    }
    h.english()
}

impl H {
    /// The key's name, as `.lng` files write it after [`HELP_PREFIX`].
    pub fn key(self) -> &'static str {
        help_en::HELP_KEYS[self as usize]
    }

    pub fn from_key(key: &str) -> Option<H> {
        help_en::HELP_KEYS.iter().position(|k| *k == key).map(|i| H::ALL[i])
    }

    /// The help text for the control labelled `s`: the help key of the same name.
    pub fn for_label(s: S) -> Option<H> {
        H::from_key(s.key())
    }

    /// The English text.
    pub fn english(self) -> &'static str {
        &english_help()[self as usize]
    }

    /// The key as a web page anchor: `PrefMaxConnections` is `pref-max-connections`.
    pub fn anchor(self) -> String {
        let mut out = String::new();
        for c in self.key().chars() {
            if c.is_ascii_uppercase() && !out.is_empty() {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        }
        out
    }
}

static CURRENT: AtomicPtr<Table> = AtomicPtr::new(std::ptr::null_mut());
/// [`right_to_left`] for [`CURRENT`], kept beside it for the paint code that asks often.
static RTL: AtomicBool = AtomicBool::new(false);
/// Bumped by every [`set`] and [`reset`], so a cache of texts knows to rebuild.
static GENERATION: AtomicUsize = AtomicUsize::new(0);

/// Makes `table` the current language. Meant for start-up: windows already built keep their
/// texts.
pub fn set(table: Table) {
    RTL.store(is_rtl(&table.code), Ordering::Release);
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

/// Whether the current language is written right to left. A translator's `.lng` file counts
/// by its name (`ar.lng`).
pub fn right_to_left() -> bool {
    RTL.load(Ordering::Acquire)
}

fn is_rtl(code: &str) -> bool {
    LANGUAGES.iter().any(|l| l.rtl && l.code.eq_ignore_ascii_case(code))
}

/// Back to English.
fn reset() {
    RTL.store(false, Ordering::Release);
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
            Some(l) if l.deflated.is_empty() => {
                reset();
                Vec::new()
            }
            Some(l) => match l.table() {
                Ok(Some((table, problems))) => {
                    set(table);
                    problems.into_iter().map(|p| format!("{}.lng: {p}", l.code)).collect()
                }
                Ok(None) => {
                    reset();
                    Vec::new()
                }
                Err(e) => {
                    reset();
                    vec![format!("{}.lng: {e}", l.code)]
                }
            },
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

#[cfg(test)]
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t")
}

/// The English table as a `.lng` file, with each key's translator notes as comments, then the
/// help texts as `help.<Key>=` lines.
#[cfg(test)]
fn english_lng() -> String {
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
    out.push_str(
        "\n# ---- Help texts (D55): tooltips, the Preferences description pane and F1, one to three\n\
         # sentences each. Optional in a translation: a missing one shows in English.\n",
    );
    for (key, text) in help_en::HELP_KEYS.iter().zip(help_en::HELP_ENGLISH) {
        out.push_str(HELP_PREFIX);
        out.push_str(key);
        out.push('=');
        out.push_str(&escape(text));
        out.push('\n');
    }
    out
}

/// `docs/HELP_TEXTS.md`: every help text with where its claim was read in the code, for a
/// review against the code (D55).
#[cfg(test)]
pub(crate) fn help_texts_md() -> String {
    let cell = |s: &str| s.replace('|', "\\|");
    let menu = |k: &str| k.starts_with("Menu") || k.starts_with("Tool") || k.starts_with("Shutdown");
    let mut out = String::from(
        "# Help texts\n\n\
         Generated from `crates/frontend/src/lang/help_en.rs` by a test \
         (`PATORRENT_WRITE_LNG=1 cargo test -p patorrent-frontend` rewrites it); do not edit by hand. \
         Each text is shown as a tooltip, in the Preferences description pane and on F1 (D55). \
         \"Read in\" names the code the text was written from, for a review against it.\n\n\
         ## Preferences controls\n\n\
         The key is the control's label key (`S::PrefDht` has `help.PrefDht`).\n\n\
         | Key | Text | Read in |\n|---|---|---|\n",
    );
    let row = |out: &mut String, h: H| {
        let i = h as usize;
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            h.key(),
            cell(help_en::HELP_ENGLISH[i]),
            cell(help_en::HELP_NOTES[i].trim())
        ));
    };
    for &h in H::ALL.iter().filter(|h| !h.key().starts_with("Adv") && !menu(h.key())) {
        row(&mut out, h);
    }
    out.push_str(
        "\n## The Advanced table\n\n\
         Every engine setting, in the table's order. A row a Preferences page edits under the same \
         name shows that page's text (its key starts with `Pref`).\n\n\
         | Setting | Key | Text | Read in |\n|---|---|---|---|\n",
    );
    for e in crate::settings::TABLE {
        let Some(h) = crate::help::for_setting(e.name) else {
            out.push_str(&format!("| `{}` | (none) | | |\n", e.name));
            continue;
        };
        let i = h as usize;
        let note = if h.key().starts_with("Adv") {
            cell(help_en::HELP_NOTES[i].trim())
        } else {
            "(the page's)".into()
        };
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} |\n",
            e.name,
            h.key(),
            cell(help_en::HELP_ENGLISH[i]),
            note
        ));
    }
    out.push_str("\n## Menus and toolbar\n\n| Key | Text | Read in |\n|---|---|---|\n");
    for &h in H::ALL.iter().filter(|h| menu(h.key())) {
        row(&mut out, h);
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
        assert!(H::ALL.iter().all(|&h| t.get_help(h) == help_en::HELP_ENGLISH[h as usize]));
        assert_eq!(t.help_translated(), H::ALL.len());
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
        assert_eq!(best_match(&["fr_FR".to_owned()]), "fr");
        assert_eq!(best_match(&["eo".to_owned(), "zh_TW.UTF-8".to_owned()]), "zh-Hant");
        assert_eq!(best_match(&["eo_XX".to_owned()]), "en");
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
    fn posix_and_macos_language_order() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned())
        };
        let list = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        // Linux: LANGUAGE, then the first locale variable
        let e = env(&[("LANGUAGE", "fr:de"), ("LC_MESSAGES", "it_IT.UTF-8"), ("LANG", "es_ES.UTF-8")]);
        assert_eq!(posix_languages(e, Vec::new()), list(&["fr", "de", "it_IT.UTF-8"]));
        // the C locale silences LANGUAGE
        assert_eq!(posix_languages(env(&[("LANGUAGE", "fr"), ("LANG", "C")]), Vec::new()), list(&["C"]));
        assert!(posix_languages(env(&[("LANGUAGE", "fr")]), Vec::new()).is_empty());

        // macOS from the Finder: no variables, the system's list
        let mac = list(&["zh-Hans-CN", "en-US"]);
        assert_eq!(posix_languages(env(&[]), mac.clone()), mac);
        assert_eq!(best_match(&posix_languages(env(&[]), mac.clone())), "zh-Hans");
        // Terminal's LANG (from the region) comes after the system's list
        assert_eq!(
            posix_languages(env(&[("LANG", "en_GB.UTF-8")]), mac.clone()),
            list(&["zh-Hans-CN", "en-US", "en_GB.UTF-8"])
        );
        // LC_ALL / LC_MESSAGES / LANGUAGE set on purpose win
        assert_eq!(
            posix_languages(env(&[("LC_ALL", "de_DE.UTF-8"), ("LANGUAGE", "fr")]), mac.clone()),
            list(&["fr", "de_DE.UTF-8", "zh-Hans-CN", "en-US"])
        );
    }

    /// macOS always names at least one preferred language.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_names_its_languages() {
        let names = macos::preferred_languages();
        assert!(!names.is_empty() && names.iter().all(|n| canonical(n).is_some()), "{names:?}");
        println!("system languages: {names:?}");
    }

    #[test]
    fn the_preferences_list() {
        let system = vec!["de-DE".to_owned()];
        let c = choices("", &system);
        assert_eq!(c[0], (String::new(), "System default (Deutsch)".to_owned()));
        assert_eq!(c[1], ("en".to_owned(), "English".to_owned()));
        assert!(c.contains(&("de".to_owned(), "Deutsch".to_owned())));
        assert_eq!(c.len(), LANGUAGES.len() + 1);
        // no table for the system's language: English
        assert_eq!(choices("", &["xx-YY".to_owned()])[0].1, "System default (English)");
        assert_eq!(resolve("", &["xx".to_owned()]), "en");
        // a value written by hand stays a choice, so the dialog does not drop it
        assert_eq!(choices("pseudo", &system).last().unwrap(), &("pseudo".to_owned(), "pseudo".to_owned()));
        assert_eq!(choices("en", &system).len(), c.len());
        assert_eq!(resolve("", &system), "de");
        assert_eq!(resolve(" pseudo ", &system), "pseudo");
        assert!(LANGUAGES[0].code == "en" && LANGUAGES[0].table().is_ok_and(|t| t.is_none()));
    }

    /// Every `lang/<code>.lng` but English's is shipped, inflates back to the file's texts, and
    /// gives a text for every key with the English text's placeholders, mnemonic and line breaks.
    #[test]
    fn every_table_is_shipped_and_complete() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lang");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| {
                let p = e.unwrap().path();
                let stem = p.file_stem()?.to_str()?.to_owned();
                (p.extension()? == "lng" && stem != "en").then_some(stem)
            })
            .collect();
        files.sort_unstable();
        let listed: Vec<String> = LANGUAGES[1..].iter().map(|l| l.code.to_owned()).collect();
        assert!(listed.windows(2).all(|w| w[0] < w[1]), "LANGUAGES is not sorted by code");
        assert_eq!(files, listed, "lang/*.lng and LANGUAGES differ");
        let mut total = 0;
        for l in &LANGUAGES[1..] {
            let (shipped, problems) = l.table().unwrap_or_else(|e| panic!("{}: {e}", l.code)).unwrap();
            assert!(problems.is_empty(), "{}: {problems:?}", l.code);
            let file = std::fs::read_to_string(dir.join(format!("{}.lng", l.code))).unwrap();
            let (t, problems) = Table::parse(l.code, &file);
            assert!(problems.is_empty(), "{}: {problems:?}", l.code);
            assert_eq!(shipped.code(), l.code);
            assert!(S::ALL.iter().all(|&s| shipped.get(s) == t.get(s)), "{}: shipped texts differ", l.code);
            let missing: Vec<&str> =
                S::ALL.iter().filter(|&&s| t.texts[s as usize].is_none()).map(|s| s.key()).collect();
            assert!(missing.is_empty(), "{}.lng lacks {missing:?}", l.code);
            for &s in S::ALL {
                let (en, tr) = (s.english(), t.get(s));
                let key = s.key();
                assert_eq!(mnemonic(tr).is_some(), mnemonic(en).is_some(), "{}: {key}: {tr:?}", l.code);
                assert_eq!(tr.trim(), tr, "{}: {key} has spaces around it", l.code);
                assert_eq!(
                    tr.matches('\n').count(),
                    en.matches('\n').count(),
                    "{}: {key}: line breaks",
                    l.code
                );
                // a mnemonic marks a letter or digit; labels keep their colon, menu items
                // that open a dialog their ellipsis
                assert!(mnemonic(tr).is_none_or(char::is_alphanumeric), "{}: {key}: {tr:?}", l.code);
                if en.ends_with(':') {
                    assert!(tr.ends_with(':') || tr.ends_with('\u{ff1a}'), "{}: {key}: {tr:?}", l.code);
                }
                if en.ends_with("...") {
                    assert!(tr.ends_with("...") || tr.ends_with('\u{2026}'), "{}: {key}: {tr:?}", l.code);
                }
            }
            assert_eq!(l.rtl, matches!(l.code, "ar" | "fa" | "he"), "{}", l.code);
            assert!(!l.name.is_empty() && l.name != "English");
            total += l.deflated_len();
        }
        println!("{} translations, {total} bytes deflated", LANGUAGES.len() - 1);
    }

    /// `build.rs` writes the shipped tables in `en.lng`'s key order, which must be [`S::ALL`]'s.
    #[test]
    fn shipped_tables_follow_the_key_order() {
        let keys: Vec<&str> = include_str!(concat!(env!("OUT_DIR"), "/keys.txt")).split('\n').collect();
        let want: Vec<String> = en::KEYS
            .iter()
            .map(|k| k.to_string())
            .chain(help_en::HELP_KEYS.iter().map(|k| format!("{HELP_PREFIX}{k}")))
            .collect();
        assert_eq!(keys, want);
    }

    /// The help table: unique keys, a text for each, and the binaries' deflated English equal
    /// to `help_en.rs`.
    #[test]
    fn help_texts_are_set_and_shipped() {
        let mut keys: Vec<&str> = help_en::HELP_KEYS.to_vec();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate help key");
        assert_eq!(help_en::HELP_ENGLISH.len(), H::ALL.len());
        for &h in H::ALL {
            let text = help_en::HELP_ENGLISH[h as usize];
            assert!(!text.is_empty() && text.trim() == text, "{}", h.key());
            assert!(placeholders(text).is_empty() && !text.contains('&'), "{}: {text}", h.key());
            assert_eq!(h.english(), text, "{}: help-en.z is stale (en.lng)", h.key());
            assert_eq!(H::from_key(h.key()), Some(h));
            assert!(!help_en::HELP_NOTES[h as usize].trim().is_empty(), "{} has no source note", h.key());
        }
        let words: usize = help_en::HELP_ENGLISH.iter().map(|t| t.split_whitespace().count()).sum();
        let bytes: usize = help_en::HELP_ENGLISH.iter().map(|t| t.len()).sum();
        println!(
            "{} help texts, {words} words, {bytes} bytes, {} deflated",
            H::ALL.len(),
            ENGLISH_HELP.len()
        );
    }

    #[test]
    fn help_lookup_and_fallback() {
        assert_eq!(H::for_label(S::PrefMaxConnections), Some(H::PrefMaxConnections));
        assert_eq!(H::for_label(S::PrefMaxConnections).map(H::key), Some(S::PrefMaxConnections.key()));
        assert_eq!(H::PrefMaxConnections.anchor(), "pref-max-connections");
        // no help key for a label that has none
        assert_eq!(H::for_label(S::StatusPaused), None);
        // a translation's help line wins; one it lacks shows in English
        let lng = "MenuFile=&Fichier\nhelp.PrefDht=Le réseau DHT.\nhelp.Nope=x\n";
        let (t, problems) = Table::parse("fr", lng);
        assert_eq!(t.get_help(H::PrefDht), "Le réseau DHT.");
        assert_eq!(t.get_help(H::PrefLsd), H::PrefLsd.english());
        assert_eq!((t.translated(), t.help_translated()), (1, 1));
        assert_eq!(problems.len(), 1, "{problems:?}");
        // the current language (English here) gives the English text
        assert_eq!(crate::help!(PrefDht), H::PrefDht.english());
        assert!(Table::pseudo().get_help(H::PrefDht).starts_with('['));
    }

    /// Help texts are optional in a translation (D55: English compiled in, translations with
    /// the wiki): the shipped tables carry none yet, and every one still inflates and falls back.
    #[test]
    fn translations_need_no_help_texts() {
        for l in &LANGUAGES[1..] {
            let (t, _) = l.table().unwrap().unwrap();
            assert!(H::ALL.iter().all(|&h| !t.get_help(h).is_empty()), "{}", l.code);
            if t.help_translated() == 0 {
                assert_eq!(t.get_help(H::PrefDht), H::PrefDht.english());
            }
        }
    }

    /// The letter after a single `&`.
    fn mnemonic(text: &str) -> Option<char> {
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '&' {
                match chars.next() {
                    Some('&') => {}
                    other => return other,
                }
            }
        }
        None
    }

    #[test]
    fn mnemonics() {
        assert_eq!(plain("&File"), "File");
        assert_eq!(plain("Save && E&xit"), "Save & Exit");
        assert_eq!(gtk_mnemonic("&File"), "_File");
        assert_eq!(gtk_mnemonic("Save && E&xit"), "Save & E_xit");
        assert_eq!(gtk_mnemonic("a_b &_c"), "a__b ___c");
        // the CJK style keeps the letter in brackets after the text
        assert_eq!(plain("文件(&F)"), "文件");
        assert_eq!(plain("添加 Torrent(&A)..."), "添加 Torrent...");
        assert_eq!(plain("ファイルを開く (&O)"), "ファイルを開く");
        assert_eq!(plain("a (&&) b"), "a (&) b");
        assert_eq!(gtk_mnemonic("文件(&F)"), "文件(_F)");
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
