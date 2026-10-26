//! RSS and Atom feed parsing on top of the engine's forgiving XML scanner
//! ([`patorrent_core::xml`], libtorrent's `xml_parse`).
//!
//! Written from the public formats: RSS 2.0 (`channel` / `item`: `title`, `link`, `guid`,
//! `pubDate`, `description`, `enclosure url= type= length=`), RSS 1.0 (`item` beside the
//! channel, `dc:date`), Atom (RFC 4287: `entry`, `id`, `link href= rel=`, `published` /
//! `updated`, `summary` / `content`) and the torrent feed conventions: an enclosure of type
//! `application/x-bittorrent`, and `magnetURI`, `infoHash`, `contentLength` and `fileName`
//! elements in a `torrent:` (or any other) namespace.
//!
//! Junk never fails the parse: what is not understood is skipped, text is size-capped, and a
//! document without a single feed element is `None`. Dates become unix time
//! ([`parse_date`]: RFC 822 / 2822 and RFC 3339).

use patorrent_core::xml::{self, Token};

/// Longest text kept for a title, link or other short field; descriptions get [`MAX_DESCRIPTION`].
const MAX_FIELD: usize = 8 * 1024;
const MAX_DESCRIPTION: usize = 64 * 1024;
/// Items kept from one document.
pub const MAX_ITEMS: usize = 5000;
/// Elements nested deeper than this are skipped.
const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FeedKind {
    #[default]
    Rss,
    Atom,
}

/// A parsed feed document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Feed {
    pub kind: FeedKind,
    /// The channel's (or Atom feed's) title, for a feed without an alias.
    pub title: String,
    pub link: String,
    /// In document order (feeds usually list the newest first).
    pub items: Vec<FeedItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Enclosure {
    pub url: String,
    /// The MIME type, `application/x-bittorrent` for torrent feeds.
    pub mime: String,
    pub length: Option<u64>,
}

/// One `item` (RSS) or `entry` (Atom). Text is entity-decoded and trimmed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedItem {
    pub title: String,
    pub link: String,
    /// `guid` (RSS) or `id` (Atom).
    pub guid: String,
    /// `pubDate`, `published`, `updated` or `dc:date`, as unix time.
    pub published: Option<i64>,
    /// `description`, `summary` or `content`. May hold HTML.
    pub description: String,
    pub enclosure: Option<Enclosure>,
    /// `torrent:magnetURI`.
    pub magnet: String,
    /// `torrent:infoHash` (hex or base32, as published).
    pub info_hash: String,
    /// `torrent:contentLength`.
    pub content_length: Option<u64>,
    /// `torrent:fileName`.
    pub file_name: String,
}

impl FeedItem {
    /// What identifies the item across refreshes: its guid, else its link, else its enclosure,
    /// else its title.
    pub fn id(&self) -> &str {
        [&self.guid, &self.link, self.enclosure.as_ref().map_or(&self.magnet, |e| &e.url), &self.title]
            .into_iter()
            .find(|s| !s.is_empty())
            .map_or("", String::as_str)
    }

    /// What to add: a torrent enclosure, else the magnet link, else any other enclosure, else
    /// the item's link (µTorrent took the enclosure when there was one and the link otherwise),
    /// else a magnet link made from the info-hash. `None` when the item names nothing.
    pub fn torrent_url(&self) -> Option<String> {
        let enc = self.enclosure.as_ref().filter(|e| !e.url.is_empty());
        if let Some(e) = enc.filter(|e| e.mime.eq_ignore_ascii_case("application/x-bittorrent")) {
            return Some(e.url.clone());
        }
        if !self.magnet.is_empty() {
            return Some(self.magnet.clone());
        }
        if let Some(e) = enc {
            return Some(e.url.clone());
        }
        if !self.link.is_empty() {
            return Some(self.link.clone());
        }
        let h = self.info_hash.trim();
        let hex = h.len() == 40 && h.bytes().all(|c| c.is_ascii_hexdigit());
        let b32 = h.len() == 32 && h.bytes().all(|c| c.is_ascii_alphabetic() || (b'2'..=b'7').contains(&c));
        (hex || b32).then(|| {
            let mut m = format!("magnet:?xt=urn:btih:{h}");
            if !self.title.is_empty() {
                m.push_str("&dn=");
                m.push_str(&patorrent_core::util::string::escape_string(self.title.as_bytes()));
            }
            m
        })
    }
}

