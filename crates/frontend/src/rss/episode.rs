//! Episodes and qualities read from release titles, and the RSS filter's "Episode number"
//! ranges.
//!
//! Titles name episodes as `S01E02` (also `S01E02E03`, `S01E02-E03`, `S1.E2`), `1x02`, an air
//! date (`2026.10.19`, `2026-10-19`) or, for anime, an absolute number (`Show - 12`,
//! `Episode 12`). A season pack (`S01` alone) has no episode.

/// The episode a title names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Episode {
    /// Episodes `first..=last` of a season; season 0 when the title has none (absolute
    /// numbering).
    Numbered { season: u32, first: u32, last: u32 },
    /// An air date as `yyyymmdd`.
    Dated(u32),
}

impl Episode {
    /// The key the smart episode filter remembers, one per episode (`s<season>e<episode>` or
    /// `d<yyyymmdd>`).
    pub fn keys(&self) -> Vec<String> {
        match *self {
            Episode::Numbered { season, first, last } => {
                (first..=last.min(first.saturating_add(99))).map(|e| format!("s{season}e{e}")).collect()
            }
            Episode::Dated(d) => vec![format!("d{d}")],
        }
    }
}

/// Up to `max` digits at `i`, not followed by another digit: the number and where it ends.
fn digits_at(b: &[u8], i: usize, max: usize) -> Option<(u32, usize)> {
    let mut j = i;
    while j < b.len() && j - i < max && b[j].is_ascii_digit() {
        j += 1;
    }
    if j == i || b.get(j).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(&b[i..j]).ok()?.parse().ok().map(|n| (n, j))
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

/// The episode a release title names, if any.
pub fn parse_episode(title: &str) -> Option<Episode> {
    let lower = title.to_ascii_lowercase();
    let b = lower.as_bytes();
    let before_ok = |i: usize| i == 0 || !is_word(b[i - 1]);
    let after_ok = |j: usize| j >= b.len() || !b[j].is_ascii_alphanumeric() || b[j] == b'v';
    // S01E02, S01.E02, S01E02E03, S01E02-E03, S01E02-03
    for i in 0..b.len() {
        if b[i] != b's' || !before_ok(i) {
            continue;
        }
        let Some((season, mut j)) = digits_at(b, i + 1, 3) else { continue };
        if matches!(b.get(j), Some(b'.' | b' ' | b'_' | b'-')) && b.get(j + 1) == Some(&b'e') {
            j += 1;
        }
        if b.get(j) != Some(&b'e') {
            continue;
        }
        let Some((first, mut j)) = digits_at(b, j + 1, 4) else { continue };
        let mut last = first;
        loop {
            let k = match (b.get(j), b.get(j + 1)) {
                (Some(b'e'), _) => j + 1,
                (Some(b'-'), Some(b'e')) => j + 2,
                (Some(b'-'), Some(c)) if c.is_ascii_digit() => j + 1,
                _ => break,
            };
            match digits_at(b, k, 4) {
                Some((n, next)) if n > last && n - last < 100 => {
                    last = n;
                    j = next;
                }
                _ => break,
            }
        }
        if after_ok(j) {
            return Some(Episode::Numbered { season, first, last });
        }
    }
    // 1x02, 01x02-03 (not 1920x1080: the season has at most two digits and nothing before it)
    for i in 0..b.len() {
        if !b[i].is_ascii_digit() || !before_ok(i) {
            continue;
        }
        let Some((season, j)) = digits_at(b, i, 2) else { continue };
        if b.get(j) != Some(&b'x') {
            continue;
        }
        let Some((first, mut j)) = digits_at(b, j + 1, 3) else { continue };
        let mut last = first;
        if b.get(j) == Some(&b'-') {
            if let Some((n, next)) = digits_at(b, j + 1, 3).filter(|&(n, _)| n > first && n - first < 100) {
                last = n;
                j = next;
            }
        }
        if after_ok(j) {
            return Some(Episode::Numbered { season, first, last });
        }
    }
    // an air date: 2026.10.19, 2026-10-19, 2026_10_19, 2026 10 19
    for i in 0..b.len() {
        if !before_ok(i) {
            continue;
        }
        let Some((y, j)) = digits_at(b, i, 4).filter(|&(y, j)| j - i == 4 && (1950..2100).contains(&y))
        else {
            continue;
        };
        let sep = |k: usize| matches!(b.get(k), Some(b'.' | b'-' | b'_' | b' '));
        if !sep(j) {
            continue;
        }
        let Some((m, k)) = digits_at(b, j + 1, 2).filter(|&(m, k)| k - j == 3 && (1..=12).contains(&m))
        else {
            continue;
        };
        if !sep(k) || b.get(k) != b.get(j) {
            continue;
        }
        let Some((d, l)) = digits_at(b, k + 1, 2).filter(|&(d, l)| l - k == 3 && (1..=31).contains(&d))
        else {
            continue;
        };
        if l >= b.len() || !is_word(b[l]) {
            return Some(Episode::Dated(y * 10_000 + m * 100 + d));
        }
    }
    // "Episode 12", "Ep 12", "Ep.12", "E12"
    for (word, need_sep) in [("episode", true), ("ep", false), ("e", false)] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(word) {
            let i = from + pos;
            from = i + 1;
            if !before_ok(i) {
                continue;
            }
            let mut j = i + word.len();
            if matches!(b.get(j), Some(b' ' | b'.' | b'_')) {
                j += 1;
            } else if need_sep {
                continue;
            }
            if let Some((n, k)) = digits_at(b, j, 4) {
                if after_ok(k) {
                    return Some(Episode::Numbered { season: 0, first: n, last: n });
                }
            }
        }
    }
    // anime: "[Group] Show - 12 [1080p]", "Show - 12v2", "Show - 01-02"
    let mut from = 0;
    while let Some(pos) = lower[from..].find(" - ") {
        let i = from + pos + 3;
        from = i;
        let Some((first, mut j)) = digits_at(b, i, 4) else { continue };
        let mut last = first;
        if b.get(j) == Some(&b'-') {
            if let Some((n, next)) = digits_at(b, j + 1, 4).filter(|&(n, _)| n > first && n - first < 100) {
                last = n;
                j = next;
            }
        }
        // a version suffix: 12v2
        if b.get(j) == Some(&b'v') {
            if let Some((_, next)) = digits_at(b, j + 1, 2) {
                j = next;
            }
        }
        let end_ok = j >= b.len() || matches!(b[j], b' ' | b'[' | b'(' | b'.' | b'_');
        // a four-digit number there is far more often a year than an episode
        if end_ok && !(1950..2100).contains(&first) {
            // "Show S2 - 14": a season word right before the dash
            let before = lower[..i - 3].rsplit(' ').next().unwrap_or("");
            let season = before
                .strip_prefix('s')
                .filter(|d| (1..=2).contains(&d.len()) && d.bytes().all(|c| c.is_ascii_digit()))
                .and_then(|d| d.parse().ok())
                .unwrap_or(0);
            return Some(Episode::Numbered { season, first, last });
        }
    }
    None
}

