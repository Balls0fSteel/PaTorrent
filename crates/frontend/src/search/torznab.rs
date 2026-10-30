//! The Torznab side of a search (D41): request URLs for a source, its answers turned into
//! [`SearchResult`]s, its `<error>` document and its capabilities' category list.
//!
//! Torznab is RSS 2.0 with `torznab:attr name= value=` elements (Prowlarr and Jackett both
//! speak it); the items are read with the RSS parser ([`crate::rss::feed`]). Everything in an
//! answer is untrusted text: shown as text, never as HTML.

use std::fmt;

use patorrent_core::metainfo::parse_magnet_uri;
use patorrent_core::util::string::{base32decode, escape_string};
use patorrent_core::xml::{self, Token};

use crate::rss::feed::{FeedItem, decode_entities, parse_feed};

/// Results asked of each source (`limit=`).
pub const LIMIT: u32 = 100;
/// Categories kept from a caps answer (subcategories included).
const MAX_CATEGORIES: usize = 2000;
/// Longest category name or error description kept.
const MAX_TEXT: usize = 1024;

/// One result of a search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResult {
    pub name: String,
    pub size: Option<u64>,
    pub seeders: Option<u32>,
    /// Seeders and leechers together (the Torznab `peers` attribute).
    pub peers: Option<u32>,
    /// The v1 info-hash in lowercase hex: the `infohash` attribute, else the magnet's `btih`;
    /// empty when neither names one.
    pub info_hash: String,
    /// Newznab category numbers (2000 movies, 5000 TV, 5040 TV/HD, ...).
    pub categories: Vec<u32>,
    /// Unix time.
    pub published: Option<i64>,
    /// The item's link: usually the indexer's own download URL (a `.torrent`, or a redirect to
    /// a magnet), proxied by Prowlarr or Jackett through themselves.
    pub link: String,
    /// The `magneturl` attribute (Jackett), the item's `torrent:magnetURI`, or the link when it
    /// is a magnet.
    pub magnet: String,
    /// The index of the source that answered, in the search's sources
    /// ([`super::worker::Search::sources`]); the one with the most seeders after merging.
    pub source: usize,
    /// Every source that returned this torrent (merging), `source` first.
    pub sources: Vec<usize>,
}

impl SearchResult {
    /// The result an item describes, from source `source`; `None` without a title or without
    /// anything to add (no link, magnet or info-hash).
    pub fn from_item(item: &FeedItem, source: usize) -> Option<SearchResult> {
        let attr =
            |n: &str| item.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.trim());
        let num = |n: &str| attr(n).and_then(|v| v.parse::<u32>().ok());
        let name = item.title.trim();
        if name.is_empty() {
            return None;
        }
        let is_magnet = |s: &str| s.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("magnet:"));
        let enclosure = item.enclosure.as_ref().map_or("", |e| e.url.trim());
        let mut link = if item.link.trim().is_empty() { enclosure } else { item.link.trim() }.to_owned();
        let magnet = [attr("magneturl").unwrap_or(""), item.magnet.trim(), link.as_str()]
            .into_iter()
            .find(|m| is_magnet(m))
            .unwrap_or("")
            .to_owned();
        if is_magnet(&link) {
            link.clear();
        }
        let info_hash = [attr("infohash").unwrap_or(""), item.info_hash.trim()]
            .into_iter()
            .find_map(hex_hash)
            .or_else(|| parse_magnet_uri(&magnet).ok()?.info_hashes.v1.map(|h| h.to_hex()))
            .unwrap_or_default();
        if link.is_empty() && magnet.is_empty() && info_hash.is_empty() {
            return None;
        }
        let seeders = num("seeders");
        let peers = num("peers").or_else(|| Some(seeders?.saturating_add(num("leechers")?)));
        let mut categories = Vec::new();
        for (k, v) in &item.attrs {
            if let (true, Ok(c)) = (k.eq_ignore_ascii_case("category"), v.trim().parse::<u32>()) {
                if !categories.contains(&c) {
                    categories.push(c);
                }
            }
        }
        let size = item
            .size
            .or_else(|| attr("size").and_then(|s| s.parse().ok()))
            .or(item.content_length)
            .or_else(|| item.enclosure.as_ref().and_then(|e| e.length))
            .filter(|&s| s > 0);
        Some(SearchResult {
            name: name.to_owned(),
            size,
            seeders,
            peers,
            info_hash,
            categories,
            published: item.published,
            link,
            magnet,
            source,
            sources: vec![source],
        })
    }

    /// What to add without fetching anything: the magnet, else one made from the info-hash.
    pub fn magnet_link(&self) -> Option<String> {
        if !self.magnet.is_empty() {
            return Some(self.magnet.clone());
        }
        (!self.info_hash.is_empty()).then(|| {
            format!("magnet:?xt=urn:btih:{}&dn={}", self.info_hash, escape_string(self.name.as_bytes()))
        })
    }
}