/// The item fields we read, by element name (namespace prefix aside).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Title,
    Link,
    Guid,
    /// `pubDate`, `published`, `dc:date`.
    Date,
    /// Atom `updated`: the date of an entry without `published`.
    Updated,
    Description,
    Magnet,
    InfoHash,
    ContentLength,
    FileName,
}

/// Which field an element of an item fills. Core RSS / Atom names count without a prefix (or
/// with `atom:`); the torrent elements with any prefix (`torrent:`, `nyaa:`, ...).
fn item_field(name: &str) -> Option<Field> {
    let (prefix, local) = name.rsplit_once(':').unwrap_or(("", name));
    let core = prefix.is_empty() || prefix.eq_ignore_ascii_case("atom");
    let is = |n: &str| local.eq_ignore_ascii_case(n);
    Some(match () {
        _ if core && is("title") => Field::Title,
        _ if core && is("link") => Field::Link,
        _ if core && (is("guid") || is("id")) => Field::Guid,
        _ if core && (is("pubDate") || is("published")) => Field::Date,
        _ if core && is("updated") => Field::Updated,
        _ if prefix.eq_ignore_ascii_case("dc") && is("date") => Field::Date,
        _ if core && (is("description") || is("summary") || is("content")) => Field::Description,
        _ if is("magnetURI") => Field::Magnet,
        _ if is("infoHash") => Field::InfoHash,
        _ if is("contentLength") => Field::ContentLength,
        _ if is("fileName") => Field::FileName,
        _ => return None,
    })
}

