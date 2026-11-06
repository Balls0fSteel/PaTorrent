//! What the search windows show and pick, without widgets (the native macOS GUI's, ported from
//! the GTK and Win32 windows, which keep their own copies for now):
//!
//! - the toolbar search field's mode ([`Mode`], kept as `ui.search_source`), its menu
//!   ([`mode_menu`]) and grey hint ([`Mode::cue`]), and the sources a search asks ([`picked`]);
//! - the Search Results window's source choices ([`source_choices`]), columns ([`Col`]), cells
//!   ([`cells`]), order ([`sorted`]), what keeps a row selected ([`result_key`]) and each
//!   source's status line ([`status_line`]);
//! - the Add / Edit Search Source form ([`SourceForm`], [`apply_form`]);
//! - the search helper's site picker ([`initially_ticked`], [`ticked_ids`]).
//!
//! An API key is never shown: a source in the preferences is remembered by its URL with the key
//! masked ([`source_id`]), and every line has it taken out.

use std::cmp::Ordering;

use super::helper::Indexer;
use super::source::url_host;
use super::{SearchResult, SearchSource, Source, SourceStatus, category_text, main_categories, redact};
use crate::prefs::Prefs;
use crate::{fmt, tr};

/// The longest query sent (as the Web UI's).
pub const MAX_QUERY: usize = 256;

/// The preference that remembers the mode: missing for the name filter, `*` for all sources,
/// else the source's [`source_id`] (as the Win32 window's).
pub const MODE_PREF: &str = "ui.search_source";

/// What the toolbar's search field does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Typing filters the torrent list by name (D35).
    #[default]
    Filter,
    /// Enter searches every enabled source.
    All,
    /// Enter searches the source with this [`source_id`].
    Source(String),
}

/// A source as the mode remembers it: its URL with the key masked (so `rookparcel.ini` holds no
/// key in the clear; two Prowlarr indexers on one host differ by their path).
pub fn source_id(s: &Source) -> String {
    redact(s.url.trim(), &s.api_key)
}

impl Mode {
    pub fn load(prefs: &Prefs) -> Mode {
        match prefs.get(MODE_PREF).map(str::trim) {
            None | Some("") => Mode::Filter,
            Some("*") => Mode::All,
            Some(id) => Mode::Source(id.to_owned()),
        }
    }

    pub fn store(&self, prefs: &mut Prefs) {
        match self {
            Mode::Filter => prefs.remove(MODE_PREF),
            Mode::All => prefs.set(MODE_PREF, "*"),
            Mode::Source(id) => prefs.set(MODE_PREF, id),
        }
    }

    /// The mode with these sources: a picked source that is gone or switched off, or all
    /// sources with none on, fall back to the name filter.
    pub fn resolve(self, sources: &[Source]) -> Mode {
        match &self {
            Mode::Source(id) if !sources.iter().any(|s| s.enabled && source_id(s) == *id) => Mode::Filter,
            Mode::All if !sources.iter().any(|s| s.enabled) => Mode::Filter,
            _ => self,
        }
    }

    /// The source at `index` of `sources` as a mode.
    pub fn of_source(sources: &[Source], index: usize) -> Option<Mode> {
        sources.get(index).map(|s| Mode::Source(source_id(s)))
    }

    pub fn is_filter(&self) -> bool {
        *self == Mode::Filter
    }

    /// The search field's grey hint, which says what Enter does.
    pub fn cue(&self, sources: &[Source]) -> String {
        match self {
            Mode::Filter => tr!(SearchCue).to_owned(),
            Mode::All => tr!(SearchCueAll).to_owned(),
            Mode::Source(id) => {
                let label = sources.iter().find(|s| source_id(s) == *id).map(Source::label);
                tr!(SearchCueSource, source = label.unwrap_or_else(|| id.clone()))
            }
        }
    }
}

/// One choice of the search field's menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub text: String,
    pub mode: Mode,
    /// The source's index in the stored sources (the menu item's value).
    pub index: Option<usize>,
    pub checked: bool,
    pub enabled: bool,
}

