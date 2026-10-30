// Ported from libtorrent-rasterbar 2.0.11: src/magnet_uri.cpp (parse_magnet_uri, make_magnet_uri);
// fixes from 2.0.13 (invalid tracker URLs are rejected) and 2.1.1 (ca5a92e4f8: numbers that are out of
// range are rejected)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: parses into `MagnetUri` instead of `add_torrent_params`; a 40-character btih with
// non-hex characters is rejected (libtorrent ignores the from_hex failure and keeps garbage).

//! Magnet links (BEP 9 `xt`/`dn`/`tr`/`ws`, BEP 53 `so=`, `x.pe`, `dht`).

use std::net::SocketAddr;

use super::error::MetainfoError;
use super::torrent_info::{TorrentInfo, WebSeedKind, is_valid_tracker_url};
use crate::params::DownloadPriority;
use crate::types::{InfoHashes, Sha1Hash, Sha256Hash};
use crate::util::string::{
    base32decode, escape_string, parse_decimal, split_string, string_equal_no_case, unescape_string,
};

/// The contents of a magnet link.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MagnetUri {
    pub info_hashes: InfoHashes,
    /// `dn`: display name.
    pub name: String,
    /// `tr`: trackers; `tracker_tiers[i]` is the tier of `trackers[i]` (each gets its own).
    pub trackers: Vec<String>,
    pub tracker_tiers: Vec<u32>,
    /// `ws`: web seeds.
    pub url_seeds: Vec<String>,
    /// `x.pe`: peers to connect to.
    pub peers: Vec<SocketAddr>,
    /// `dht`: DHT bootstrap nodes as host and port.
    pub dht_nodes: Vec<(String, u16)>,
    /// `so`: select-only. Files not listed get [`DownloadPriority::DONT_DOWNLOAD`].
    pub file_priorities: Vec<DownloadPriority>,
    /// True when `so` was present, so unlisted files default to "don't download"
    /// (libtorrent `torrent_flags::default_dont_download`).
    pub default_dont_download: bool,
}

/// libtorrent `parse_magnet_uri`.
pub fn parse_magnet_uri(uri: &str) -> Result<MagnetUri, MetainfoError> {
    let mut sv = uri.strip_prefix("magnet:?").ok_or(MetainfoError::UnsupportedUrlProtocol)?;
    let mut p = MagnetUri::default();
    let mut display_name = String::new();
    let mut tier = 0u32;
    let (mut has_v1, mut has_v2) = (false, false);

    let unescape = |v: &str| unescape_string(v.as_bytes()).map(|b| String::from_utf8_lossy(&b).into_owned());

    while !sv.is_empty() {
        let (name, rest) = split_string(sv, '=');
        let (value, rest) = split_string(rest, '&');
        sv = rest;
        // names may carry a ".<number>" suffix with no meaning; strip it
        let (stripped, number) = split_string(name, '.');
        let name = if number.bytes().all(|c| c.is_ascii_digit()) { stripped } else { name };
        let is = |n: &str| string_equal_no_case(name.as_bytes(), n.as_bytes());

        if is("dn") {
            display_name = unescape(value).unwrap_or_default();
        } else if is("tr") {
            if let Some(tracker) = unescape(value).filter(|t| is_valid_tracker_url(t)) {
                p.trackers.push(tracker);
                p.tracker_tiers.push(tier);
                tier += 1;
            }
        } else if is("ws") {
            if let Some(ws) = unescape(value) {
                p.url_seeds.push(ws);
            }
        } else if is("xt") {
            let unescaped;
            let mut value = value;
            if value.contains('%') {
                unescaped = unescape(value).ok_or(MetainfoError::InvalidEscapedString)?;
                value = &unescaped;
            }
            if let Some(h) = value.strip_prefix("urn:btih:") {
                let hash = match h.len() {
                    40 => Sha1Hash::from_hex(h).ok_or(MetainfoError::InvalidInfoHash)?,
                    32 => base32decode(h.as_bytes())
                        .and_then(|b| (b.len() == 20).then(|| Sha1Hash::from_slice(&b)).flatten())
                        .ok_or(MetainfoError::InvalidInfoHash)?,
                    _ => return Err(MetainfoError::InvalidInfoHash),
                };
                p.info_hashes.v1 = Some(hash);
                has_v1 = true;
            } else if let Some(h) = value.strip_prefix("urn:btmh:") {
                // multihash: 0x12 = sha2-256, 0x20 = 32 bytes
                let h = h.strip_prefix("1220").ok_or(MetainfoError::InvalidInfoHash)?;
                if h.len() != 64 {
                    return Err(MetainfoError::InvalidInfoHash);
                }
                p.info_hashes.v2 = Some(Sha256Hash::from_hex(h).ok_or(MetainfoError::InvalidInfoHash)?);
                has_v2 = true;
            }
        } else if is("so") {
            parse_select_only(value, &mut p);
        } else if is("x.pe") {
            if let Ok(ep) = value.parse::<SocketAddr>() {
                p.peers.push(ep);
            }
        } else if is("dht") {
            if let Some(div) = value.rfind(':') {
                let port = parse_decimal(&value[div + 1..]);
                if let Some(port) = port.filter(|&p| p > 0 && p < u16::MAX as i32) {
                    p.dht_nodes.push((value[..div].to_owned(), port as u16));
                }
            }
        }
    }

    if !has_v1 && !has_v2 {
        return Err(MetainfoError::MissingInfoHashInUri);
    }
    if !display_name.is_empty() {
        p.name = display_name;
    }
    Ok(p)
}