fn local_name(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

/// Parser state between tokens.
#[derive(Default)]
struct State {
    feed: Feed,
    found: bool,
    /// Open elements (raw names).
    stack: Vec<String>,
    /// The item being read and the depth of its element.
    item: Option<(FeedItem, usize)>,
    /// The field being read: which, its element's depth, the text so far.
    field: Option<(Field, usize, String)>,
    /// Channel-level `title` / `link` being read (depth, is_title, text).
    channel_field: Option<(usize, bool, String)>,
    /// Attributes of the tag just opened, for `enclosure` and Atom `link`.
    tag: Option<(String, Vec<(String, String)>)>,
    /// The `updated` date of an Atom entry, used when it has no `published`.
    updated: Option<i64>,
}

impl State {
    fn start(&mut self, name: &str, empty: bool) {
        let local = local_name(name);
        if local.eq_ignore_ascii_case("rss") || local.eq_ignore_ascii_case("RDF") {
            self.found = true;
        } else if local.eq_ignore_ascii_case("feed") && !name.contains(':') {
            self.found = true;
            self.feed.kind = FeedKind::Atom;
        } else if local.eq_ignore_ascii_case("channel") {
            self.found = true;
        }
        let depth = self.stack.len();
        let is_item = (local.eq_ignore_ascii_case("item") || local.eq_ignore_ascii_case("entry"))
            && !name.contains(':');
        if is_item && self.item.is_none() && !empty {
            self.found = true;
            self.item = Some((FeedItem::default(), depth));
            self.updated = None;
        } else if let Some((_, item_depth)) = &self.item {
            if depth == item_depth + 1 && self.field.is_none() && !empty {
                if let Some(f) = item_field(name) {
                    self.field = Some((f, depth, String::new()));
                }
            }
        } else if self.found && self.channel_field.is_none() && !empty && depth <= 3 {
            // the channel's own title and link: `rss > channel > title` or `feed > title`
            let parent = self.stack.last().map(|p| local_name(p).to_ascii_lowercase());
            let channel = matches!(parent.as_deref(), Some("channel" | "feed"));
            if channel && !name.contains(':') {
                if local.eq_ignore_ascii_case("title") && self.feed.title.is_empty() {
                    self.channel_field = Some((depth, true, String::new()));
                } else if local.eq_ignore_ascii_case("link") && self.feed.link.is_empty() {
                    self.channel_field = Some((depth, false, String::new()));
                }
            }
        }
        if !empty {
            self.stack.push(name.to_owned());
        }
    }

    fn text(&mut self, s: &str, cdata: bool) {
        let max = match &self.field {
            Some((Field::Description, ..)) => MAX_DESCRIPTION,
            _ => MAX_FIELD,
        };
        let buf = match (&mut self.field, &mut self.channel_field) {
            (Some((.., buf)), _) => buf,
            (None, Some((.., buf))) => buf,
            (None, None) => return,
        };
        if buf.len() >= max {
            return;
        }
        if cdata {
            buf.push_str(s);
        } else {
            decode_entities_into(s, buf);
        }
        if buf.len() > max {
            let mut cut = max;
            while !buf.is_char_boundary(cut) {
                cut -= 1;
            }
            buf.truncate(cut);
        }
    }

    fn end(&mut self, name: &str) {
        // a stray end tag closes nothing; a missing one is implied by its parent's
        let Some(pos) = self.stack.iter().rposition(|n| n.eq_ignore_ascii_case(name)) else { return };
        self.stack.truncate(pos);
        let depth = pos;
        if self.field.as_ref().is_some_and(|(_, d, _)| *d >= depth) {
            let (field, _, text) = self.field.take().expect("checked");
            if let Some((item, _)) = &mut self.item {
                self.updated = set_field(item, field, text.trim(), self.updated);
            }
        }
        if self.channel_field.as_ref().is_some_and(|(d, ..)| *d >= depth) {
            let (_, title, text) = self.channel_field.take().expect("checked");
            let text = text.trim().to_owned();
            if title { self.feed.title = text } else { self.feed.link = text }
        }
        if self.item.as_ref().is_some_and(|(_, d)| *d >= depth) {
            let (mut item, _) = self.item.take().expect("checked");
            if item.published.is_none() {
                item.published = self.updated;
            }
            if self.feed.items.len() < MAX_ITEMS && item != FeedItem::default() {
                self.feed.items.push(item);
            }
        }
    }

    /// `enclosure` (RSS) and `link` (Atom) carry their data in attributes.
    fn tag_done(&mut self) {
        let Some((name, attrs)) = self.tag.take() else { return };
        let Some((item, item_depth)) = &mut self.item else { return };
        // the tag was pushed already unless it was empty
        let depth = if self.stack.last() == Some(&name) { self.stack.len() - 1 } else { self.stack.len() };
        if depth != *item_depth + 1 {
            return;
        }
        let attr = |k: &str| attrs.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
        let local = local_name(&name);
        let core = !name.contains(':') || name.to_ascii_lowercase().starts_with("atom:");
        if core && local.eq_ignore_ascii_case("enclosure") {
            if let Some(url) = attr("url").filter(|u| !u.is_empty()) {
                set_enclosure(item, url, attr("type"), attr("length"));
            }
        } else if core && local.eq_ignore_ascii_case("link") {
            let Some(href) = attr("href").filter(|h| !h.is_empty()) else { return };
            match attr("rel").unwrap_or("alternate") {
                r if r.eq_ignore_ascii_case("enclosure") => {
                    set_enclosure(item, href, attr("type"), attr("length"))
                }
                r if r.eq_ignore_ascii_case("alternate") && item.link.is_empty() => {
                    item.link = clip(href.trim(), MAX_FIELD)
                }
                _ => {}
            }
            // an Atom link is empty; the text of an RSS `<link>` would be read as the field
            if self.field.as_ref().is_some_and(|(f, ..)| *f == Field::Link) {
                self.field = None;
            }
        }
    }
}

/// Keeps the first torrent enclosure, or the first of any type until one comes.
fn set_enclosure(item: &mut FeedItem, url: &str, mime: Option<&str>, length: Option<&str>) {
    let torrent = |m: &str| m.eq_ignore_ascii_case("application/x-bittorrent");
    let mime = mime.unwrap_or("").trim();
    if item.enclosure.as_ref().is_some_and(|e| torrent(&e.mime) || !torrent(mime)) {
        return;
    }
    item.enclosure = Some(Enclosure {
        url: clip(url.trim(), MAX_FIELD),
        mime: clip(mime, 256),
        length: length.and_then(|l| l.trim().parse().ok()),
    });
}

fn clip(s: &str, max: usize) -> String {
    let mut cut = s.len().min(max);
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s[..cut].to_owned()
}

/// Stores a finished field; returns the entry's `updated` date (kept apart: Atom entries have
/// both, and `published` wins).
fn set_field(item: &mut FeedItem, field: Field, text: &str, updated: Option<i64>) -> Option<i64> {
    let first = |slot: &mut String| {
        if slot.is_empty() {
            *slot = text.to_owned();
        }
    };
    match field {
        Field::Title => first(&mut item.title),
        Field::Link => first(&mut item.link),
        Field::Guid => first(&mut item.guid),
        Field::Date => {
            if item.published.is_none() {
                item.published = parse_date(text);
            }
        }
        Field::Updated => return updated.or_else(|| parse_date(text)),
        Field::Description => first(&mut item.description),
        Field::Magnet => first(&mut item.magnet),
        Field::InfoHash => first(&mut item.info_hash),
        Field::ContentLength => {
            if item.content_length.is_none() {
                item.content_length = text.parse().ok();
            }
        }
        Field::FileName => first(&mut item.file_name),
    }
    updated
}

/// Parses an RSS or Atom document. `None` when nothing in it looks like a feed.
pub fn parse_feed(data: &[u8]) -> Option<Feed> {
    let text = decode_text(data);
    let input = text.as_str();
    let mut st = State::default();
    xml::parse(input, |t| {
        if !matches!(t, Token::Attribute(..) | Token::TagContent(_) | Token::ParseError(_)) {
            st.tag_done();
        }
        match t {
            Token::StartTag(name) if st.stack.len() < MAX_DEPTH => {
                st.start(name, false);
                st.tag = Some((name.to_owned(), Vec::new()));
            }
            Token::StartTag(name) => st.stack.push(name.to_owned()),
            Token::EmptyTag(name) => {
                st.start(name, true);
                st.tag = Some((name.to_owned(), Vec::new()));
            }
            Token::Attribute(k, v) => {
                if let Some((_, attrs)) = &mut st.tag {
                    if attrs.len() < 32 {
                        let mut val = String::new();
                        decode_entities_into(v, &mut val);
                        attrs.push((k.to_owned(), val));
                    }
                }
            }
            Token::EndTag(name) => st.end(name),
            Token::String(s) => {
                let cdata = is_cdata(input, s);
                st.text(s, cdata);
            }
            _ => {}
        }
    });
    st.tag_done();
    // an unterminated last item still counts
    if let Some((_, depth)) = st.item {
        if let Some(name) = st.stack.get(depth).cloned() {
            st.end(&name);
        }
    }
    st.found.then_some(st.feed)
}

/// The scanner reports CDATA contents as plain strings; they are slices of the input, so the
/// bytes before one tell whether it was `<![CDATA[`.
fn is_cdata(input: &str, s: &str) -> bool {
    let off = (s.as_ptr() as usize).wrapping_sub(input.as_ptr() as usize);
    off <= input.len() && off >= 9 && input.as_bytes()[off - 9..off].eq_ignore_ascii_case(b"<![CDATA[")
}

/// The document as text: UTF-8 (a BOM dropped), or ISO-8859-1 / Windows-1252 when the XML
/// declaration says so and the bytes are not UTF-8. Anything else is decoded lossily.
fn decode_text(data: &[u8]) -> String {
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
    if let Ok(s) = std::str::from_utf8(data) {
        return s.to_owned();
    }
    let head = String::from_utf8_lossy(&data[..data.len().min(200)]).to_ascii_lowercase();
    let latin1 = head.starts_with("<?xml")
        && ["iso-8859-1", "latin1", "windows-1252", "iso-8859-15"].iter().any(|e| head.contains(e));
    if latin1 {
        // windows-1252's 0x80-0x9f extras are kept as the C1 controls ISO-8859-1 has there
        data.iter().map(|&b| b as char).collect()
    } else {
        String::from_utf8_lossy(data).into_owned()
    }
}

/// Decodes XML character references and the five predefined entities, plus `&nbsp;`. Anything
/// else that starts with `&` is kept as is (feeds are full of unescaped ampersands).
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    decode_entities_into(s, &mut out);
    out
}

