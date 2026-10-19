//! RSS Downloader filters: µTorrent 2.2.1's filter fields and how an item is matched against
//! them.

use super::FeedId;
use super::episode::{EpisodeSpec, Quality, parse_episode};
use super::feed::FeedItem;

pub type FilterId = u32;

/// One filter of the RSS Downloader (the dialog's field names in quotes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RssFilter {
    pub id: FilterId,
    /// The name in the filter list; its check box is `enabled`.
    pub name: String,
    pub enabled: bool,
    /// "Filter": `*` matches any run of characters, `?` one, `|` separates alternatives. Case is
    /// ignored, and `.` / `_` match a space (release names use them as spaces). An alternative
    /// without wildcards matches anywhere in the title; with them, the whole title. Empty
    /// matches nothing (use `*` for everything).
    pub pattern: String,
    /// "Not": the same syntax; a title that matches it is skipped. Empty skips nothing.
    pub not: String,
    /// "Save in": empty for the default download folder.
    pub save_in: String,
    /// "Feed": `None` for all feeds.
    pub feed: Option<FeedId>,
    /// "Quality": the qualities accepted; [`Quality::ALL`] accepts any title, including one
    /// that names no quality.
    pub quality: Quality,
    /// "Episode number" as typed (see [`EpisodeSpec`]); empty for any.
    pub episodes: String,
    /// "Label": empty for none (or the feed's name, see `RssSettings::feed_as_default_label`).
    pub label: String,
    /// "Minimum interval" in seconds: after an add, the filter adds nothing more until this
    /// much time has passed. 0 = no minimum.
    pub min_interval: u32,
    /// "Smart ep. filter": each episode is added once, whatever its quality or release group.
    pub smart: bool,
    /// "Give download highest priority": the frontend puts it at the top of the queue.
    pub top_priority: bool,
    /// "Add in paused state".
    pub paused: bool,
    /// When the filter last added something (unix time).
    pub last_match: Option<i64>,
    /// Episodes the smart filter has seen (`Episode::keys`), oldest first.
    pub seen_episodes: Vec<String>,
}

impl Default for RssFilter {
    fn default() -> RssFilter {
        RssFilter {
            id: 0,
            name: "New filter".into(),
            enabled: true,
            pattern: String::new(),
            not: String::new(),
            save_in: String::new(),
            feed: None,
            quality: Quality::ALL,
            episodes: String::new(),
            label: String::new(),
            min_interval: 0,
            smart: false,
            top_priority: false,
            paused: false,
            last_match: None,
            seen_episodes: Vec::new(),
        }
    }
}

/// µTorrent's "Minimum interval" choices, in seconds, with their names (the list as
/// remembered; any number of seconds works).
pub const MIN_INTERVALS: [(u32, &str); 13] = [
    (0, "0 (Ignore)"),
    (2 * 3600, "2 hours"),
    (5 * 3600, "5 hours"),
    (10 * 3600, "10 hours"),
    (12 * 3600, "12 hours"),
    (86_400, "1 day"),
    (2 * 86_400, "2 days"),
    (3 * 86_400, "3 days"),
    (4 * 86_400, "4 days"),
    (7 * 86_400, "1 week"),
    (14 * 86_400, "2 weeks"),
    (21 * 86_400, "3 weeks"),
    (30 * 86_400, "1 month"),
];

impl RssFilter {
    /// Whether the filter would take `item` from `feed` on its fields alone (the dialog's
    /// preview of matching items): Feed, Filter, Not, Quality and Episode number. Minimum
    /// interval and the smart episode filter depend on what was added before and are checked
    /// by `RssDownloader`.
    pub fn matches(&self, feed: FeedId, item: &FeedItem) -> bool {
        if self.feed.is_some_and(|f| f != feed) {
            return false;
        }
        let title = item.title.as_str();
        if !wildcard_match(&self.pattern, title)
            || (!self.not.trim().is_empty() && wildcard_match(&self.not, title))
        {
            return false;
        }
        if !self.quality.is_all() && !self.quality.intersects(Quality::of_title(title)) {
            return false;
        }
        if !self.episodes.trim().is_empty() {
            // a field that does not parse matches nothing rather than everything
            let Some(spec) = EpisodeSpec::parse(&self.episodes) else { return false };
            if !spec.matches(parse_episode(title)) {
                return false;
            }
        }
        true
    }
}