/// A v1 info-hash as published (40 hex digits or 32 base32 letters) in lowercase hex.
fn hex_hash(h: &str) -> Option<String> {
    let h = h.trim();
    if h.len() == 40 && h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Some(h.to_ascii_lowercase());
    }
    if h.len() == 32 {
        let bytes = base32decode(h.to_ascii_uppercase().as_bytes()).filter(|b| b.len() == 20)?;
        return Some(bytes.iter().map(|b| format!("{b:02x}")).collect());
    }
    None
}

/// Why a source gave no results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchError {
    /// The request failed (network, HTTP status, timeout); the text has the key removed
    /// ([`super::source::redact`]).
    Fetch(String),
    /// The source answered a Torznab `<error code= description=/>` (100: bad API key, 201:
    /// function not available, ...).
    Torznab { code: u32, description: String },
    /// The answer was neither a Torznab feed nor its error document. Holds the URL (key
    /// removed); empty when [`parse_results`] could not know it.
    NotTorznab(String),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchError::Fetch(e) => f.write_str(e),
            SearchError::Torznab { code, description } if description.is_empty() => write!(f, "{code}"),
            SearchError::Torznab { code, description } => write!(f, "{code}: {description}"),
            SearchError::NotTorznab(url) => f.write_str(&crate::tr!(RssErrNotFeed, url)),
        }
    }
}

impl std::error::Error for SearchError {}

/// A source's answer to a search: its results (source index 0, set by the caller), or its
/// error document. Items without a title or anything to add are skipped.
pub fn parse_results(data: &[u8]) -> Result<Vec<SearchResult>, SearchError> {
    if let Some((code, description)) = parse_error(data) {
        return Err(SearchError::Torznab { code, description });
    }
    let feed = parse_feed(data).ok_or_else(|| SearchError::NotTorznab(String::new()))?;
    Ok(feed.items.iter().filter_map(|i| SearchResult::from_item(i, 0)).collect())
}

/// The tags of a document with their attributes (entity-decoded), in order, with the depth
/// each opened at; `on_tag` sees each once its attributes are read.
fn scan_tags(data: &[u8], mut on_tag: impl FnMut(&str, usize, &[(String, String)])) {
    let text = String::from_utf8_lossy(data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data));
    let mut tag: Option<OpenTag> = None;
    let mut depth = 0usize;
    let mut flush = |tag: &mut Option<OpenTag>| {
        if let Some((name, d, attrs)) = tag.take() {
            on_tag(&name, d, &attrs);
        }
    };
    xml::parse(&text, |t| {
        if !matches!(t, Token::Attribute(..) | Token::TagContent(_) | Token::ParseError(_)) {
            flush(&mut tag);
        }
        match t {
            Token::StartTag(name) => {
                tag = Some((name.to_owned(), depth, Vec::new()));
                depth = depth.saturating_add(1);
            }
            Token::EmptyTag(name) => tag = Some((name.to_owned(), depth, Vec::new())),
            Token::Attribute(k, v) => {
                if let Some((_, _, attrs)) = &mut tag {
                    if attrs.len() < 32 {
                        attrs.push((k.to_owned(), clip(&decode_entities(v), MAX_TEXT)));
                    }
                }
            }
            Token::EndTag(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    });
    flush(&mut tag);
}

/// A tag being read: its name, depth and attributes.
type OpenTag = (String, usize, Vec<(String, String)>);

fn local_name(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

fn attr<'a>(attrs: &'a [(String, String)], k: &str) -> Option<&'a str> {
    attrs.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.trim())
}