/// True when the title says it fixes an earlier release of the same episode (the smart
/// filter's repack rule).
pub fn is_repack(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| matches!(w, "repack" | "proper" | "rerip" | "real"))
}

// --- Episode ranges ---------------------------------------------------------------------------

/// One range of an "Episode number" field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Range {
    /// `(season, episode)` bounds, inclusive: `S01E01-S01E10`, `S02` (the whole season),
    /// `S01E05-` (from there on).
    Seasoned((u32, u32), (u32, u32)),
    /// Episode numbers in any season (or none): `5`, `1-12`, `100-`.
    Episodes(u32, u32),
    /// Air dates as `yyyymmdd`.
    Dates(u32, u32),
}

/// The filter's "Episode number" field: ranges separated by `,` or `;`. Each is a single
/// episode or `from-to`, written as `S01E02`, `1x02`, `S02` (a whole season), a bare number
/// (that episode in any season, for absolute numbering), or a date (`2026.10.19`); `to` may be
/// left out for "from here on" and may drop the season (`S01E01-10`, `1x01-10`).
///
/// µTorrent 2.2.1's own syntax is from memory only (`1x1-12` style examples); everything else
/// here is PaTorrent's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EpisodeSpec {
    ranges: Vec<Range>,
    text: String,
}

/// One endpoint: season (if any) and episode, or a date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    /// `S02` alone.
    Season(u32),
    Seasoned(u32, u32),
    Episode(u32),
    Date(u32),
}