fn decode_entities_into(s: &str, out: &mut String) {
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest[1..].find(';').filter(|&semi| semi <= 10).and_then(|semi| {
            let name = &rest[1..1 + semi];
            let c = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => '\u{a0}',
                _ => {
                    let num = name.strip_prefix('#')?;
                    let cp = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(cp).filter(|&c| c != '\0')?
                }
            };
            Some((c, semi + 2))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
}

// --- Dates ------------------------------------------------------------------------------------

/// Parses an RFC 822 / 2822 date (`Mon, 19 Oct 2026 06:35:37 +0000`, RSS) or an RFC 3339 one
/// (`2026-10-19T06:35:37Z`, Atom) into unix time. Lenient: the weekday is ignored, seconds are
/// optional, two-digit years are 19xx / 20xx, unknown zone names count as UTC.
pub fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() > 64 {
        return None;
    }
    if s.as_bytes().first().is_some_and(u8::is_ascii_digit) && s.len() >= 10 && s.as_bytes()[4] == b'-' {
        parse_rfc3339(s)
    } else {
        parse_rfc822(s)
    }
}

fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let t = s.get(from..from + len)?;
        t.bytes().all(|c| c.is_ascii_digit()).then(|| t.parse().ok())?
    };
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    if b.get(7) != Some(&b'-') {
        return None;
    }
    let mut t = 0;
    let mut zone = 0;
    if b.len() > 10 {
        if !matches!(b[10], b'T' | b't' | b' ') {
            return None;
        }
        let (h, mi) = (num(11, 2)?, num(14, 2)?);
        if b.get(13) != Some(&b':') {
            return None;
        }
        let mut p = 16;
        let mut sec = 0;
        if b.get(p) == Some(&b':') {
            sec = num(p + 1, 2)?;
            p += 3;
            if b.get(p) == Some(&b'.') {
                p += 1;
                while b.get(p).is_some_and(u8::is_ascii_digit) {
                    p += 1;
                }
            }
        }
        zone = match &s[p..] {
            "" | "Z" | "z" => 0,
            z => zone_offset(z)?,
        };
        if h > 23 || mi > 59 || sec > 60 {
            return None;
        }
        t = h * 3600 + mi * 60 + sec;
    }
    Some(days_from_civil(y, mo, d)? * 86400 + t - zone)
}