/// The search field menu's choices: "Search torrent names", each enabled source, "All sources"
/// (greyed with none). The caller adds a separator and Manage Sources. Two enabled sources with
/// the same name (Prowlarr indexers on one host, unnamed) show their masked URLs instead.
pub fn mode_menu(sources: &[Source], current: &Mode) -> Vec<MenuEntry> {
    let entry = |text: String, mode: Mode, index: Option<usize>, enabled: bool| MenuEntry {
        checked: mode == *current,
        text,
        mode,
        index,
        enabled,
    };
    let on: Vec<(usize, &Source)> = sources.iter().enumerate().filter(|(_, s)| s.enabled).collect();
    let mut out = vec![entry(tr!(SearchCue).to_owned(), Mode::Filter, None, true)];
    for &(i, s) in &on {
        let label = s.label();
        let twin = on.iter().filter(|(_, o)| o.label() == label).count() > 1;
        let text = if twin { source_id(s) } else { label };
        out.push(entry(text, Mode::Source(source_id(s)), Some(i), true));
    }
    out.push(entry(tr!(SearchAllSources).to_owned(), Mode::All, None, !on.is_empty()));
    out
}

/// The sources a search in `mode` asks: every enabled one, or the one picked (searched even when
/// switched off since it was picked); `None` for the name filter or nothing to search.
pub fn picked(mode: &Mode, sources: &[Source]) -> Option<Vec<Source>> {
    let out: Vec<Source> = match mode {
        Mode::Filter => return None,
        Mode::All => sources.iter().filter(|s| s.enabled).cloned().collect(),
        Mode::Source(id) => sources
            .iter()
            .find(|s| source_id(s) == *id)
            .map(|s| Source { enabled: true, ..s.clone() })
            .into_iter()
            .collect(),
    };
    (!out.is_empty()).then_some(out)
}

/// The Search Results window's source list: "All sources", then each enabled source and the
/// one `mode` picked (when switched off).
pub fn source_choices(sources: &[Source], mode: &Mode) -> Vec<(Mode, String)> {
    let mut out = vec![(Mode::All, tr!(SearchAllSources).to_owned())];
    for s in sources {
        let m = Mode::Source(source_id(s));
        if s.enabled || m == *mode {
            out.push((m, s.label()));
        }
    }
    out
}

/// The query as sent: trimmed, at most [`MAX_QUERY`] characters.
pub fn query_text(text: &str) -> String {
    text.trim().chars().take(MAX_QUERY).collect()
}

/// The categories a choice of the Search Results window's category list asks: 0 is "All
/// categories" (none), then [`main_categories`].
pub fn category_choice(index: usize) -> Vec<u32> {
    match index {
        0 => Vec::new(),
        n => main_categories().get(n - 1).map(|(id, _)| vec![*id]).unwrap_or_default(),
    }
}

/// The category list's texts: "All categories", then [`main_categories`].
pub fn category_names() -> Vec<String> {
    let mut out = vec![tr!(SearchAllCategories).to_owned()];
    out.extend(main_categories().iter().map(|(_, n)| (*n).to_owned()));
    out
}

// ---- the results' rows ---------------------------------------------------------------------

/// A Search Results column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Name,
    Size,
    Seeds,
    Peers,
    Category,
    Age,
    Source,
}

impl Col {
    /// The columns in [`cells`] order.
    pub const ALL: [Col; 7] =
        [Col::Name, Col::Size, Col::Seeds, Col::Peers, Col::Category, Col::Age, Col::Source];

    pub fn title(self) -> &'static str {
        match self {
            Col::Name => tr!(ColName),
            Col::Size => tr!(ColSize),
            Col::Seeds => tr!(ColSeeds),
            Col::Peers => tr!(ColPeers),
            Col::Category => tr!(SearchColCategory),
            Col::Age => tr!(SearchColAge),
            Col::Source => tr!(SearchColSource),
        }
    }

    /// The width it starts with, in points.
    pub fn width(self) -> f64 {
        match self {
            Col::Name => 260.0,
            Col::Size => 80.0,
            Col::Seeds | Col::Peers => 60.0,
            Col::Category => 80.0,
            Col::Age => 90.0,
            Col::Source => 120.0,
        }
    }

    /// Numbers are right-aligned.
    pub fn right_aligned(self) -> bool {
        matches!(self, Col::Size | Col::Seeds | Col::Peers | Col::Age)
    }

    /// The first click sorts numbers most first, words A to Z, the age newest first.
    pub fn first_descending(self) -> bool {
        matches!(self, Col::Size | Col::Seeds | Col::Peers)
    }
}