fn clip(s: &str, max: usize) -> String {
    let mut cut = s.len().min(max);
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s[..cut].to_owned()
}

/// A Torznab error document, `<error code="100" description="Incorrect user credentials"/>`
/// (the root element): its code and description. Servers send it with HTTP 200 too.
pub fn parse_error(data: &[u8]) -> Option<(u32, String)> {
    let mut first = true;
    let mut out = None;
    // the root opens the document: no need to scan a whole result list for it
    scan_tags(&data[..data.len().min(16 * 1024)], |name, _, attrs| {
        if !std::mem::take(&mut first) || !local_name(name).eq_ignore_ascii_case("error") {
            return;
        }
        let code = attr(attrs, "code").and_then(|c| c.parse().ok()).unwrap_or(0);
        out = Some((code, attr(attrs, "description").unwrap_or("").to_owned()));
    });
    out
}

/// A category of a source's capabilities (`t=caps`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Category {
    pub id: u32,
    pub name: String,
    pub subcats: Vec<Category>,
}

/// Newznab's main categories a search can be limited to, with their names in the current
/// language (the search window's category list; "All categories" is the empty choice).
pub fn main_categories() -> [(u32, &'static str); 6] {
    [
        (2000, crate::tr!(SearchCatMovies)),
        (3000, crate::tr!(SearchCatAudio)),
        (4000, crate::tr!(SearchCatPc)),
        (5000, crate::tr!(SearchCatTv)),
        (7000, crate::tr!(SearchCatBooks)),
        (8000, crate::tr!(SearchCatOther)),
    ]
}

/// A result's Category cell: the name of the main category of its first standard category
/// (5040 TV/HD is "TV"); the number itself for one outside the list, empty for none.
/// Indexer-specific numbers (100000 and up) are skipped.
pub fn category_text(categories: &[u32]) -> String {
    let Some(&c) = categories.iter().find(|&&c| c < 100_000) else { return String::new() };
    let main = c / 1000 * 1000;
    main_categories()
        .iter()
        .find(|(id, _)| *id == main)
        .map_or_else(|| c.to_string(), |(_, n)| (*n).to_owned())
}

/// The `<categories>` of a caps answer, `<category id= name=>` with their `<subcat id= name=/>`;
/// an error document is the error, and an answer without a `<caps>` root is not Torznab.
pub fn parse_caps(data: &[u8]) -> Result<Vec<Category>, SearchError> {
    if let Some((code, description)) = parse_error(data) {
        return Err(SearchError::Torznab { code, description });
    }
    let mut caps = false;
    let mut cats: Vec<Category> = Vec::new();
    let mut count = 0;
    // the depth of the open `<categories>`, and whether its last child is a `<category>`; a tag
    // at or above that depth closes it (end tags are not needed: depths tell)
    let mut in_cats: Option<usize> = None;
    let mut open_cat = false;
    scan_tags(data, |name, depth, attrs| {
        let local = local_name(name);
        if depth == 0 && local.eq_ignore_ascii_case("caps") {
            caps = true;
        }
        if in_cats.is_some_and(|cd| depth <= cd) {
            in_cats = None;
        }
        if local.eq_ignore_ascii_case("categories") {
            in_cats = Some(depth);
            open_cat = false;
            return;
        }
        let Some(cd) = in_cats else { return };
        let is_cat = local.eq_ignore_ascii_case("category") && depth == cd + 1;
        if depth == cd + 1 {
            open_cat = false;
        }
        let id = attr(attrs, "id").and_then(|i| i.parse::<u32>().ok());
        let (Some(id), true) = (id, count < MAX_CATEGORIES) else { return };
        let cat = Category { id, name: attr(attrs, "name").unwrap_or("").to_owned(), subcats: Vec::new() };
        if is_cat {
            count += 1;
            cats.push(cat);
            open_cat = true;
        } else if local.eq_ignore_ascii_case("subcat") && open_cat && depth == cd + 2 {
            if let Some(parent) = cats.last_mut() {
                count += 1;
                parent.subcats.push(cat);
            }
        }
    });
    if caps { Ok(cats) } else { Err(SearchError::NotTorznab(String::new())) }
}

// --- URLs --------------------------------------------------------------------------------------

/// `endpoint` (the Torznab URL the user entered: Prowlarr's `{base}/{id}/api`, Jackett's
/// `{base}/api/v2.0/indexers/{id}/results/torznab/`) with `params` added to its query, values
/// percent-encoded. Parameters of the same names already there are replaced; others are kept,
/// and so is a fragment-free endpoint's existing query. Empty values are left out.
pub fn with_query(endpoint: &str, params: &[(&str, &str)]) -> String {
    let endpoint = endpoint.trim();
    let endpoint = endpoint.split_once('#').map_or(endpoint, |(e, _)| e);
    let (base, query) = endpoint.split_once('?').unwrap_or((endpoint, ""));
    let set = |k: &str| params.iter().any(|(p, v)| p.eq_ignore_ascii_case(k) && !v.is_empty());
    let mut parts: Vec<String> = query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .filter(|kv| !set(kv.split_once('=').map_or(*kv, |(k, _)| k)))
        .map(str::to_owned)
        .collect();
    for (k, v) in params {
        if !v.is_empty() {
            parts.push(format!("{k}={}", escape_string(v.as_bytes())));
        }
    }
    if parts.is_empty() { base.to_owned() } else { format!("{base}?{}", parts.join("&")) }
}

/// A search request: `t=search&q=&cat=&limit=` and the key.
pub fn search_url(endpoint: &str, api_key: &str, query: &str, categories: &[u32]) -> String {
    let cats: Vec<String> = categories.iter().map(u32::to_string).collect();
    let cats = cats.join(",");
    let limit = LIMIT.to_string();
    with_query(
        endpoint,
        &[
            ("t", "search"),
            ("q", query.trim()),
            ("cat", &cats),
            ("limit", &limit),
            ("apikey", api_key.trim()),
        ],
    )
}

/// A capabilities request (a source's Test): `t=caps` and the key.
pub fn caps_url(endpoint: &str, api_key: &str) -> String {
    with_query(endpoint, &[("t", "caps"), ("apikey", api_key.trim())])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prowlarr's shape: no `magneturl`, the link is Prowlarr's own download proxy.
    const PROWLARR: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <atom:link rel="self" type="application/rss+xml" />
    <title>Example Indexer</title>
    <item>
      <title>Linux Distro 24.04 x64 DVD</title>
      <guid>https://tracker.example.org/details/1234</guid>
      <prowlarrindexer id="5" type="public">Example Indexer</prowlarrindexer>
      <comments>https://tracker.example.org/details/1234</comments>
      <pubDate>Mon, 19 Oct 2026 06:35:37 +0000</pubDate>
      <size>4700000000</size>
      <link>http://localhost:9696/5/download?apikey=0123456789abcdef&amp;link=abc&amp;file=Linux+Distro</link>
      <category>4000</category>
      <enclosure url="http://localhost:9696/5/download?apikey=0123456789abcdef&amp;link=abc" length="4700000000" type="application/x-bittorrent" />
      <torznab:attr name="category" value="4000" />
      <torznab:attr name="category" value="100004" />
      <torznab:attr name="seeders" value="120" />
      <torznab:attr name="peers" value="135" />
      <torznab:attr name="infohash" value="114AAC2345B3A0A9DFFDC4A5D0922C69200B45F6" />
      <torznab:attr name="downloadvolumefactor" value="0" />
    </item>
    <item>
      <title>Linux Distro 24.04 arm64</title>
      <size>3000000000</size>
      <link>http://localhost:9696/5/download?apikey=0123456789abcdef&amp;link=def</link>
      <torznab:attr name="seeders" value="7" />
      <torznab:attr name="leechers" value="3" />
    </item>
    <item><title>Nothing to add</title><size>1</size></item>
    <item><link>http://localhost:9696/5/download?link=untitled</link></item>
  </channel>
</rss>"#;

    /// Jackett's shape: the same attributes and `magneturl`.
    const JACKETT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="1.0" xmlns:atom="http://www.w3.org/2005/Atom" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <title>AggregateSearch</title>
    <item>
      <title>Show.Name.S02E05.720p.HDTV.x264-GRP</title>
      <guid>https://tracker.example.org/t/99</guid>
      <jackettindexer id="exampletracker">Example Tracker</jackettindexer>
      <type>public</type>
      <link>http://192.168.1.20:9117/dl/exampletracker/?jackett_apikey=abcdefghijklmnop&amp;path=Q2ZESjh&amp;file=Show</link>
      <pubDate>Tue, 20 Oct 2026 10:00:00 +0200</pubDate>
      <size>1288490188</size>
      <description />
      <category>5000</category>
      <category>5040</category>
      <torznab:attr name="category" value="5000" />
      <torznab:attr name="category" value="5040" />
      <torznab:attr name="seeders" value="33" />
      <torznab:attr name="peers" value="40" />
      <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:CFH2QKGMI5WP2W6RHCIYGLQ2QE2IKVGZ&amp;dn=Show" />
    </item>
  </channel>
</rss>"#;

    #[test]
    fn a_prowlarr_answer() {
        let r = parse_results(PROWLARR.as_bytes()).unwrap();
        assert_eq!(r.len(), 2, "untitled items and items with nothing to add are skipped");
        let a = &r[0];
        assert_eq!(a.name, "Linux Distro 24.04 x64 DVD");
        assert_eq!(a.size, Some(4_700_000_000));
        assert_eq!((a.seeders, a.peers), (Some(120), Some(135)));
        assert_eq!(a.info_hash, "114aac2345b3a0a9dffdc4a5d0922c69200b45f6");
        assert_eq!(a.categories, [4000, 100_004]);
        assert_eq!(a.published, crate::rss::parse_date("Mon, 19 Oct 2026 06:35:37 +0000"));
        assert!(a.link.starts_with("http://localhost:9696/5/download?apikey=0123456789abcdef&link=abc"));
        assert_eq!(a.magnet, "");
        assert_eq!((a.source, a.sources.as_slice()), (0, &[0][..]));
        assert_eq!(
            a.magnet_link().unwrap(),
            "magnet:?xt=urn:btih:114aac2345b3a0a9dffdc4a5d0922c69200b45f6&dn=Linux%20Distro%2024.04%20x64%20DVD"
        );
        let b = &r[1];
        assert_eq!((b.seeders, b.peers), (Some(7), Some(10)), "peers from seeders and leechers");
        assert_eq!(b.info_hash, "");
        assert_eq!(b.magnet_link(), None);
    }

    #[test]
    fn a_jackett_answer_with_magneturl() {
        let r = parse_results(JACKETT.as_bytes()).unwrap();
        assert_eq!(r.len(), 1);
        let a = &r[0];
        assert_eq!(a.magnet, "magnet:?xt=urn:btih:CFH2QKGMI5WP2W6RHCIYGLQ2QE2IKVGZ&dn=Show");
        // the base32 btih, as hex
        assert_eq!(a.info_hash, "114fa828cc476cfd5bd13891832e1a81348554d9");
        assert_eq!(a.categories, [5000, 5040]);
        assert_eq!((a.seeders, a.peers, a.size), (Some(33), Some(40), Some(1_288_490_188)));
        assert!(a.link.starts_with("http://192.168.1.20:9117/dl/exampletracker/"));
        assert_eq!(a.magnet_link().as_deref(), Some(a.magnet.as_str()));
    }

    #[test]
    fn a_magnet_link_is_the_magnet_not_the_link() {
        let item = FeedItem {
            title: "x".into(),
            link: "magnet:?xt=urn:btih:114aac2345b3a0a9dffdc4a5d0922c69200b45f6".into(),
            ..FeedItem::default()
        };
        let r = SearchResult::from_item(&item, 3).unwrap();
        assert_eq!((r.link.as_str(), r.source), ("", 3));
        assert_eq!(r.info_hash, "114aac2345b3a0a9dffdc4a5d0922c69200b45f6");
        assert!(r.magnet.starts_with("magnet:"));
        // only an info-hash: still something to add
        let item = FeedItem { title: "y".into(), info_hash: "1".repeat(40), ..FeedItem::default() };
        assert_eq!(SearchResult::from_item(&item, 0).unwrap().info_hash, "1".repeat(40));
        let bad = FeedItem { title: "z".into(), info_hash: "nothex".into(), ..FeedItem::default() };
        assert_eq!(SearchResult::from_item(&bad, 0), None);
    }

    #[test]
    fn error_documents() {
        let bad_key = br#"<?xml version="1.0" encoding="UTF-8"?>
<error code="100" description="Invalid API Key" />"#;
        let e = parse_results(bad_key).unwrap_err();
        assert_eq!(e, SearchError::Torznab { code: 100, description: "Invalid API Key".into() });
        assert_eq!(e.to_string(), "100: Invalid API Key");
        assert_eq!(
            parse_error(br#"<error code="201" description="Function &quot;x&quot; not available"></error>"#),
            Some((201, "Function \"x\" not available".into()))
        );
        assert_eq!(parse_error(b"<error/>"), Some((0, String::new())));
        assert_eq!(SearchError::Torznab { code: 0, description: String::new() }.to_string(), "0");
        // an `error` that is not the root is no error document
        assert_eq!(parse_error(b"<rss><channel><error code=\"1\"/></channel></rss>"), None);
        assert_eq!(parse_error(PROWLARR.as_bytes()), None);
        assert!(matches!(parse_caps(bad_key), Err(SearchError::Torznab { code: 100, .. })));
        // neither
        assert_eq!(
            parse_results(b"<html><body>Login</body></html>"),
            Err(SearchError::NotTorznab(String::new()))
        );
        assert_eq!(parse_results(b""), Err(SearchError::NotTorznab(String::new())));
        // an empty result list is fine
        assert_eq!(parse_results(b"<rss><channel><title>x</title></channel></rss>"), Ok(Vec::new()));
    }

    #[test]
    fn caps_categories() {
        let caps = br#"<?xml version="1.0" encoding="UTF-8"?>
<caps>
  <server title="Jackett" />
  <limits default="100" max="100" />
  <searching>
    <search available="yes" supportedParams="q" />
    <tv-search available="yes" supportedParams="q,season,ep" />
  </searching>
  <categories>
    <category id="2000" name="Movies">
      <subcat id="2030" name="Movies/SD" />
      <subcat id="2040" name="Movies/HD" />
    </category>
    <category id="5000" name="TV &amp; Shows">
      <subcat id="5040" name="TV/HD"></subcat>
    </category>
    <category id="8000" name="Other" />
    <category id="bogus" name="skipped" />
    <category id="100001" name="Custom"><subcat id="100002" name="Custom/Sub"/></category>
  </categories>
  <subcat id="1" name="outside" />
</caps>"#;
        let cats = parse_caps(caps).unwrap();
        let ids: Vec<(u32, &str, Vec<u32>)> =
            cats.iter().map(|c| (c.id, c.name.as_str(), c.subcats.iter().map(|s| s.id).collect())).collect();
        assert_eq!(
            ids,
            [
                (2000, "Movies", vec![2030, 2040]),
                (5000, "TV & Shows", vec![5040]),
                (8000, "Other", vec![]),
                (100_001, "Custom", vec![100_002]),
            ]
        );
        assert_eq!(cats[0].subcats[1].name, "Movies/HD");
        assert_eq!(parse_caps(b"<caps></caps>"), Ok(Vec::new()));
        assert_eq!(parse_caps(PROWLARR.as_bytes()), Err(SearchError::NotTorznab(String::new())));
    }

    #[test]
    fn urls_keep_the_endpoint_and_encode_values() {
        // Prowlarr
        assert_eq!(
            search_url("http://localhost:9696/5/api", "k3y", "linux distro", &[]),
            "http://localhost:9696/5/api?t=search&q=linux%20distro&limit=100&apikey=k3y"
        );
        // Jackett, categories, special characters
        assert_eq!(
            search_url(
                " http://jackett.example.org:9117/api/v2.0/indexers/all/results/torznab/ ",
                "abc",
                "Tom & Jerry ?=#/ä",
                &[5000, 5040]
            ),
            "http://jackett.example.org:9117/api/v2.0/indexers/all/results/torznab/?t=search\
             &q=Tom%20%26%20Jerry%20%3f%3d%23%2f%c3%a4&cat=5000%2c5040&limit=100&apikey=abc"
        );
        // an existing query is kept, our names replaced, a fragment dropped; no key: none sent
        assert_eq!(
            search_url("https://idx.example.org/api?foo=1&T=caps&apikey=old#frag", "", "x", &[2000]),
            "https://idx.example.org/api?foo=1&apikey=old&t=search&q=x&cat=2000&limit=100"
        );
        assert_eq!(
            search_url("https://idx.example.org/api?apikey=old&", "new", "x", &[]),
            "https://idx.example.org/api?t=search&q=x&limit=100&apikey=new"
        );
        assert_eq!(
            caps_url("http://localhost:9696/5/api?", "k"),
            "http://localhost:9696/5/api?t=caps&apikey=k"
        );
        assert_eq!(with_query("http://h.example.org/x", &[("q", "")]), "http://h.example.org/x");
    }

    /// Answers captured from Prowlarr 2026-10-30 (its Generic Torznab indexer over the
    /// `tools/arr-test` fakeznab; the key replaced by `APIKEY`).
    #[test]
    fn captured_prowlarr_answers() {
        let r = parse_results(include_bytes!("fixtures/prowlarr-results.xml")).unwrap();
        let names: Vec<&str> = r.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Fawlty.Towers.S01E01.720p.WEB-DL.x264-PATEST",
                "Night.of.the.Living.Dead.1968.720p.WEB-DL.x264-PATEST",
                "PaTorrent.Prowlarr.Grab.Test.x264-PATEST"
            ]
        );
        let a = &r[0];
        assert_eq!((a.size, a.seeders, a.peers), (Some(122_170_147), Some(25), Some(30)));
        assert_eq!(a.info_hash, "58cebaa968651dc2a241c495719f2b6cbec33e32");
        assert_eq!(a.categories, [5000, 105000, 5040, 105040]);
        assert_eq!(category_text(&a.categories), "TV");
        assert!(a.link.starts_with("http://localhost:9696/2/download?apikey=APIKEY&link="), "{}", a.link);
        assert!(a.magnet.is_empty());
        assert_eq!(a.published, Some(1_793_351_163));
        let caps = parse_caps(include_bytes!("fixtures/prowlarr-caps.xml")).unwrap();
        let ids: Vec<(u32, usize)> = caps.iter().map(|c| (c.id, c.subcats.len())).collect();
        assert_eq!(ids, [(2000, 1), (5000, 1), (102000, 0), (102040, 0), (105000, 0), (105040, 0)]);
        assert_eq!(caps[1].subcats[0].name, "TV/HD");
    }

    #[test]
    fn category_cells() {
        assert_eq!(category_text(&[5040, 5000]), "TV");
        assert_eq!(category_text(&[100004, 4000]), "PC");
        assert_eq!(category_text(&[6000]), "6000");
        assert_eq!(category_text(&[100004]), "");
        assert_eq!(category_text(&[]), "");
        assert_eq!(main_categories()[0], (2000, "Movies"));
    }
}