/// `+hhmm`, `-hh:mm`, `GMT`, `EST` and friends; seconds east of UTC.
fn zone_offset(z: &str) -> Option<i64> {
    let z = z.trim();
    if let Some(sign) = z.strip_prefix('+').map(|r| (1, r)).or_else(|| z.strip_prefix('-').map(|r| (-1, r))) {
        let digits: String = sign.1.chars().filter(|&c| c != ':').collect();
        if digits.len() != 4 || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let h: i64 = digits[..2].parse().ok()?;
        let m: i64 = digits[2..].parse().ok()?;
        return Some(sign.0 * (h * 3600 + m * 60));
    }
    let hours = match z.to_ascii_uppercase().as_str() {
        "EST" => -5,
        "EDT" => -4,
        "CST" => -6,
        "CDT" => -5,
        "MST" => -7,
        "MDT" => -6,
        "PST" => -8,
        "PDT" => -7,
        // GMT, UT, UTC, Z, military letters and whatever else: UTC (RFC 2822 says to treat
        // unknown zones as -0000)
        _ => 0,
    };
    Some(hours * 3600)
}

fn parse_rfc822(s: &str) -> Option<i64> {
    let mut parts = s.split([' ', ',', '\t']).filter(|p| !p.is_empty()).peekable();
    // the optional weekday
    if parts.peek().is_some_and(|p| p.bytes().all(|c| c.is_ascii_alphabetic())) {
        parts.next();
    }
    // unsigned: a signed field (`12:-5:00`) is not a time, and a negative minute multiplied
    // below overflowed (fuzz target rss, 2026-10-26)
    let d = i64::from(parts.next()?.parse::<u32>().ok()?);
    let mon = parts.next()?;
    let mo = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
        .iter()
        .position(|m| mon.get(..3).is_some_and(|p| p.eq_ignore_ascii_case(m)))? as i64
        + 1;
    let ys = parts.next()?;
    let mut y = i64::from(ys.parse::<u32>().ok()?);
    if ys.len() <= 2 {
        y += if y < 50 { 2000 } else { 1900 };
    }
    let mut t = 0;
    let mut zone = 0;
    if let Some(time) = parts.next() {
        let mut hms = time.split(':');
        let h = i64::from(hms.next()?.parse::<u8>().ok()?);
        let mi = i64::from(hms.next()?.parse::<u8>().ok()?);
        let sec = match hms.next() {
            Some(x) => i64::from(x.parse::<u8>().ok()?),
            None => 0,
        };
        if h > 23 || mi > 59 || sec > 60 {
            return None;
        }
        t = h * 3600 + mi * 60 + sec;
        if let Some(z) = parts.next() {
            zone = zone_offset(z)?;
        }
    }
    Some(days_from_civil(y, mo, d)? * 86400 + t - zone)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1..=9999).contains(&y) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torrent="http://xmlns.ezrss.it/0.1/" xmlns:media="http://search.yahoo.com/mrss/">
  <channel>
    <title>Example Torrents &amp; More</title>
    <link>https://feeds.example.org/</link>
    <description>Test feed</description>
    <image><title>not the channel title</title><url>https://feeds.example.org/logo.png</url></image>
    <item>
      <title>Show.Name.S02E05.720p.HDTV.x264-GRP</title>
      <link>https://feeds.example.org/details/1</link>
      <guid isPermaLink="false">item-0001</guid>
      <pubDate>Mon, 19 Oct 2026 06:35:37 +0000</pubDate>
      <description><![CDATA[<b>Size:</b> 1.2 GB &amp; more]]></description>
      <enclosure url="https://feeds.example.org/get.php?id=1&amp;key=abc" length="1234567" type="application/x-bittorrent" />
      <torrent:contentLength>1288490188</torrent:contentLength>
      <torrent:infoHash>0123456789abcdef0123456789abcdef01234567</torrent:infoHash>
      <torrent:magnetURI><![CDATA[magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Show]]></torrent:magnetURI>
      <torrent:fileName>Show.Name.S02E05.720p.HDTV.x264-GRP.torrent</torrent:fileName>
      <media:title>not the item title</media:title>
    </item>
    <item>
      <title>Other &lt;Thing&gt; &#8211; caf&#xE9;</title>
      <link>magnet:?xt=urn:btih:ABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCD</link>
      <pubDate>Tue, 1 Sep 26 23:59 EST</pubDate>
    </item>
    <item><title>Junk & unescaped</title><guid>junk</guid><pubDate>yesterday</pubDate></item>
  </channel>