fn parse_point(s: &str) -> Option<Point> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Some(Episode::Dated(d)) = parse_episode(&s) {
        return Some(Point::Date(d));
    }
    let num = |t: &str| -> Option<u32> {
        (!t.is_empty() && t.len() <= 4 && t.bytes().all(|c| c.is_ascii_digit())).then(|| t.parse().ok())?
    };
    if let Some(rest) = s.strip_prefix('s') {
        return match rest.split_once('e') {
            Some((season, ep)) => Some(Point::Seasoned(num(season)?, num(ep)?)),
            None => Some(Point::Season(num(rest)?)),
        };
    }
    if let Some((season, ep)) = s.split_once('x') {
        return Some(Point::Seasoned(num(season)?, num(ep)?));
    }
    num(s.strip_prefix('e').unwrap_or(&s)).map(Point::Episode)
}

fn parse_range(part: &str) -> Option<Range> {
    let part = part.trim();
    // dates contain '-' themselves: split at a '-' that is followed by a full point
    let (from, to) = match split_range(part) {
        Some((a, b)) => (parse_point(a)?, if b.trim().is_empty() { None } else { Some(b) }),
        None => {
            let p = parse_point(part)?;
            return Some(match p {
                Point::Season(s) => Range::Seasoned((s, 0), (s, u32::MAX)),
                Point::Seasoned(s, e) => Range::Seasoned((s, e), (s, e)),
                Point::Episode(e) => Range::Episodes(e, e),
                Point::Date(d) => Range::Dates(d, d),
            });
        }
    };
    let open = u32::MAX;
    let r = match (from, to) {
        (Point::Season(s), None) => Range::Seasoned((s, 0), (open, open)),
        (Point::Seasoned(s, e), None) => Range::Seasoned((s, e), (open, open)),
        (Point::Episode(e), None) => Range::Episodes(e, open),
        (Point::Date(d), None) => Range::Dates(d, open),
        (from, Some(to)) => {
            let to = to.trim().to_ascii_lowercase();
            match (from, parse_point(&to)?) {
                (Point::Season(a), Point::Season(b)) => Range::Seasoned((a, 0), (b, open)),
                (Point::Season(a), Point::Seasoned(b, e)) => Range::Seasoned((a, 0), (b, e)),
                (Point::Seasoned(s, e), Point::Seasoned(t, f)) => Range::Seasoned((s, e), (t, f)),
                (Point::Seasoned(s, e), Point::Season(t)) => Range::Seasoned((s, e), (t, open)),
                // S01E01-10, 1x01-10, S01E01-E10: the season carries over
                (Point::Seasoned(s, e), Point::Episode(f)) => Range::Seasoned((s, e), (s, f)),
                (Point::Episode(a), Point::Episode(b)) => Range::Episodes(a, b),
                (Point::Date(a), Point::Date(b)) => Range::Dates(a, b),
                _ => return None,
            }
        }
    };
    Some(r)
}

/// `a-b` or `a-`, where `a` may itself be a date with dashes.
fn split_range(part: &str) -> Option<(&str, &str)> {
    let dashes: Vec<usize> = part.match_indices('-').map(|(i, _)| i).collect();
    // try each dash: the left side must be a point, the right side empty or a point
    dashes.into_iter().find_map(|i| {
        let (a, b) = (&part[..i], &part[i + 1..]);
        (parse_point(a).is_some() && (b.trim().is_empty() || parse_point(b).is_some())).then_some((a, b))
    })
}

impl EpisodeSpec {
    /// Parses the field; `None` when a part of it makes no sense (the dialog shows that).
    /// An empty field is an empty spec, which matches everything.
    pub fn parse(text: &str) -> Option<EpisodeSpec> {
        let mut ranges = Vec::new();
        for part in text.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
            ranges.push(parse_range(part)?);
        }
        Some(EpisodeSpec { ranges, text: text.trim().to_owned() })
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// The field as typed.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether `ep` falls in (overlaps) one of the ranges. A title without an episode matches
    /// only an empty spec.
    pub fn matches(&self, ep: Option<Episode>) -> bool {
        if self.ranges.is_empty() {
            return true;
        }
        let Some(ep) = ep else { return false };
        self.ranges.iter().any(|r| match (*r, ep) {
            (Range::Seasoned(lo, hi), Episode::Numbered { season, first, last }) => {
                season != 0 && (season, last) >= lo && (season, first) <= hi
            }
            (Range::Episodes(lo, hi), Episode::Numbered { first, last, .. }) => last >= lo && first <= hi,
            (Range::Dates(lo, hi), Episode::Dated(d)) => (lo..=hi).contains(&d),
            _ => false,
        })
    }
}

// --- Qualities --------------------------------------------------------------------------------

/// A set of release qualities as a bit mask (the filter's "Quality" list). The first sixteen
/// are µTorrent 2.2.1's list as remembered (HDTV through SatRip; its bit values are unknown, so
/// these are PaTorrent's); the rest are later additions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Quality(pub u32);