fn fold(c: char) -> char {
    match c {
        '.' | '_' => ' ',
        c => c.to_lowercase().next().unwrap_or(c),
    }
}

/// The "Filter" / "Not" syntax: `|`-separated alternatives with `*` and `?` wildcards (see
/// [`RssFilter::pattern`]). Empty matches nothing.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let text: Vec<char> = text.chars().map(fold).collect();
    pattern.split('|').map(str::trim).filter(|p| !p.is_empty()).any(|alt| {
        let p: Vec<char> = alt.chars().map(fold).collect();
        if p.iter().any(|&c| c == '*' || c == '?') {
            glob(&p, &text)
        } else {
            text.windows(p.len()).any(|w| w == p.as_slice())
        }
    })
}

/// Whole-text match with `*` and `?`, iterative with one backtrack point (no recursion, linear
/// memory, at most `pattern × text` steps).
fn glob(p: &[char], t: &[char]) -> bool {
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some(&c) if c == '?' || c == t[ti] => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rss::episode::Quality;

    #[test]
    fn wildcards() {
        let table: &[(&str, &str, bool)] = &[
            ("", "anything", false),
            ("  |  ", "anything", false),
            ("*", "anything", true),
            ("*", "", true),
            ("show", "The.Show.S01E02.720p", true),
            ("the show", "The.Show.S01E02.720p", true),
            ("The_Show", "the show s01e02", true),
            ("SHOW", "the show", true),
            ("other", "The.Show.S01E02", false),
            ("*show*720p*", "The.Show.S01E02.720p.HDTV", true),
            ("*show*1080p*", "The.Show.S01E02.720p.HDTV", false),
            ("show*", "The.Show.S01E02", false),
            ("the show*", "The.Show.S01E02", true),
            ("*s01e0?*", "The.Show.S01E02", true),
            ("*s01e0?", "The.Show.S01E02.x", false),
            ("*hdtv", "The.Show.HDTV", true),
            ("other|*show*", "The.Show", true),
            ("other | show", "The.Show", true),
            ("a|b", "c", false),
            ("**a**b**", "xaxbx", true),
            ("*a*a*a*a*b", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac", false),
            ("Café*", "café crème", true),
        ];
        for (p, t, want) in table {
            assert_eq!(wildcard_match(p, t), *want, "{p:?} vs {t:?}");
        }
    }

    fn item(title: &str) -> FeedItem {
        FeedItem { title: title.into(), link: format!("https://x/{title}"), ..FeedItem::default() }
    }

    #[test]
    fn filter_fields() {
        let f = RssFilter {
            pattern: "*show*".into(),
            not: "*german*|*sample*".into(),
            quality: Quality::P720.union(Quality::HR_HDTV),
            episodes: "S02E01-S02E10".into(),
            feed: Some(3),
            ..RssFilter::default()
        };
        let table: &[(FeedId, &str, bool)] = &[
            (3, "The.Show.S02E05.720p.HDTV", true),
            (4, "The.Show.S02E05.720p.HDTV", false),
            (3, "The.Show.S02E05.720p.GERMAN.HDTV", false),
            (3, "The.Show.S02E05.1080p.HDTV", false),
            (3, "The.Show.S02E05.HR.HDTV", true),
            (3, "The.Show.S02E05.HDTV", false),
            (3, "The.Show.S02E11.720p", false),
            (3, "The.Show.S02.Complete.720p", false),
            (3, "Another.Program.S02E05.720p", false),
        ];
        for (feed, title, want) in table {
            assert_eq!(f.matches(*feed, &item(title)), *want, "{feed} {title}");
        }
        let any = RssFilter { pattern: "*".into(), ..RssFilter::default() };
        assert!(any.matches(1, &item("anything at all")));
        assert!(!RssFilter::default().matches(1, &item("x")), "an empty filter matches nothing");
        let broken = RssFilter { pattern: "*".into(), episodes: "nonsense".into(), ..RssFilter::default() };
        assert!(!broken.matches(1, &item("Show.S01E01")));
    }
}
