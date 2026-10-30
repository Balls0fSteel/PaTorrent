//! Search sources (D41): the user's Torznab endpoints (a Prowlarr indexer, a Jackett `all`),
//! kept in the preferences as `search.source.<n>.name|url|key|enabled`, numbered from 0
//! without gaps. The API key is sealed in the file like the proxy password
//! ([`crate::prefs::is_sealed_key`]) and never shown: [`redact`] takes it out of any URL or
//! message.

use crate::prefs::Prefs;

/// Sources kept.
pub const MAX_SOURCES: usize = 256;

const PREFIX: &str = "search.source.";

/// One Torznab endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The name shown (the indexer's).
    pub name: String,
    /// The full Torznab endpoint: Prowlarr's `{base}/{indexerId}/api`, Jackett's
    /// `{base}/api/v2.0/indexers/{id}/results/torznab/`.
    pub url: String,
    /// Sent as `apikey=`. Never logged or shown (see [`redact`]).
    pub api_key: String,
    pub enabled: bool,
}

impl Default for Source {
    fn default() -> Source {
        Source { name: String::new(), url: String::new(), api_key: String::new(), enabled: true }
    }
}

impl Source {
    /// The name, else the endpoint's host.
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.trim().to_owned();
        }
        url_host(&self.url).unwrap_or_else(|| redact(self.url.trim(), &self.api_key))
    }

    /// `text` with this source's key taken out ([`redact`]).
    pub fn redact(&self, text: &str) -> String {
        redact(text, &self.api_key)
    }
}

fn key(n: usize, field: &str) -> String {
    format!("{PREFIX}{n}.{field}")
}

/// The sources in `prefs`, in order. A number without a URL is skipped.
pub fn load_sources(prefs: &Prefs) -> Vec<Source> {
    (0..MAX_SOURCES)
        .filter_map(|n| {
            let url = prefs.get(&key(n, "url"))?.trim();
            (!url.is_empty()).then(|| Source {
                name: prefs.get(&key(n, "name")).unwrap_or("").trim().to_owned(),
                url: url.to_owned(),
                api_key: prefs.get(&key(n, "key")).unwrap_or("").trim().to_owned(),
                enabled: prefs.get_bool(&key(n, "enabled"), true),
            })
        })
        .collect()
}

/// Writes `sources` as numbers 0.. and removes every other `search.source.*` key, so a removed
/// source leaves no gap and no stale key behind (the sealed one included).
pub fn store_sources(prefs: &mut Prefs, sources: &[Source]) {
    let sources = &sources[..sources.len().min(MAX_SOURCES)];
    for (n, s) in sources.iter().enumerate() {
        prefs.set(&key(n, "name"), s.name.trim());
        prefs.set(&key(n, "url"), s.url.trim());
        if s.api_key.trim().is_empty() {
            prefs.remove(&key(n, "key"));
        } else {
            prefs.set(&key(n, "key"), s.api_key.trim());
        }
        prefs.set_bool(&key(n, "enabled"), s.enabled);
    }
    let fields = ["name", "url", "key", "enabled"];
    let stale: Vec<String> = prefs
        .entries()
        .map(|(k, _)| k)
        .filter(|k| {
            let Some((n, field)) = k.strip_prefix(PREFIX).and_then(|r| r.split_once('.')) else {
                return false;
            };
            let current = n.parse::<usize>().is_ok_and(|n| n < sources.len()) && fields.contains(&field);
            !current
        })
        .map(str::to_owned)
        .collect();
    for k in stale {
        prefs.remove(&k);
    }
}

/// Removes source `index` and numbers the rest again; false when there is no such source.
pub fn remove_source(prefs: &mut Prefs, index: usize) -> bool {
    let mut sources = load_sources(prefs);
    if index >= sources.len() {
        return false;
    }
    sources.remove(index);
    store_sources(prefs, &sources);
    true
}

/// `text` (a URL, an error message) with the API key taken out: every occurrence of `key`
/// (as is and percent-encoded) and the value of any `apikey=`-like query parameter
/// (`apikey`, `api_key`, `jackett_apikey`: download links carry the server's key) become
/// `***`.
pub fn redact(text: &str, key: &str) -> String {
    let mut out = text.to_owned();
    let key = key.trim();
    if !key.is_empty() {
        out = out.replace(key, "***");
        let enc = patorrent_core::util::string::escape_string(key.as_bytes());
        if enc != key {
            out = out.replace(&enc, "***");
        }
    }
    mask_key_params(&out)
}