/// The sources that returned a result, as its Source cell shows them.
fn source_names(r: &SearchResult, names: &[String]) -> String {
    let from: Vec<&str> = r.sources.iter().filter_map(|&i| names.get(i)).map(String::as_str).collect();
    from.join(", ")
}

/// A result's cells, in [`Col::ALL`] order; `names` are the searched sources' labels, `now` the
/// Unix time the ages count to.
pub fn cells(r: &SearchResult, names: &[String], now: i64) -> [String; 7] {
    let count = |v: Option<u32>| v.map(|v| v.to_string()).unwrap_or_default();
    [
        r.name.clone(),
        r.size.map(fmt::size).unwrap_or_default(),
        count(r.seeders),
        count(r.peers),
        category_text(&r.categories),
        r.published.map(|p| fmt::duration(now.saturating_sub(p).max(0) as u64)).unwrap_or_default(),
        source_names(r, names),
    ]
}

/// The order the rows show `results` in: as merged (most seeders first) without a sort, else by
/// `col` (unknown values last either way; ties keep the merged order). Age sorts by the date
/// published, the newest first ascending.
pub fn sorted(results: &[SearchResult], names: &[String], sort: Option<(Col, bool)>) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..results.len()).collect();
    let Some((col, descending)) = sort else { return rows };
    let text = |r: &SearchResult| match col {
        Col::Name => r.name.to_lowercase(),
        Col::Category => category_text(&r.categories).to_lowercase(),
        _ => source_names(r, names).to_lowercase(),
    };
    let number = |r: &SearchResult| -> Option<i64> {
        match col {
            Col::Size => r.size.map(|v| v.min(i64::MAX as u64) as i64),
            Col::Seeds => r.seeders.map(i64::from),
            Col::Peers => r.peers.map(i64::from),
            // the newest has the smallest age
            Col::Age => r.published.map(|p| p.saturating_neg()),
            _ => None,
        }
    };
    let numeric = !matches!(col, Col::Name | Col::Category | Col::Source);
    rows.sort_by(|&a, &b| {
        let (ra, rb) = (&results[a], &results[b]);
        let o = if numeric {
            match (number(ra), number(rb)) {
                (Some(x), Some(y)) => {
                    let o = x.cmp(&y);
                    if descending { o.reverse() } else { o }
                }
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        } else {
            let o = text(ra).cmp(&text(rb));
            if descending { o.reverse() } else { o }
        };
        o.then(a.cmp(&b))
    });
    rows
}

/// What keeps a result's row selected while answers come in and the rows move: its info-hash,
/// else its name and size (as [`super::merge`] de-duplicates).
pub fn result_key(r: &SearchResult) -> String {
    if r.info_hash.is_empty() {
        format!("{}\n{}", r.name.to_lowercase(), r.size.map(|s| s.to_string()).unwrap_or_default())
    } else {
        r.info_hash.clone()
    }
}

/// A source's status line (the key taken out of an error).
pub fn status_line(s: &SearchSource) -> String {
    let source = s.source.label();
    match &s.status {
        SourceStatus::Searching => tr!(SearchSearching, source),
        SourceStatus::Done(count) => tr!(SearchFound, source, count),
        SourceStatus::Failed(error) => tr!(SearchFailed, source, error = s.source.redact(error)),
        SourceStatus::Stopped => tr!(SearchStopped, source),
    }
}

// ---- editing the sources -------------------------------------------------------------------

/// The Add / Edit Search Source dialog's fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceForm {
    pub name: String,
    pub url: String,
    /// Empty when editing: the stored key stays.
    pub key: String,
    pub enabled: bool,
}

/// An `http://` or `https://` URL with a host, trimmed.
pub fn valid_url(url: &str) -> Option<String> {
    let url = url.trim();
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    (matches!(scheme.as_deref(), Some("http" | "https")) && url_host(url).is_some()).then(|| url.to_owned())
}

