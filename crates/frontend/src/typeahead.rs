//! Type-ahead in lists: typing a name's first letters selects the next row whose name starts
//! with them, as in Explorer. Toolkit-independent; the Add New Torrent dialog's file list uses
//! it through [`crate::add::FileTree::find_row`], and any list can call [`find`] with its own
//! names.
//!
//! A GUI keeps one [`TypeAhead`] per list and, for each printable character typed in it
//! (without Ctrl or Alt), does:
//!
//! ```
//! # use std::time::Instant;
//! # use rookparcel_frontend::typeahead::{self, TypeAhead};
//! # let names = ["alpha", "Gamma", "golf"];
//! # let mut ta = TypeAhead::default();
//! # let (c, current) = ('g', Some(0));
//! let (prefix, from) = ta.push(c, Instant::now(), current);
//! if let Some(row) = typeahead::find(names.len(), from, prefix, |i| names[i]) {
//!     // select and show `row`
//! #   assert_eq!(row, 1);
//! }
//! ```
//!
//! Space is not fed to [`TypeAhead::push`] by callers: in the lists it has its own meaning
//! (toggling a check box, opening a row), so it never becomes part of the prefix.

use std::time::{Duration, Instant};

/// How long the typed letters are kept between two key presses.
pub const TIMEOUT: Duration = Duration::from_secs(1);

/// Whether `name` starts with `prefix`, ignoring case (as [`crate::model`]'s filter matches,
/// D35/D50). An ASCII prefix is compared without allocating; any other is compared lowercased.
pub fn starts_with(name: &str, prefix: &str) -> bool {
    if prefix.is_ascii() {
        let (h, p) = (name.as_bytes(), prefix.as_bytes());
        return h.len() >= p.len() && h[..p.len()].eq_ignore_ascii_case(p);
    }
    name.to_lowercase().starts_with(&prefix.to_lowercase())
}

/// The first of `len` rows, from row `start` on and wrapping around to row 0, whose name
/// (`name(row)`) starts with `prefix`, ignoring case ([`starts_with`]). A `start` at or past
/// the end starts at row 0. `None` for an empty prefix or no match.
///
/// `name` may return `&str`, `String` or `Cow<str>`, whatever the list has at hand.
pub fn find<S: AsRef<str>>(
    len: usize,
    start: usize,
    prefix: &str,
    mut name: impl FnMut(usize) -> S,
) -> Option<usize> {
    if prefix.is_empty() || len == 0 {
        return None;
    }
    let start = if start < len { start } else { 0 };
    (start..len).chain(0..start).find(|&i| starts_with(name(i).as_ref(), prefix))
}

/// The letters typed into a list. They are forgotten after [`TIMEOUT`] without a key press.
/// Typing one letter repeatedly cycles through the rows starting with it (Explorer's
/// behaviour), while a longer word refines the prefix in place.
#[derive(Clone, Debug, Default)]
pub struct TypeAhead {
    /// The letters typed so far, lowercased.
    typed: String,
    last: Option<Instant>,
}

impl TypeAhead {
    pub fn new() -> TypeAhead {
        TypeAhead::default()
    }

    /// Adds `ch`, typed at `now` with the list's focused (or selected) row `current`
    /// (`None` when no row has it), and returns what to search for: the prefix and the row to
    /// start from, for [`find`] or [`crate::add::FileTree::find_row`].
    ///
    /// - While the typed letters are all the same one ("g", "gg", "ggg"), the prefix is that one
    ///   letter and the search starts after `current`: each press moves to the next row
    ///   starting with it, wrapping around.
    /// - Otherwise the prefix is everything typed ("gam") and the search starts at `current`,
    ///   so the focused row stays when it still matches.
    ///
    /// More than [`TIMEOUT`] after the last key press, the earlier letters are dropped first.
    /// Callers do not pass Space or control characters (see the module's notes).
    pub fn push(&mut self, ch: char, now: Instant, current: Option<usize>) -> (&str, usize) {
        let stale = self.last.is_none_or(|t| now.saturating_duration_since(t) > TIMEOUT);
        if stale {
            self.typed.clear();
        }
        self.last = Some(now);
        self.typed.extend(ch.to_lowercase());
        let mut chars = self.typed.chars();
        let first = chars.next().expect("just pushed");
        if chars.all(|c| c == first) {
            let one = &self.typed[..first.len_utf8()];
            (one, current.map_or(0, |c| c + 1))
        } else {
            (&self.typed, current.unwrap_or(0))
        }
    }