/// BEP 53 `so=0,2,4-6`: only the listed files are downloaded.
fn parse_select_only(value: &str, p: &mut MagnetUri) {
    if value.bytes().any(|c| !c.is_ascii_digit() && c != b'-' && c != b',') {
        return;
    }
    // can't risk running out of memory on absurd indices
    const MAX_INDEX: i32 = 10000;
    p.default_dont_download = true;
    for token in value.split(',') {
        if token.is_empty() {
            continue;
        }
        // an index that is not a number, or out of range, skips the entry (ca5a92e4f8)
        let (first, last) = match token.find('-') {
            Some(0) => continue,                         // no start index
            Some(d) if d == token.len() - 1 => continue, // no end index
            Some(d) => (parse_decimal(&token[..d]), parse_decimal(&token[d + 1..])),
            None => {
                let i = parse_decimal(token);
                (i, i)
            }
        };
        let (Some(first), Some(last)) = (first, last) else { continue };
        if !(0..=MAX_INDEX).contains(&first) || !(0..=MAX_INDEX).contains(&last) || first > last {
            continue;
        }
        let (first, last) = (first as usize, last as usize);
        if p.file_priorities.len() <= last {
            p.file_priorities.resize(last + 1, DownloadPriority::DONT_DOWNLOAD);
        }
        p.file_priorities[first..=last].fill(DownloadPriority::NORMAL);
    }
}

impl MagnetUri {
    /// What libtorrent's `make_magnet_uri(torrent_info)` puts in a link: the info-hashes, the
    /// name, every tracker and the BEP 19 web seeds.
    pub fn from_torrent_info(ti: &TorrentInfo) -> MagnetUri {
        MagnetUri {
            info_hashes: ti.info_hashes(),
            name: ti.name().to_owned(),
            trackers: ti.trackers().iter().map(|t| t.url.clone()).collect(),
            tracker_tiers: ti.trackers().iter().map(|t| t.tier as u32).collect(),
            url_seeds: ti
                .web_seeds()
                .iter()
                .filter(|s| s.kind == WebSeedKind::UrlSeed)
                .map(|s| s.url.clone())
                .collect(),
            ..MagnetUri::default()
        }
    }
}

/// libtorrent `make_magnet_uri(torrent_info)`.
pub fn make_magnet_uri_from_info(ti: &TorrentInfo) -> String {
    make_magnet_uri(&MagnetUri::from_torrent_info(ti))
}