/// Puts `form` into `sources`: a new source (`index` `None`) at the end, else source `index`
/// changed (an empty key keeps its stored one). Returns the source's index, or the message when
/// the URL is not an `http(s)://` one.
pub fn apply_form(
    sources: &mut Vec<Source>,
    index: Option<usize>,
    form: &SourceForm,
) -> Result<usize, String> {
    let url = valid_url(&form.url).ok_or_else(|| tr!(SearchNoUrl).to_owned())?;
    let key = form.key.trim().to_owned();
    let mut s = Source { name: form.name.trim().to_owned(), url, api_key: key, enabled: form.enabled };
    match index.filter(|&i| i < sources.len()) {
        Some(i) => {
            if s.api_key.is_empty() {
                s.api_key = std::mem::take(&mut sources[i].api_key);
            }
            sources[i] = s;
            Ok(i)
        }
        None => {
            sources.push(s);
            Ok(sources.len() - 1)
        }
    }
}

// ---- the search helper's site picker -------------------------------------------------------

/// The picker's rows start ticked only where the site was ticked before (configured), never a
/// FlareSolverr one.
pub fn initially_ticked(list: &[Indexer]) -> Vec<bool> {
    list.iter().map(|i| i.configured && !i.needs_flaresolverr).collect()
}

/// The ids of the ticked sites (a FlareSolverr one never).
pub fn ticked_ids(list: &[Indexer], ticks: &[bool]) -> Vec<String> {
    list.iter().zip(ticks).filter(|(i, t)| **t && !i.needs_flaresolverr).map(|(i, _)| i.id.clone()).collect()
}