    /// [`TypeAhead::push`], then [`find`] among the list's `len` rows named by `name`: the row
    /// to select, if any.
    pub fn search<S: AsRef<str>>(
        &mut self,
        ch: char,
        now: Instant,
        current: Option<usize>,
        len: usize,
        name: impl FnMut(usize) -> S,
    ) -> Option<usize> {
        let (prefix, from) = self.push(ch, now, current);
        find(len, from, prefix, name)
    }

    /// The letters typed so far (lowercased), until [`TIMEOUT`] passes at the next push.
    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// Forgets the typed letters (for example when the list's contents change).
    pub fn reset(&mut self) {
        self.typed.clear();
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    const NAMES: [&str; 6] = ["alpha", "Gamma", "beta", "golf", "GAMUT", "Ötzi"];

    fn find_in(start: usize, prefix: &str) -> Option<usize> {
        find(NAMES.len(), start, prefix, |i| NAMES[i])
    }

    #[test]
    fn prefixes_ignore_case() {
        assert!(starts_with("Gamma", "ga"));
        assert!(starts_with("Gamma", "GAM"));
        assert!(!starts_with("Gamma", "gamman"));
        assert!(!starts_with("Ga", "gamma"));
        assert!(starts_with("Ötzi", "öt"));
        assert!(starts_with("ötzi", "ÖT"));
        assert!(!starts_with("Ötzi", "o"));
        assert!(starts_with("x", ""));
    }

    #[test]
    fn find_wraps_around() {
        assert_eq!(find_in(0, "g"), Some(1));
        assert_eq!(find_in(2, "g"), Some(3));
        assert_eq!(find_in(5, "g"), Some(1), "wraps");
        assert_eq!(find_in(99, "g"), Some(1), "past the end starts at 0");
        assert_eq!(find_in(2, "gam"), Some(4));
        assert_eq!(find_in(0, "ö"), Some(5));
        assert_eq!(find_in(0, "z"), None);
        assert_eq!(find_in(0, ""), None);
        assert_eq!(find(0, 0, "a", |_| ""), None);
        // owned names work too
        assert_eq!(find(3, 0, "B", |i| NAMES[i].to_owned()), Some(2));
        assert_eq!(find(3, 0, "B", |i| Cow::Borrowed(NAMES[i])), Some(2));
    }

    #[test]
    fn a_repeated_letter_cycles() {
        let mut ta = TypeAhead::new();
        let t = Instant::now();
        let mut cur = Some(0);
        let mut seen = Vec::new();
        for k in 0..4 {
            cur = ta.search('G', t + Duration::from_millis(100 * k), cur, NAMES.len(), |i| NAMES[i]);
            seen.push(cur.unwrap());
        }
        assert_eq!(seen, [1, 3, 4, 1]);
        assert_eq!(ta.typed(), "gggg");
    }

    #[test]
    fn letters_refine_in_place() {
        let mut ta = TypeAhead::new();
        let t = Instant::now();
        // "g" moves from alpha to Gamma, then "ga" keeps Gamma, "gamu" moves to GAMUT
        let mut cur = ta.search('g', t, Some(0), NAMES.len(), |i| NAMES[i]);
        assert_eq!(cur, Some(1));
        cur = ta.search('a', t, cur, NAMES.len(), |i| NAMES[i]);
        assert_eq!(cur, Some(1));
        assert_eq!(ta.push('m', t, cur), ("gam", 1));
        cur = ta.search('u', t, cur, NAMES.len(), |i| NAMES[i]);
        assert_eq!(cur, Some(4));
        // no row selected: from the top
        ta.reset();
        assert_eq!(ta.push('b', t, None), ("b", 0));
    }

    #[test]
    fn the_letters_are_forgotten_after_a_second() {
        let mut ta = TypeAhead::new();
        let t = Instant::now();
        assert_eq!(ta.push('g', t, Some(0)), ("g", 1));
        assert_eq!(ta.push('a', t + Duration::from_millis(900), Some(1)), ("ga", 1));
        // within the second of the last key press, not of the first
        assert_eq!(ta.push('m', t + Duration::from_millis(1800), Some(1)), ("gam", 1));
        assert_eq!(ta.push('b', t + Duration::from_millis(2900), Some(1)), ("b", 2));
        assert_eq!(ta.typed(), "b");
    }
}