impl Quality {
    pub const ALL: Quality = Quality(0);
    pub const HDTV: Quality = Quality(1);
    pub const TVRIP: Quality = Quality(1 << 1);
    pub const DVDRIP: Quality = Quality(1 << 2);
    pub const SVCD: Quality = Quality(1 << 3);
    pub const DSRIP: Quality = Quality(1 << 4);
    pub const DVBRIP: Quality = Quality(1 << 5);
    pub const PDTV: Quality = Quality(1 << 6);
    pub const HR_HDTV: Quality = Quality(1 << 7);
    pub const HR_PDTV: Quality = Quality(1 << 8);
    pub const DVDR: Quality = Quality(1 << 9);
    pub const DVDSCR: Quality = Quality(1 << 10);
    pub const P720: Quality = Quality(1 << 11);
    pub const I1080: Quality = Quality(1 << 12);
    pub const P1080: Quality = Quality(1 << 13);
    pub const WEBRIP: Quality = Quality(1 << 14);
    pub const SATRIP: Quality = Quality(1 << 15);
    pub const WEBDL: Quality = Quality(1 << 16);
    pub const BLURAY: Quality = Quality(1 << 17);
    pub const P480: Quality = Quality(1 << 18);
    pub const P2160: Quality = Quality(1 << 19);

    /// Every quality with its display name, in the dialog's order.
    pub const LIST: [(Quality, &'static str); 20] = [
        (Quality::HDTV, "HDTV"),
        (Quality::TVRIP, "TVRip"),
        (Quality::DVDRIP, "DVDRip"),
        (Quality::SVCD, "SVCD"),
        (Quality::DSRIP, "DSRip"),
        (Quality::DVBRIP, "DVBRip"),
        (Quality::PDTV, "PDTV"),
        (Quality::HR_HDTV, "HR.HDTV"),
        (Quality::HR_PDTV, "HR.PDTV"),
        (Quality::DVDR, "DVDR"),
        (Quality::DVDSCR, "DVDScr"),
        (Quality::P720, "720p"),
        (Quality::I1080, "1080i"),
        (Quality::P1080, "1080p"),
        (Quality::WEBRIP, "WebRip"),
        (Quality::SATRIP, "SatRip"),
        (Quality::WEBDL, "WEB-DL"),
        (Quality::BLURAY, "BluRay"),
        (Quality::P480, "480p"),
        (Quality::P2160, "2160p"),
    ];

    pub fn is_all(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, q: Quality) -> bool {
        self.0 & q.0 == q.0
    }

    pub fn intersects(self, q: Quality) -> bool {
        self.0 & q.0 != 0
    }

    pub fn union(self, q: Quality) -> Quality {
        Quality(self.0 | q.0)
    }

    /// The qualities a release title names. Tokens are matched whole, ignoring case, with
    /// `.`, `_`, `-`, spaces and brackets as separators (`HR.HDTV` and `WEB-DL` span one).
    pub fn of_title(title: &str) -> Quality {
        let mut norm = String::with_capacity(title.len() + 2);
        norm.push('.');
        for c in title.chars() {
            norm.push(if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '.' });
        }
        norm.push('.');
        let has = |alias: &str| norm.contains(&format!(".{alias}."));
        let aliases: [(Quality, &[&str]); 20] = [
            (Quality::HDTV, &["hdtv"]),
            (Quality::TVRIP, &["tvrip"]),
            (Quality::DVDRIP, &["dvdrip"]),
            (Quality::SVCD, &["svcd"]),
            (Quality::DSRIP, &["dsrip", "dsr"]),
            (Quality::DVBRIP, &["dvbrip"]),
            (Quality::PDTV, &["pdtv"]),
            (Quality::HR_HDTV, &["hr.hdtv"]),
            (Quality::HR_PDTV, &["hr.pdtv"]),
            (Quality::DVDR, &["dvdr", "dvd.r", "dvd5", "dvd9"]),
            (Quality::DVDSCR, &["dvdscr", "dvdscreener"]),
            (Quality::P720, &["720p"]),
            (Quality::I1080, &["1080i"]),
            (Quality::P1080, &["1080p"]),
            (Quality::WEBRIP, &["webrip", "web.rip"]),
            (Quality::SATRIP, &["satrip"]),
            (Quality::WEBDL, &["web.dl", "webdl"]),
            (Quality::BLURAY, &["bluray", "blu.ray", "bdrip", "brrip"]),
            (Quality::P480, &["480p"]),
            (Quality::P2160, &["2160p", "4k", "uhd"]),
        ];
        let mut q = Quality::ALL;
        for (flag, names) in aliases {
            if names.iter().any(|a| has(a)) {
                q = q.union(flag);
            }
        }
        // "HR.HDTV" also contains the token "hdtv": count plain HDTV only when it appears alone
        for (hr, plain, name) in
            [(Quality::HR_HDTV, Quality::HDTV, "hdtv"), (Quality::HR_PDTV, Quality::PDTV, "pdtv")]
        {
            if q.contains(hr) && norm.matches(&format!(".{name}.")).count() == 1 {
                q = Quality(q.0 & !plain.0);
            }
        }
        q
    }