/// A site's line under its name: language, type, and the FlareSolverr note.
pub fn site_detail(i: &Indexer) -> String {
    if i.needs_flaresolverr {
        format!("{} · {} · {}", i.language, i.kind, tr!(SearchHelperNeedsFlaresolverr))
    } else {
        format!("{} · {}", i.language, i.kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(name: &str, url: &str, enabled: bool) -> Source {
        Source { name: name.into(), url: url.into(), api_key: format!("key-{name}"), enabled }
    }

    fn result(name: &str, size: Option<u64>, seeders: Option<u32>, published: Option<i64>) -> SearchResult {
        SearchResult {
            name: name.into(),
            size,
            seeders,
            peers: seeders.map(|s| s + 1),
            info_hash: String::new(),
            categories: Vec::new(),
            published,
            link: String::new(),
            magnet: String::new(),
            source: 0,
            sources: vec![0],
        }
    }

    #[test]
    fn the_mode_is_kept_without_the_key() {
        let mut p = Prefs::default();
        assert_eq!(Mode::load(&p), Mode::Filter);
        let s = Source {
            name: "a".into(),
            url: "http://h/api?apikey=SECRET".into(),
            api_key: "SECRET".into(),
            enabled: true,
        };
        let m = Mode::of_source(std::slice::from_ref(&s), 0).unwrap();
        m.store(&mut p);
        let kept = p.get(MODE_PREF).unwrap().to_owned();
        assert!(!kept.contains("SECRET"), "{kept}");
        assert_eq!(Mode::load(&p), m);
        Mode::All.store(&mut p);
        assert_eq!(p.get(MODE_PREF), Some("*"));
        assert_eq!(Mode::load(&p), Mode::All);
        Mode::Filter.store(&mut p);
        assert_eq!(p.get(MODE_PREF), None);
        assert_eq!(Mode::of_source(&[], 0), None);
    }

    #[test]
    fn a_mode_falls_back_to_the_filter() {
        let sources = [src("a", "http://h/1/api", true), src("b", "http://h/2/api", false)];
        let a = Mode::Source(source_id(&sources[0]));
        let b = Mode::Source(source_id(&sources[1]));
        assert_eq!(a.clone().resolve(&sources), a);
        assert_eq!(b.resolve(&sources), Mode::Filter, "switched off");
        assert_eq!(Mode::Source("gone".into()).resolve(&sources), Mode::Filter);
        assert_eq!(Mode::All.resolve(&sources), Mode::All);
        assert_eq!(Mode::All.resolve(&sources[1..]), Mode::Filter);
        assert_eq!(Mode::Filter.resolve(&[]), Mode::Filter);
    }

    #[test]
    fn the_cue_says_what_enter_does() {
        let sources = [src("Idx", "http://h/1/api", true)];
        assert_eq!(Mode::Filter.cue(&sources), "Search torrent names");
        assert_eq!(Mode::All.cue(&sources), "Search all sources");
        assert_eq!(Mode::Source(source_id(&sources[0])).cue(&sources), "Search Idx");
    }

    #[test]
    fn the_menu_lists_the_filter_enabled_sources_and_all() {
        let none = mode_menu(&[], &Mode::Filter);
        assert_eq!(none.len(), 2);
        assert!(none[0].checked && none[0].enabled && none[0].mode == Mode::Filter);
        assert_eq!(none[0].text, "Search torrent names");
        assert!(!none[1].enabled, "All sources greyed without a source");
        let sources = [
            src("a", "http://h/1/api", true),
            src("b", "http://h/2/api", false),
            src("", "http://j/x", true),
            src("", "http://j/y", true),
        ];
        let current = Mode::Source(source_id(&sources[2]));
        let m = mode_menu(&sources, &current);
        let texts: Vec<&str> = m.iter().map(|e| e.text.as_str()).collect();
        // the two unnamed sources on host "j" show their URLs
        assert_eq!(texts, ["Search torrent names", "a", "http://j/x", "http://j/y", "All sources"]);
        let index: Vec<Option<usize>> = m.iter().map(|e| e.index).collect();
        assert_eq!(index, [None, Some(0), Some(2), Some(3), None]);
        let checked: Vec<bool> = m.iter().map(|e| e.checked).collect();
        assert_eq!(checked, [false, false, true, false, false]);
        assert!(m[4].enabled);
    }

    #[test]
    fn a_search_asks_the_picked_sources() {
        let all = [src("a", "http://h/1/api", true), src("b", "http://h/2/api", false)];
        assert_eq!(picked(&Mode::Filter, &all), None);
        assert_eq!(picked(&Mode::All, &all), Some(vec![all[0].clone()]));
        // picked by name: searched although switched off
        let b = Mode::Source(source_id(&all[1]));
        assert_eq!(picked(&b, &all), Some(vec![Source { enabled: true, ..all[1].clone() }]));
        assert_eq!(picked(&Mode::Source("gone".into()), &all), None);
        assert_eq!(picked(&Mode::All, &all[1..]), None);
        let choices: Vec<String> = source_choices(&all, &b).into_iter().map(|(_, t)| t).collect();
        assert_eq!(choices, ["All sources", "a", "b"]);
        assert_eq!(source_choices(&all, &Mode::All).len(), 2);
    }

    #[test]
    fn queries_and_categories() {
        assert_eq!(query_text("  ubuntu  "), "ubuntu");
        assert_eq!(query_text(&"x".repeat(400)).len(), MAX_QUERY);
        assert_eq!(category_choice(0), Vec::<u32>::new());
        assert_eq!(category_choice(1), vec![main_categories()[0].0]);
        assert_eq!(category_choice(99), Vec::<u32>::new());
        assert_eq!(category_names().len(), main_categories().len() + 1);
    }

    #[test]
    fn the_form_adds_and_edits_keeping_the_key() {
        let mut sources = vec![src("a", "http://h/1/api", true)];
        let form = |name: &str, url: &str, key: &str| SourceForm {
            name: name.into(),
            url: url.into(),
            key: key.into(),
            enabled: false,
        };
        assert_eq!(apply_form(&mut sources, None, &form("b", " https://j/x ", "k2")), Ok(1));
        assert_eq!(
            sources[1],
            Source { name: "b".into(), url: "https://j/x".into(), api_key: "k2".into(), enabled: false }
        );
        // an empty key keeps the stored one
        assert_eq!(apply_form(&mut sources, Some(0), &form(" a2 ", "http://h/9/api", "")), Ok(0));
        assert_eq!(sources[0].api_key, "key-a");
        assert_eq!(
            (sources[0].name.as_str(), sources[0].url.as_str(), sources[0].enabled),
            ("a2", "http://h/9/api", false)
        );
        assert_eq!(apply_form(&mut sources, Some(0), &form("a2", "http://h/9/api", " k3 ")), Ok(0));
        assert_eq!(sources[0].api_key, "k3");
        for bad in ["", "h/1/api", "ftp://h/1", "http://", "javascript:alert(1)"] {
            assert_eq!(
                apply_form(&mut sources, None, &form("x", bad, "")),
                Err("Enter the source's Torznab URL.".to_owned()),
                "{bad}"
            );
        }
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn status_lines_never_show_the_key() {
        let s = src("idx", "http://h/1/api", true);
        let line = |status| status_line(&SearchSource { source: s.clone(), status });
        assert_eq!(line(SourceStatus::Searching), "idx: searching...");
        assert_eq!(line(SourceStatus::Done(3)), "idx: 3 results");
        assert_eq!(line(SourceStatus::Stopped), "idx: stopped");
        assert_eq!(
            line(SourceStatus::Failed(
                "GET http://h/1/api?t=caps&apikey=key-idx refused, key key-idx".into()
            )),
            "idx: GET http://h/1/api?t=caps&apikey=*** refused, key ***"
        );
    }

    #[test]
    fn rows_sort_by_any_column() {
        let mut rs = vec![
            result("Beta", Some(2_000), Some(50), Some(1_000)),
            result("alpha", None, Some(10), Some(3_000)),
            result("Gamma", Some(1_000), None, None),
        ];
        rs[1].sources = vec![0, 1];
        rs[2].categories = vec![5040];
        let names = ["One".to_owned(), "Two".to_owned()];
        assert_eq!(sorted(&rs, &names, None), [0, 1, 2]);
        assert_eq!(sorted(&rs, &names, Some((Col::Name, false))), [1, 0, 2]);
        assert_eq!(sorted(&rs, &names, Some((Col::Name, true))), [2, 0, 1]);
        // unknown sizes and seeders last, either way
        assert_eq!(sorted(&rs, &names, Some((Col::Size, false))), [2, 0, 1]);
        assert_eq!(sorted(&rs, &names, Some((Col::Size, true))), [0, 2, 1]);
        assert_eq!(sorted(&rs, &names, Some((Col::Seeds, true))), [0, 1, 2]);
        assert_eq!(sorted(&rs, &names, Some((Col::Category, true))), [2, 0, 1]);
        // the youngest first
        assert_eq!(sorted(&rs, &names, Some((Col::Age, false))), [1, 0, 2]);
        assert_eq!(sorted(&rs, &names, Some((Col::Source, true))), [1, 0, 2]);
        let c = cells(&rs[1], &names, 3_000 + 7_200);
        assert_eq!(c[0], "alpha");
        assert_eq!((c[1].as_str(), c[2].as_str(), c[3].as_str()), ("", "10", "11"));
        assert_eq!(c[5], fmt::duration(7_200));
        assert_eq!(c[6], "One, Two");
        assert_eq!(cells(&rs[2], &names, 0)[4], "TV");
        assert_eq!(Col::ALL.len(), c.len());
        assert!(Col::Seeds.first_descending() && !Col::Name.first_descending());
        assert!(Col::Age.right_aligned() && !Col::Source.right_aligned());
    }

    #[test]
    fn rows_keep_their_selection_by_hash_or_name_and_size() {
        let mut a = result("Name", Some(5), Some(1), None);
        assert_eq!(result_key(&a), "name\n5");
        a.info_hash = "ab".repeat(20);
        assert_eq!(result_key(&a), "ab".repeat(20));
    }

    #[test]
    fn nothing_is_ticked_for_the_user() {
        let site = |id: &str, configured, cf| Indexer {
            id: id.into(),
            configured,
            needs_flaresolverr: cf,
            language: "en-US".into(),
            kind: "public".into(),
            ..Indexer::default()
        };
        let list = [site("a", false, false), site("b", true, false), site("c", true, true)];
        assert_eq!(initially_ticked(&list), [false, true, false]);
        assert_eq!(ticked_ids(&list, &[true, true, true]), ["a", "b"]);
        assert_eq!(site_detail(&list[0]), "en-US · public");
        assert!(site_detail(&list[2]).ends_with(tr!(SearchHelperNeedsFlaresolverr)));
    }
}