/// libtorrent `make_magnet_uri(add_torrent_params)`. Empty if there is no info-hash.
pub fn make_magnet_uri(m: &MagnetUri) -> String {
    let mut ret = String::from("magnet:?");
    let mut first = true;
    if let Some(v1) = m.info_hashes.v1 {
        ret.push_str("xt=urn:btih:");
        ret.push_str(&v1.to_hex());
        first = false;
    }
    if let Some(v2) = m.info_hashes.v2 {
        if !first {
            ret.push('&');
        }
        ret.push_str("xt=urn:btmh:1220");
        ret.push_str(&v2.to_hex());
        first = false;
    }
    if first {
        return String::new();
    }
    if !m.name.is_empty() {
        ret.push_str("&dn=");
        ret.push_str(&escape_string(m.name.as_bytes()));
    }
    for tr in &m.trackers {
        ret.push_str("&tr=");
        ret.push_str(&escape_string(tr.as_bytes()));
    }
    for ws in &m.url_seeds {
        ret.push_str("&ws=");
        ret.push_str(&escape_string(ws.as_bytes()));
    }
    for (host, port) in &m.dht_nodes {
        ret.push_str("&dht=");
        ret.push_str(&escape_string(host.as_bytes()));
        ret.push(':');
        ret.push_str(&port.to_string());
    }
    for peer in &m.peers {
        ret.push_str("&x.pe=");
        ret.push_str(&peer.to_string());
    }
    // only emit "so" if at least one file is skipped
    if m.file_priorities.contains(&DownloadPriority::DONT_DOWNLOAD) {
        ret.push_str("&so=");
        let mut ranges = Vec::new();
        let mut start: Option<usize> = None;
        for (idx, prio) in m.file_priorities.iter().enumerate() {
            match (*prio != DownloadPriority::DONT_DOWNLOAD, start) {
                (true, None) => start = Some(idx),
                (false, Some(s)) => {
                    ranges.push((s, idx - 1));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            ranges.push((s, m.file_priorities.len() - 1));
        }
        let parts: Vec<String> = ranges
            .into_iter()
            .map(|(a, b)| if a == b { a.to_string() } else { format!("{a}-{b}") })
            .collect();
        ret.push_str(&parts.join(","));
    }
    ret
}

#[cfg(test)]
mod tests {
    //! Ported from libtorrent-rasterbar 2.0.11 test/test_magnet.cpp (the parts that do not need a
    //! session).
    use super::*;

    const CD: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

    fn parse(s: &str) -> MagnetUri {
        parse_magnet_uri(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn v1_hex(p: &MagnetUri) -> String {
        p.info_hashes.v1.map(|h| h.to_hex()).unwrap_or_default()
    }

    #[test]
    fn escaped_hashes() {
        assert_eq!(v1_hex(&parse("magnet:?xt=urn%3Abtih%3Acdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd")), CD);
        assert_eq!(v1_hex(&parse("magnet:?xt=urn:btih:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdc%64")), CD);
        assert_eq!(parse_magnet_uri("magnet:?xt=urn%%3A"), Err(MetainfoError::InvalidEscapedString));
    }

    #[test]
    fn mixed_case_and_quoted() {
        let p = parse(
            "magnet:?XT=urn:btih:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdc%64&dN=foobar&Ws=http://foo.com/bar",
        );
        assert_eq!(v1_hex(&p), CD);
        assert_eq!(p.name, "foobar");
        assert_eq!(p.url_seeds, ["http://foo.com/bar"]);
        assert_eq!(v1_hex(&parse(&format!("magnet:?\"foo=bar&xt=urn:btih:{CD}"))), CD);
    }

    #[test]
    fn missing_and_invalid_hashes() {
        assert_eq!(
            parse_magnet_uri("magnet:?dn=foo&dht=127.0.0.1:43"),
            Err(MetainfoError::MissingInfoHashInUri)
        );
        assert_eq!(
            parse_magnet_uri("magnet:?xt=blah&dn=foo&dht=127.0.0.1:43"),
            Err(MetainfoError::MissingInfoHashInUri)
        );
        for bad in [
            "magnet:?xt=urn:btih:abababab",
            "magnet:?xt=urn:btih:ababababababababababab",
            "magnet:?xt=urn:btih: abababababababababab",
        ] {
            assert_eq!(parse_magnet_uri(bad), Err(MetainfoError::InvalidInfoHash), "{bad}");
        }
        assert_eq!(parse_magnet_uri("http://x"), Err(MetainfoError::UnsupportedUrlProtocol));
        // trailing whitespace makes a base32 hash invalid
        assert!(parse_magnet_uri("magnet:?xt=urn:btih:abaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n").is_err());
        assert!(parse_magnet_uri("magnet:?xt=urn:btih:abaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_ok());
    }

    #[test]
    fn base32_hash() {
        let p = parse("magnet:?xt=urn:btih:MFRGCYTBMJQWEYLCMFRGCYTBMJQWEYLC");
        assert_eq!(p.info_hashes.v1, Some(Sha1Hash(*b"abababababababababab")));
    }

    #[test]
    fn web_seeds() {
        let p = parse(&format!("magnet:?xt=urn:btih:{CD}&ws=http://foo.com/bar&ws=http://bar.com/foo"));
        assert_eq!(p.url_seeds, ["http://foo.com/bar", "http://bar.com/foo"]);
    }

    #[test]
    fn v2_and_hybrid() {
        let cd64 = CD.repeat(2)[..64].to_owned();
        let p = parse(&format!("magnet:?xt=urn:btmh:1220{cd64}"));
        assert_eq!(p.info_hashes.v2.unwrap().to_hex(), cd64);
        assert_eq!(p.info_hashes.v1, None);
        assert_eq!(
            parse_magnet_uri(
                "magnet:?xt=urn:btmh:1220cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdccdcdcdcdcdcdcd"
            ),
            Err(MetainfoError::InvalidInfoHash)
        );
        assert_eq!(
            parse_magnet_uri(&format!("magnet:?xt=urn:btmh:1221{cd64}")),
            Err(MetainfoError::InvalidInfoHash)
        );
        let p = parse(&format!("magnet:?xt=urn:btmh:1220{cd64}&xt=urn:btih:{CD}"));
        assert_eq!(v1_hex(&p), CD);
        assert_eq!(p.info_hashes.v2.unwrap().to_hex(), cd64);
    }

    #[test]
    fn peers_and_dht_nodes() {
        let p = parse(&format!(
            "magnet:?xt=urn:btih:{CD}&dn=foo&x.pe=127.0.0.1:43&x.pe=<invalid1>&x.pe=<invalid2>:100&x.pe=[::1]:45"
        ));
        assert_eq!(p.peers, ["127.0.0.1:43".parse::<SocketAddr>().unwrap(), "[::1]:45".parse().unwrap()]);
        let p = parse(&format!("magnet:?xt=urn:btih:{CD}&dn=foo&dht=127.0.0.1:43&dht=10.0.0.1:1337"));
        assert_eq!(p.dht_nodes, [("127.0.0.1".to_owned(), 43), ("10.0.0.1".to_owned(), 1337)]);
    }

    #[test]
    fn invalid_escaping() {
        assert!(parse_magnet_uri(
            "magnet:?tr=udp%3A%2F%2Ftracker.openjnt.com%\u{f7}A80&tr=udp%3A%2F%2Ftracker.pub.ciltbcom%3A80&xt=urn:btih:a38d02c287893842a39737aa866e00828aA80&xt=urn:buntu+11.04+%28Final%29"
        )
        .is_err());
        assert!(parse_magnet_uri("magnet:?ws=udp%3A%2F%2Ftracker.openjnt.com%\u{f7}A80").is_err());
        assert!(parse_magnet_uri("magnet:?tr=").map(|p| p.trackers.is_empty()).unwrap_or(true));
    }

    fn so(uri_tail: &str) -> Vec<u8> {
        parse(&format!("magnet:?xt=urn:btih:{CD}&dn=foo{uri_tail}"))
            .file_priorities
            .iter()
            .map(|p| p.0)
            .collect()
    }

    #[test]
    fn select_only() {
        assert_eq!(so("&so=0,2,4,6-8"), [4, 0, 4, 0, 4, 0, 4, 4, 4]);
        assert_eq!(so("&so=0,2-4,3-5&dht=10.0.0.1:1337"), [4, 0, 4, 4, 4, 4]);
        assert_eq!(so("&so=2-4&dht=10.0.0.1:1337&so=1"), [0, 4, 4, 4, 4]);
        assert_eq!(so("&so=7-4,100000000&dht=10.0.0.1:1337&so=10"), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4]);
        assert_eq!(so("&so=100000000&dht=10.0.0.1:1337&so=10"), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4]);
        assert_eq!(so("&so=-4&so=1"), [0, 4]);
        assert_eq!(so("&so=3-&so=1"), [0, 4]);
        assert_eq!(so("&so=a&so=1"), [0, 4]);
        assert_eq!(so("&so=100000000&so=1"), [0, 4]);
        assert_eq!(so("&so=&dht=10.0.0.1:1337&so="), [] as [u8; 0]);
        assert_eq!(so("&so=&dht=10.0.0.1:1337&so=,,1"), [0, 4]);
        assert_eq!(so("&so=\"1,2\""), [] as [u8; 0]);
        // ca5a92e4f8: a negative end index or one too large for an int skips the entry
        assert_eq!(so("&so=0--2,1"), [0, 4]);
        assert_eq!(so("&so=4294967297,0"), [4]);
    }

    #[test]
    fn dht_node_port_out_of_range() {
        // ca5a92e4f8: 4294967376 is 80 when truncated to an int
        let p = parse(&format!("magnet:?xt=urn:btih:{CD}&dht=10.0.0.1:4294967376&dht=10.0.0.2:-1&dht=a:1"));
        assert_eq!(p.dht_nodes, [("a".to_owned(), 1)]);
    }

    #[test]
    fn tracker_suffixes() {
        let p = parse(
            "magnet:?tr.0=udp://1&tr.1=http://2&tr=http://3&xt=urn:btih:c352cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
        );
        assert_eq!(p.trackers, ["udp://1", "http://2", "http://3"]);
        assert_eq!(p.tracker_tiers, [0, 1, 2]);
        let p =
            parse("magnet:?tr.a=udp://1&tr.1=http://2&xt=urn:btih:c352cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd");
        assert_eq!(p.trackers, ["http://2"]);
        assert_eq!(p.tracker_tiers, [0]);
    }

    /// libtorrent 2.0.13 reject_invalid_tracker_url.
    #[test]
    fn reject_invalid_tracker_url() {
        let p = parse(concat!(
            "magnet:?xt=urn:btih:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
            "&tr=ftp://1",
            "&tr=HTTP://2",
            "&tr=udp://4",
            "&tr=wss://4",
            "&tr=https://5",
            "&tr=magnet:?xt=urn:btih:abc123",
            "&tr=http%3A%2F%2Ffoo",
            "&tr=<!DOCTYPE html><html>",
            "&tr=httpabc",
            "&tr=udpabc",
            "&tr=",
            "&tr=garbage",
        ));
        assert_eq!(p.trackers, ["HTTP://2", "udp://4", "https://5", "http://foo"]);
        assert_eq!(p.tracker_tiers, [0, 1, 2, 3]);
    }

    fn with_v1() -> MagnetUri {
        MagnetUri { info_hashes: InfoHashes::v1(Sha1Hash(*b"aaaaaaaaaaaaaaaaaaaa")), ..MagnetUri::default() }
    }

    const AA: &str = "magnet:?xt=urn:btih:6161616161616161616161616161616161616161";

    #[test]
    fn make_uris() {
        let v2 = MagnetUri {
            info_hashes: InfoHashes { v1: None, v2: Some(Sha256Hash(*b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")) },
            ..MagnetUri::default()
        };
        assert_eq!(make_magnet_uri(&v2), format!("magnet:?xt=urn:btmh:1220{}", "61".repeat(32)));
        assert_eq!(make_magnet_uri(&MagnetUri::default()), "");

        let mut m = with_v1();
        let prio = |v: &[u8]| v.iter().map(|&p| DownloadPriority(p)).collect::<Vec<_>>();
        for (p, tail) in [
            (&[0u8, 1][..], "&so=1"),
            (&[1, 1], ""),
            (&[1, 0], "&so=0"),
            (&[1, 0, 1, 1, 1], "&so=0,2-4"),
            (&[1, 1, 0, 1, 1, 1], "&so=0-1,3-5"),
            (&[1, 0, 1, 0, 1], "&so=0,2,4"),
        ] {
            m.file_priorities = prio(p);
            assert_eq!(make_magnet_uri(&m), format!("{AA}{tail}"));
        }

        let mut m = with_v1();
        m.peers = vec!["127.0.0.1:6881".parse().unwrap(), "1.2.3.4:1234".parse().unwrap()];
        assert_eq!(make_magnet_uri(&m), format!("{AA}&x.pe=127.0.0.1:6881&x.pe=1.2.3.4:1234"));

        let mut m = with_v1();
        m.dht_nodes = vec![("127.0.0.1".into(), 6881), ("foobar.com".into(), 1)];
        assert_eq!(make_magnet_uri(&m), format!("{AA}&dht=127.0.0.1:6881&dht=foobar.com:1"));

        let mut m = with_v1();
        m.url_seeds = vec!["http://foo.com/bar".into()];
        assert!(make_magnet_uri(&m).contains("&ws=http%3a%2f%2ffoo.com%2fbar"));
    }

    fn bencode_torrent(extra: Vec<(&str, crate::bencode::Entry)>) -> TorrentInfo {
        use crate::bencode::Entry;
        let mut info = Entry::new_dict();
        *info.key("pieces") = "aaaaaaaaaaaaaaaaaaaa".into();
        *info.key("name") = "slightly shorter name, it's kind of sad that people started the trend of incorrectly encoding the regular name field and then adding another one with correct encoding".into();
        *info.key("name.utf-8") = "this is a long ass name in order to try to make make_magnet_uri overflow and hopefully crash. Although, by the time you read this that particular bug should have been fixed".into();
        *info.key("piece length") = Entry::Int(16 * 1024);
        *info.key("length") = Entry::Int(3245);
        let mut torrent = Entry::new_dict();
        *torrent.key("info") = info;
        for (k, v) in extra {
            *torrent.key(k) = v;
        }
        TorrentInfo::from_bytes(&torrent.bencode()).unwrap()
    }

    #[test]
    fn make_magnet_uri_torrent_info() {
        use crate::bencode::Entry;
        let urls = [
            "http://bigtorrent.org:2710/announce",
            "http://bt.careland.com.cn:6969/announce",
            "http://bt.e-burg.org:2710/announce",
            "http://bttrack.9you.com/announce",
            "http://coppersurfer.tk:6969/announce",
            "http://erdgeist.org/arts/software/opentracker/announce",
            "http://exodus.desync.com/announce",
            "http://fr33dom.h33t.com:3310/announce",
            "http://genesis.1337x.org:1337/announce",
            "http://inferno.demonoid.me:3390/announce",
            "http://inferno.demonoid.ph:3390/announce",
            "http://ipv6.tracker.harry.lu/announce",
            "http://lnxroot.com:6969/announce",
            "http://nemesis.1337x.org/announce",
            "http://puto.me:6969/announce",
            "http://sline.net:2710/announce",
            "http://tracker.beeimg.com:6969/announce",
            "http://tracker.ccc.de/announce",
            "http://tracker.coppersurfer.tk/announce",
            "http://tracker.coppersurfer.tk:6969/announce",
            "http://tracker.cpleft.com:2710/announce",
            "http://tracker.istole.it/announce",
            "http://tracker.kamyu.net/announce",
            "http://tracker.novalayer.org:6969/announce",
            "http://tracker.torrent.to:2710/announce",
            "http://tracker.torrentbay.to:6969/announce",
            "udp://tracker.openbittorrent.com:80",
            "udp://tracker.publicbt.com:80",
        ];
        let tier = Entry::List(urls.iter().map(|&u| Entry::from(u)).collect());
        let ti = bencode_torrent(vec![("announce-list", Entry::List(vec![tier]))]);
        assert_eq!(ti.trackers().len(), urls.len());
        let magnet = make_magnet_uri_from_info(&ti);
        let back = parse(&magnet);
        // the parser shuffles trackers within a tier
        let mut got = back.trackers.clone();
        got.sort();
        assert_eq!(got, urls);
        assert_eq!(back.info_hashes, ti.info_hashes());
        assert_eq!(back.name, ti.name());
        assert!(magnet.starts_with(&format!("magnet:?xt=urn:btih:{}&dn=", ti.info_hash().to_hex())));

        // make_magnet_uri2: url-list becomes ws=, httpseeds are left out
        let ti = bencode_torrent(vec![
            ("url-list", "http://foo.com/bar".into()),
            ("httpseeds", "http://http.seed/".into()),
        ]);
        let magnet = make_magnet_uri_from_info(&ti);
        assert!(magnet.contains("&ws=http%3a%2f%2ffoo.com%2fbar"));
        assert!(!magnet.contains("http.seed"));

        // make_magnet_uri_v1
        assert!(magnet.contains("xt=urn:btih:"));
        assert!(!magnet.contains("xt=urn:btmh:1220"));
    }

    #[test]
    fn round_trip() {
        let check = |m: &MagnetUri| {
            let back = parse(&make_magnet_uri(m));
            assert_eq!(back.info_hashes, m.info_hashes);
            assert_eq!(back.trackers, m.trackers);
            assert_eq!(back.url_seeds, m.url_seeds);
            assert_eq!(back.peers, m.peers);
            assert_eq!(back.dht_nodes, m.dht_nodes);
            assert_eq!(back.file_priorities, m.file_priorities);
            assert_eq!(back.name, m.name);
        };
        let mut m = with_v1();
        check(&m);
        m.dht_nodes = vec![("foobar.com".into(), 1)];
        check(&m);
        m.peers = vec!["127.0.0.1:6881".parse().unwrap()];
        check(&m);
        for p in [&[0u8, 4][..], &[4, 0, 4, 4, 4], &[4, 4, 0, 4, 4, 4], &[4, 0, 4, 0, 4]] {
            m.file_priorities = p.iter().map(|&x| DownloadPriority(x)).collect();
            check(&m);
        }
        m.name = "a name/with & stuff".into();
        m.trackers = vec!["udp://tracker.example:1337/announce".into()];
        m.tracker_tiers = vec![0];
        check(&m);
    }
}