/// Replaces the values of query parameters named like an API key.
fn mask_key_params(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        let at_param = (bytes[i] == b'?' || bytes[i] == b'&') && i + 1 < bytes.len();
        if at_param {
            let start = i + 1;
            let name_end = lower[start..]
                .find(|c: char| c == '=' || c == '&' || c.is_whitespace())
                .map_or(lower.len(), |e| start + e);
            let name = lower[start..name_end].replace(['_', '-'], "");
            if name.ends_with("apikey") && bytes.get(name_end) == Some(&b'=') {
                out.push_str(&text[i..=name_end]);
                let val_end = text[name_end + 1..]
                    .find(|c: char| c == '&' || c == '#' || c.is_whitespace() || c == '"' || c == '\'')
                    .map_or(text.len(), |e| name_end + 1 + e);
                if val_end > name_end + 1 {
                    out.push_str("***");
                }
                i = val_end;
                continue;
            }
        }
        let c = text[i..].chars().next().expect("in bounds");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// The host of an `http(s)://` URL, lowercased, without user info, port or IPv6 brackets.
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = hostport.strip_prefix('[') {
        v6.split_once(']')?.0
    } else {
        hostport.split(':').next()?
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(name: &str, url: &str, key: &str, enabled: bool) -> Source {
        Source { name: name.into(), url: url.into(), api_key: key.into(), enabled }
    }

    #[test]
    fn sources_round_trip_through_the_preferences() {
        let mut p = Prefs::default();
        p.set("search.other", "kept");
        let list = vec![
            src("Indexer A", "http://localhost:9696/1/api", "k3y-1", true),
            src(
                "Jackett",
                "http://jackett.example.org:9117/api/v2.0/indexers/all/results/torznab/",
                "",
                false,
            ),
            src("", "https://idx.example.org/api", "k3y 3", true),
        ];
        store_sources(&mut p, &list);
        assert_eq!(load_sources(&p), list);
        assert_eq!(p.get("search.source.1.enabled"), Some("0"));
        assert_eq!(p.get("search.source.1.key"), None, "an empty key is not stored");
        let text = p.to_text();
        if cfg!(windows) {
            assert!(!text.contains("k3y-1") && !text.contains("k3y 3"), "{text}");
        }
        let q = Prefs::from_text(&text);
        assert_eq!(load_sources(&q), list, "sealed keys open again");

        // removing renumbers and leaves nothing behind
        let mut q = q;
        q.set("search.source.2.extra", "stale");
        assert!(remove_source(&mut q, 0));
        assert!(!remove_source(&mut q, 5));
        assert_eq!(load_sources(&q), list[1..]);
        assert_eq!(q.get("search.source.0.name"), Some("Jackett"));
        assert_eq!(q.get("search.source.1.key"), Some("k3y 3"));
        assert!(q.entries().all(|(k, _)| !k.starts_with("search.source.2.")), "{}", q.to_text());
        assert_eq!(q.get("search.other"), Some("kept"));
        store_sources(&mut q, &[]);
        assert!(q.entries().all(|(k, _)| !k.starts_with("search.source.")));

        // a hand-edited gap is skipped; enabled defaults to on
        let r = Prefs::from_text(
            "search.source.0.url=http://a.example.org/api\nsearch.source.2.url=http://b.example.org/api\n",
        );
        let l = load_sources(&r);
        assert_eq!(l.len(), 2);
        assert!(l.iter().all(|s| s.enabled));
        assert_eq!(l[1].label(), "b.example.org");
    }

    #[test]
    fn keys_never_show() {
        let key = "s3cr3t+key/=";
        let url = format!("http://localhost:9696/1/api?t=search&q=x&apikey={}", "s3cr3t%2bkey%2f%3d");
        assert_eq!(redact(&url, key), "http://localhost:9696/1/api?t=search&q=x&apikey=***");
        assert_eq!(redact("HTTP 401 for s3cr3t+key/= here", key), "HTTP 401 for *** here");
        // the server's key in its own download links, whatever we know
        assert_eq!(
            redact("http://192.168.1.20:9117/dl/t/?jackett_apikey=abc123&path=x: not a .torrent file", ""),
            "http://192.168.1.20:9117/dl/t/?jackett_apikey=***&path=x: not a .torrent file"
        );
        assert_eq!(
            redact("https://h.example.org/5/download?link=a&API_KEY=zz#f and ?apikey= empty", "  "),
            "https://h.example.org/5/download?link=a&API_KEY=***#f and ?apikey= empty"
        );
        assert_eq!(redact("no key ä here?&", "x9"), "no key ä here?&");
        let s = src("", "http://user:pw@[::1]:9696/1/api?apikey=k", "k", true);
        assert_eq!(s.label(), "::1");
        assert_eq!(src("", "not a url k", "k", true).label(), "not a url ***");
    }

    #[test]
    fn hosts() {
        assert_eq!(url_host("http://LocalHost:9696/1/api").as_deref(), Some("localhost"));
        assert_eq!(url_host("https://a@idx.example.org./x?y").as_deref(), Some("idx.example.org"));
        assert_eq!(url_host("http://[fe80::1]:80/").as_deref(), Some("fe80::1"));
        assert_eq!(url_host("http://192.168.1.20?x").as_deref(), Some("192.168.1.20"));
        assert_eq!(url_host("magnet:?xt=urn:btih:x"), None);
        assert_eq!(url_host("http:///x"), None);
    }
}