</rss>"#;

    #[test]
    fn rss2_with_torrent_namespace() {
        let f = parse_feed(RSS.as_bytes()).unwrap();
        assert_eq!(f.kind, FeedKind::Rss);
        assert_eq!(f.title, "Example Torrents & More");
        assert_eq!(f.link, "https://feeds.example.org/");
        assert_eq!(f.items.len(), 3);
        let a = &f.items[0];
        assert_eq!(a.title, "Show.Name.S02E05.720p.HDTV.x264-GRP");
        assert_eq!(a.link, "https://feeds.example.org/details/1");
        assert_eq!(a.guid, "item-0001");
        assert_eq!(a.id(), "item-0001");
        assert_eq!(a.published, Some(days_from_civil(2026, 10, 19).unwrap() * 86400 + 23_737));
        assert_eq!(a.description, "<b>Size:</b> 1.2 GB &amp; more", "CDATA is verbatim");
        let e = a.enclosure.as_ref().unwrap();
        assert_eq!(e.url, "https://feeds.example.org/get.php?id=1&key=abc");
        assert_eq!((e.mime.as_str(), e.length), ("application/x-bittorrent", Some(1_234_567)));
        assert_eq!(a.content_length, Some(1_288_490_188));
        assert_eq!(a.info_hash, "0123456789abcdef0123456789abcdef01234567");
        assert!(a.magnet.starts_with("magnet:?xt=urn:btih:0123") && a.magnet.ends_with("&dn=Show"));
        assert_eq!(a.file_name, "Show.Name.S02E05.720p.HDTV.x264-GRP.torrent");
        assert_eq!(a.torrent_url().unwrap(), e.url, "the torrent enclosure wins");

        let b = &f.items[1];
        assert_eq!(b.title, "Other <Thing> \u{2013} café");
        assert_eq!(b.id(), b.link);
        assert_eq!(b.torrent_url().as_deref(), Some(b.link.as_str()));
        // 1 Sep 2026 23:59 EST = 2 Sep 2026 04:59 UTC
        assert_eq!(b.published, Some(days_from_civil(2026, 9, 2).unwrap() * 86400 + 4 * 3600 + 59 * 60));

        let c = &f.items[2];
        assert_eq!((c.title.as_str(), c.published), ("Junk & unescaped", None));
    }

    const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title type="text">Atom Example</title>
  <link href="https://atom.example.org/" rel="alternate"/>
  <id>urn:uuid:60a76c80-d399-11d9-b93C-0003939e0af6</id>
  <updated>2026-10-19T08:00:00Z</updated>
  <entry>
    <title type="html">Release 1.0 &amp;amp; notes</title>
    <link rel="alternate" type="text/html" href="https://atom.example.org/r/1"/>
    <link rel="enclosure" type="application/x-bittorrent" length="2048" href="https://atom.example.org/r/1.torrent"/>
    <id>urn:uuid:1225c695-cfb8-4ebb-aaaa-80da344efa6a</id>
    <updated>2026-10-19T08:00:00Z</updated>
    <published>2026-10-18T22:30:15.250+02:00</published>
    <summary>Short &lt;em&gt;summary&lt;/em&gt;</summary>
  </entry>
  <entry>
    <title>Only updated</title>
    <link href="https://atom.example.org/r/2"/>
    <id>tag:atom.example.org,2026:2</id>
    <updated>2026-10-19T08:00:00-05:30</updated>
    <content type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml">Some <b>bold</b> text</div></content>
  </entry>