    /// The names of the qualities in the set, `|`-separated (for display).
    pub fn names(self) -> String {
        Quality::LIST.iter().filter(|(q, _)| self.contains(*q)).map(|(_, n)| *n).collect::<Vec<_>>().join("|")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(season: u32, first: u32, last: u32) -> Option<Episode> {
        Some(Episode::Numbered { season, first, last })
    }

    #[test]
    fn episodes_from_titles() {
        let cases: &[(&str, Option<Episode>)] = &[
            ("Show.Name.S01E02.720p.HDTV.x264-GRP", n(1, 2, 2)),
            ("Show Name s1e2 hdtv", n(1, 2, 2)),
            ("Show.Name.S01.E02.HDTV", n(1, 2, 2)),
            ("Show.Name.S01E02E03.HDTV", n(1, 2, 3)),
            ("Show.Name.S01E02-E03.HDTV", n(1, 2, 3)),
            ("Show.Name.S01E02-03.HDTV", n(1, 2, 3)),
            ("Show.Name.S10E100.HDTV", n(10, 100, 100)),
            ("Show.Name.S01E02v2", n(1, 2, 2)),
            ("Show_Name_1x02_HDTV", n(1, 2, 2)),
            ("Show Name 01x02-03", n(1, 2, 3)),
            ("Show Name 1920x1080 sample", None),
            ("Show.Name.2026.10.19.Guest.HDTV", Some(Episode::Dated(20261019))),
            ("Show Name 2026-10-19 720p", Some(Episode::Dated(20261019))),
            ("Show Name 2026-10.19 mixed", None),
            ("Show Name 2026.13.19", None),
            ("[Group] Show - 12 [1080p].mkv", n(0, 12, 12)),
            ("[Group] Show - 12v2 (BD 1080p)", n(0, 12, 12)),
            ("[Group] Show - 01-02 [720p]", n(0, 1, 2)),
            ("[Group] Show S2 - 14 (TV 1080p)", n(2, 14, 14)),
            ("[Group] Show Ss2 - 14", n(0, 14, 14)),
            ("Show - 2026 [Documentary]", None),
            ("Show Episode 7 HDTV", n(0, 7, 7)),
            ("Show.Ep.07.HDTV", n(0, 7, 7)),
            ("Show E07 HDTV", n(0, 7, 7)),
            ("Show.Name.S01.Complete.720p", None),
            ("Linux Distro 24.04 amd64", None),
            ("Seasons.and.Specials", None),
            ("", None),
        ];
        for (title, want) in cases {
            assert_eq!(parse_episode(title), *want, "{title}");
        }
    }

    #[test]
    fn repacks() {
        assert!(is_repack("Show.S01E02.REPACK.720p"));
        assert!(is_repack("Show S01E02 PROPER"));
        assert!(!is_repack("Show.S01E02.Repackaged"));
    }

    #[test]
    fn episode_keys() {
        assert_eq!(n(1, 2, 3).unwrap().keys(), ["s1e2", "s1e3"]);
        assert_eq!(Episode::Dated(20261019).keys(), ["d20261019"]);
    }

    #[test]
    fn spec_parsing() {
        assert_eq!(EpisodeSpec::parse("").unwrap(), EpisodeSpec::default());
        assert!(EpisodeSpec::parse(" ; , ").unwrap().is_empty());
        for bad in ["abc", "S01E", "1-x", "S01E01-2026.10.19", "1x", "5-S01"] {
            assert_eq!(EpisodeSpec::parse(bad), None, "{bad}");
        }
        let s = EpisodeSpec::parse(" S01E01-S01E10 ; 1x12-14, S03, 2026.10.01-2026.10.31 ").unwrap();
        assert_eq!(s.text(), "S01E01-S01E10 ; 1x12-14, S03, 2026.10.01-2026.10.31");
        assert_eq!(s.ranges.len(), 4);
    }

    #[test]
    fn spec_matching() {
        type Cases<'a> = &'a [(Option<Episode>, bool)];
        let table: &[(&str, Cases<'_>)] = &[
            ("", &[(None, true), (n(1, 1, 1), true)]),
            (
                "S01E01-S01E10",
                &[
                    (n(1, 1, 1), true),
                    (n(1, 10, 10), true),
                    (n(1, 11, 11), false),
                    (n(2, 1, 1), false),
                    (n(0, 5, 5), false),
                    (None, false),
                ],
            ),
            ("S01E01-E10", &[(n(1, 10, 10), true), (n(1, 11, 11), false)]),
            ("S01E01-10", &[(n(1, 10, 10), true), (n(2, 5, 5), false)]),
            ("1x1-12", &[(n(1, 12, 12), true), (n(1, 13, 13), false)]),
            (
                "S01E10-S02E03",
                &[(n(1, 9, 9), false), (n(1, 20, 20), true), (n(2, 3, 3), true), (n(2, 4, 4), false)],
            ),
            ("S02", &[(n(2, 1, 1), true), (n(2, 99, 99), true), (n(1, 99, 99), false), (n(3, 1, 1), false)]),
            ("S01-S03", &[(n(1, 1, 1), true), (n(3, 40, 40), true), (n(4, 1, 1), false)]),
            ("S02E05-", &[(n(2, 4, 4), false), (n(2, 5, 5), true), (n(9, 1, 1), true)]),
            ("1-12", &[(n(0, 12, 12), true), (n(3, 1, 1), true), (n(0, 13, 13), false)]),
            ("100-", &[(n(0, 99, 99), false), (n(0, 1000, 1000), true)]),
            ("5", &[(n(0, 5, 5), true), (n(0, 4, 6), true), (n(0, 6, 6), false)]),
            ("S01E05", &[(n(1, 4, 5), true), (n(1, 6, 7), false)]),
            (
                "2026.10.01-2026.10.31",
                &[
                    (Some(Episode::Dated(20261019)), true),
                    (Some(Episode::Dated(20261101)), false),
                    (n(1, 1, 1), false),
                ],
            ),
            (
                "2026-10-19",
                &[(Some(Episode::Dated(20261019)), true), (Some(Episode::Dated(20261018)), false)],
            ),
            (
                "2026-10-19-",
                &[(Some(Episode::Dated(20271231)), true), (Some(Episode::Dated(20261018)), false)],
            ),
            ("S01E01-E03, S02", &[(n(1, 3, 3), true), (n(1, 4, 4), false), (n(2, 8, 8), true)]),
        ];
        for (spec, cases) in table {
            let s = EpisodeSpec::parse(spec).unwrap_or_else(|| panic!("{spec}"));
            for (ep, want) in *cases {
                assert_eq!(s.matches(*ep), *want, "{spec} vs {ep:?}");
            }
        }
    }

    #[test]
    fn qualities() {
        let q = |t| Quality::of_title(t);
        assert_eq!(q("Show.S01E02.720p.HDTV.x264"), Quality::P720.union(Quality::HDTV));
        assert_eq!(q("Show.S01E02.HR.HDTV.XviD"), Quality::HR_HDTV);
        assert_eq!(q("Show S01E02 HR HDTV and HDTV"), Quality::HR_HDTV.union(Quality::HDTV));
        assert_eq!(q("Movie.2026.1080p.WEB-DL.DDP5.1"), Quality::P1080.union(Quality::WEBDL));
        assert_eq!(q("Movie [BluRay 2160p]"), Quality::BLURAY.union(Quality::P2160));
        assert_eq!(q("Show.DVDRip.XviD"), Quality::DVDRIP);
        assert_eq!(q("Show.PDTV"), Quality::PDTV);
        assert_eq!(q("Show.HDTVRip"), Quality::ALL, "whole tokens only");
        assert_eq!(q("Show 720P webrip"), Quality::P720.union(Quality::WEBRIP));
        assert_eq!(q(""), Quality::ALL);
        assert_eq!(Quality::P720.union(Quality::HDTV).names(), "HDTV|720p");
        assert!(Quality::ALL.is_all());
        assert_eq!(Quality::LIST.len(), 20);
        assert_eq!(Quality::LIST.iter().fold(0, |m, (q, _)| m | q.0), (1 << 20) - 1);
    }
}