</feed>"#;

    #[test]
    fn atom() {
        let f = parse_feed(ATOM.as_bytes()).unwrap();
        assert_eq!((f.kind, f.title.as_str()), (FeedKind::Atom, "Atom Example"));
        assert_eq!(f.items.len(), 2);
        let a = &f.items[0];
        assert_eq!(a.title, "Release 1.0 &amp; notes");
        assert_eq!(a.link, "https://atom.example.org/r/1");
        assert_eq!(a.guid, "urn:uuid:1225c695-cfb8-4ebb-aaaa-80da344efa6a");
        let e = a.enclosure.as_ref().unwrap();
        assert_eq!((e.url.as_str(), e.length), ("https://atom.example.org/r/1.torrent", Some(2048)));
        assert_eq!(a.torrent_url().unwrap(), e.url);
        // 2026-10-18T22:30:15+02:00 = 20:30:15 UTC
        assert_eq!(
            a.published,
            Some(days_from_civil(2026, 10, 18).unwrap() * 86400 + 20 * 3600 + 30 * 60 + 15)
        );
        assert_eq!(a.description, "Short <em>summary</em>");
        let b = &f.items[1];
        assert_eq!(b.link, "https://atom.example.org/r/2");
        assert_eq!(b.published, Some(days_from_civil(2026, 10, 19).unwrap() * 86400 + 13 * 3600 + 30 * 60));
        assert_eq!(b.description, "Some bold text");
    }

    #[test]
    fn rss1_and_namespaced_torrent_elements() {
        let doc = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/"
            xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:nyaa="https://example.org/ns">
          <channel rdf:about="x"><title>RDF</title></channel>
          <item rdf:about="https://r.example.org/1">
            <title>[Group] Show - 12 [1080p]</title>
            <link>https://r.example.org/1.torrent</link>
            <dc:date>2026-10-19</dc:date>
            <nyaa:infoHash>114aac2345b3a0a9dffdc4a5d0922c69200b45f6</nyaa:infoHash>
          </item>
        </rdf:RDF>"#;
        let f = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(f.title, "RDF");
        let a = &f.items[0];
        assert_eq!(a.published, Some(days_from_civil(2026, 10, 19).unwrap() * 86400));
        assert_eq!(a.info_hash, "114aac2345b3a0a9dffdc4a5d0922c69200b45f6");
        let only_hash =
            FeedItem { title: "a b".into(), info_hash: a.info_hash.clone(), ..FeedItem::default() };
        assert_eq!(
            only_hash.torrent_url().unwrap(),
            "magnet:?xt=urn:btih:114aac2345b3a0a9dffdc4a5d0922c69200b45f6&dn=a%20b"
        );
        assert_eq!(FeedItem::default().torrent_url(), None);
    }

    #[test]
    fn enclosure_choice() {
        let doc = r#"<rss><channel><item><title>t</title>
            <enclosure url="https://x/preview.jpg" type="image/jpeg"/>
            <enclosure url="https://x/t.torrent" type="application/x-bittorrent"></enclosure>
            <enclosure url="https://x/other.torrent" type="application/x-bittorrent"/>
            </item><item><title>u</title><enclosure url="https://x/u.bin"/>
            <torrent:magnetURI>magnet:?xt=urn:btih:abc</torrent:magnetURI></item></channel></rss>"#;
        let f = parse_feed(doc.as_bytes()).unwrap();
        assert_eq!(f.items[0].torrent_url().unwrap(), "https://x/t.torrent");
        assert_eq!(f.items[1].torrent_url().unwrap(), "magnet:?xt=urn:btih:abc");
        assert_eq!(f.items[1].id(), "https://x/u.bin");
    }

    #[test]
    fn junk_and_edges() {
        assert_eq!(parse_feed(b""), None);
        assert_eq!(parse_feed(b"<html><body>not a feed</body></html>"), None);
        assert_eq!(parse_feed(b"\x00\xff\xfe garbage <<<>>>"), None);
        // unterminated: the open item still counts
        let f = parse_feed(b"<rss><channel><item><title>cut off").unwrap();
        assert_eq!(f.items[0].title, "cut off");
        // mismatched and stray end tags
        let f =
            parse_feed(b"<rss><channel><item><title>a</b></title></i></item><item><title>b</title>").unwrap();
        assert_eq!(f.items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        // nesting beyond the limit is skipped, not a stack overflow
        let deep =
            format!("<rss><channel><item>{}<title>x</title></item></channel></rss>", "<a>".repeat(10_000));
        assert!(parse_feed(deep.as_bytes()).is_some());
        // huge titles are capped
        let big = format!("<rss><item><title>{}</title></item></rss>", "é".repeat(100_000));
        let f = parse_feed(big.as_bytes()).unwrap();
        assert!(f.items[0].title.len() <= MAX_FIELD);
        // BOM, and ISO-8859-1 when declared
        let f = parse_feed(b"\xEF\xBB\xBF<rss><item><title>bom</title></item></rss>").unwrap();
        assert_eq!(f.items[0].title, "bom");
        let f = parse_feed(
            b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><rss><item><title>caf\xe9</title></item></rss>",
        )
        .unwrap();
        assert_eq!(f.items[0].title, "café");
        // empty items are dropped
        assert!(parse_feed(b"<rss><item></item><item/></rss>").unwrap().items.is_empty());
    }

    #[test]
    fn entities() {
        assert_eq!(decode_entities("a &amp;amp; b"), "a &amp; b");
        assert_eq!(decode_entities("&#65;&#x42;&#X43;&nbsp;"), "ABC\u{a0}");
        assert_eq!(
            decode_entities("AT&T & co &unknown; &#0; &#xD800; &#99999999999;"),
            "AT&T & co &unknown; &#0; &#xD800; &#99999999999;"
        );
        assert_eq!(decode_entities("&"), "&");
        assert_eq!(decode_entities("tail &amp"), "tail &amp");
    }

    #[test]
    fn dates() {
        let day = |y, m, d| days_from_civil(y, m, d).unwrap() * 86400;
        assert_eq!(days_from_civil(1970, 1, 1), Some(0));
        assert_eq!(days_from_civil(2000, 3, 1), Some(11_017));
        let cases: &[(&str, Option<i64>)] = &[
            ("Mon, 19 Oct 2026 06:35:37 +0000", Some(day(2026, 10, 19) + 23_737)),
            ("19 Oct 2026 06:35:37 GMT", Some(day(2026, 10, 19) + 23_737)),
            ("Mon, 19 Oct 2026 06:35:37 -0000", Some(day(2026, 10, 19) + 23_737)),
            ("Mon,19 October 2026 06:35 +0130", Some(day(2026, 10, 19) + 6 * 3600 + 35 * 60 - 5400)),
            ("Sun, 18 Oct 26 22:00:00 PDT", Some(day(2026, 10, 19) + 5 * 3600)),
            ("Thu, 01 Jan 1970 00:00:00 Z", Some(0)),
            ("1 Jan 99 00:00:00 UT", Some(day(1999, 1, 1))),
            ("2026-10-19T06:35:37Z", Some(day(2026, 10, 19) + 23_737)),
            ("2026-10-19t06:35:37.123456+00:00", Some(day(2026, 10, 19) + 23_737)),
            ("2026-10-19 06:35:37-01:00", Some(day(2026, 10, 19) + 23_737 + 3600)),
            ("2026-10-19T06:35+0000", Some(day(2026, 10, 19) + 23_700)),
            ("2026-10-19", Some(day(2026, 10, 19))),
            ("2026-13-19", None),
            ("2026-10-19T25:00:00Z", None),
            ("2026-10-19X06:35:37Z", None),
            ("2026-10-19T06:35:37+5", None),
            ("Mon, 32 Oct 2026 06:35:37 +0000", None),
            ("Mon, 19 Foo 2026 06:35:37 +0000", None),
            ("Mon, 19 Oct 2026 06:xx:37", None),
            // signed fields: the minute's multiply overflowed (fuzz crash, 2026-10-26)
            ("080 sep 66 0:-002074284530519326355:9:-", None),
            ("1 Jan 2026 12:-5:00", None),
            ("-1 Jan 2026 12:00:00", None),
            ("1 Jan -2026 12:00:00", None),
            ("yesterday", None),
            ("", None),
            ("2026-1", None),
            ("1-1-1", None),
        ];
        for (s, want) in cases {
            assert_eq!(parse_date(s), *want, "{s}");
        }
    }
}
