// Ported from libtorrent-rasterbar 2.0.11: src/string_util.cpp, src/escape_string.cpp,
// src/parse_url.cpp; fixes from 2.0.13 (188c5226f7) and 2.1.1 (ca5a92e4f8's parse_decimal)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: byte-slice based; errors are `Option`/`Result` instead of `error_code`.

//! ASCII string helpers, URL escaping, base32 and URL splitting.

/// libtorrent `is_space`.
pub fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\x0b')
}

/// ASCII-only case-insensitive equality (libtorrent `string_equal_no_case`).
pub fn string_equal_no_case(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// True if `s2` starts with `s1`, ignoring ASCII case (libtorrent `string_begins_no_case`).
pub fn string_begins_no_case(s1: &[u8], s2: &[u8]) -> bool {
    s2.len() >= s1.len() && s1.eq_ignore_ascii_case(&s2[..s1.len()])
}

/// Removes leading whitespace (libtorrent `ltrim`).
pub fn ltrim(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_ascii() && is_space(c as u8))
}

/// Removes leading and trailing whitespace (libtorrent `trim`).
pub fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii() && is_space(c as u8))
}

/// libtorrent `parse_decimal` (ca5a92e4f8, replacing `atoi`): the leading decimal number of
/// `s` (`std::from_chars` rules: an optional `-`, then digits; no whitespace or `+`). `None` if
/// there is no number or it does not fit in an `i32`, so an out-of-range value can never pass
/// a range check the way a truncating or saturating `atoi` lets it.
pub fn parse_decimal(s: &str) -> Option<i32> {
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let n = digits.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 {
        return None;
    }
    let v: i64 = digits[..n].bytes().try_fold(0i64, |a, d| {
        a.checked_mul(10)?.checked_add(i64::from(d - b'0')).filter(|&v| v <= i64::from(i32::MAX) + 1)
    })?;
    i32::try_from(if neg { -v } else { v }).ok()
}

pub fn ensure_trailing_slash(url: &mut String) {
    if !url.ends_with('/') {
        url.push('/');
    }
}

/// Splits at the first `sep` (libtorrent `split_string`).
pub fn split_string(s: &str, sep: char) -> (&str, &str) {
    s.split_once(sep).unwrap_or((s, ""))
}

/// RFC 2396 section 2.3; the first 11 characters ("%+;?:@=&,$/") are only conditionally
/// unreserved (see `escape_impl` offsets).
const UNRESERVED: &[u8] =
    b"%+;?:@=&,$/-_!.~*()ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

fn escape_impl(s: &[u8], offset: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let allowed = &UNRESERVED[offset..];
    let mut out = String::with_capacity(s.len());
    for &c in s {
        if c != 0 && allowed.contains(&c) {
            out.push(c as char);
        } else {
            out.push('%');
            out.push(HEX[(c >> 4) as usize] as char);
            out.push(HEX[(c & 15) as usize] as char);
        }
    }
    out
}

/// Percent-encodes everything except unreserved characters (libtorrent `escape_string`).
pub fn escape_string(s: &[u8]) -> String {
    escape_impl(s, 11)
}

/// Like [`escape_string`] but keeps '/' (libtorrent `escape_path`).
pub fn escape_path(s: &[u8]) -> String {
    escape_impl(s, 10)
}

/// True if `s` contains a character that must be percent-encoded in a URL.
pub fn need_encoding(s: &[u8]) -> bool {
    s.iter().any(|&c| c == 0 || !UNRESERVED.contains(&c))
}

/// Percent-decoding with '+' as space (libtorrent `unescape_string`). Fails on truncated or
/// non-hex escapes.
pub fn unescape_string(s: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = |c: Option<&u8>| (*c? as char).to_digit(16);
                let (hi, lo) = (hex(s.get(i + 1))?, hex(s.get(i + 2))?);
                out.push((hi * 16 + lo) as u8);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    Some(out)
}

/// Percent-encodes the path of `url` if it contains characters that need it, leaving
/// already-valid URLs (and anything that doesn't parse as a URL) untouched.
pub fn maybe_url_encode(url: &str) -> String {
    let Ok(u) = split_url(url) else { return url.to_owned() };
    if !need_encoding(u.path.as_bytes()) {
        return url.to_owned();
    }
    let mut msg = String::with_capacity(url.len() + 16);
    msg.push_str(&u.protocol);
    msg.push_str("://");
    if !u.auth.is_empty() {
        msg.push_str(&u.auth);
        msg.push('@');
    }
    msg.push_str(&u.host);
    if let Some(port) = u.port {
        msg.push(':');
        msg.push_str(&port.to_string());
    }
    msg.push_str(&escape_path(u.path.as_bytes()));
    msg
}

/// RFC 4648 base64 with padding (libtorrent `base64encode`), for HTTP basic authentication.
pub fn base64encode(s: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(s.len().div_ceil(3) * 4);
    for c in s.chunks(3) {
        let b = [c[0], c.get(1).copied().unwrap_or(0), c.get(2).copied().unwrap_or(0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// RFC 4648 base64 decoding, padding optional (PEM bodies, joined without line breaks). `None`
/// on any other character or an impossible length.
pub fn base64decode(s: &[u8]) -> Option<Vec<u8>> {
    let s = match s {
        [rest @ .., b'=', b'='] | [rest @ .., b'='] => rest,
        _ => s,
    };
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in s {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// RFC 4648 base32 decoding, case-insensitive, tolerating missing padding and '1' for 'I'
/// (libtorrent `base32decode`). `None` on invalid characters.
pub fn base32decode(s: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8 + 5);
    for chunk in s.chunks(8) {
        let mut inbuf = [0u8; 8];
        let mut pad_start = if chunk.len() < 8 { chunk.len() } else { 0 };
        for (j, &c) in chunk.iter().enumerate() {
            inbuf[j] = match c.to_ascii_uppercase() {
                c @ b'A'..=b'Z' => c - b'A',
                c @ b'2'..=b'7' => c - b'2' + 26,
                b'=' => {
                    if pad_start == 0 {
                        pad_start = j;
                    }
                    0
                }
                b'1' => b'I' - b'A',
                _ => return None,
            };
        }
        let o = [
            (inbuf[0] << 3) | (inbuf[1] >> 2),
            ((inbuf[1] & 0x3) << 6) | (inbuf[2] << 1) | ((inbuf[3] & 0x10) >> 4),
            ((inbuf[3] & 0x0f) << 4) | ((inbuf[4] & 0x1e) >> 1),
            ((inbuf[4] & 0x01) << 7) | ((inbuf[5] & 0x1f) << 2) | ((inbuf[6] & 0x18) >> 3),
            ((inbuf[6] & 0x07) << 5) | inbuf[7],
        ];
        const OUT_FOR_INPUT: [usize; 9] = [5, 1, 1, 2, 2, 3, 4, 4, 5];
        out.extend_from_slice(&o[..OUT_FOR_INPUT[pad_start]]);
    }
    Some(out)
}

/// Parsed URL parts (libtorrent `parse_url_components`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlParts {
    pub protocol: String,
    /// `user:pass`, without the '@'.
    pub auth: String,
    /// Without brackets for IPv6 literals.
    pub host: String,
    pub port: Option<u16>,
    /// Starts with '/'; includes query and fragment.
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlError {
    UnsupportedProtocol,
    ExpectedCloseBracket,
    InvalidPort,
    /// A control character (CR, LF, NUL, ...) inside the URL. Our requests are built by
    /// formatting URL parts into a request line, so one of these would split it (SEC-19).
    InvalidCharacter,
}

/// libtorrent `parse_url_components`, refusing URLs with control characters, since the parts
/// go straight into HTTP request lines. Surrounding whitespace is trimmed first, as torrents
/// in the wild end announce URLs with a newline.
pub fn parse_url_components(url: &str) -> Result<UrlParts, UrlError> {
    let url = trim(url);
    if url.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(UrlError::InvalidCharacter);
    }
    split_url(url)
}

/// [`parse_url_components`] without the character check, for [`maybe_url_encode`], which
/// escapes such characters instead.
fn split_url(url: &str) -> Result<UrlParts, UrlError> {
    let url = ltrim(url);
    let colon = url.find(':').ok_or(UrlError::UnsupportedProtocol)?;
    let protocol = &url[..colon];
    let rest = url[colon + 1..].strip_prefix("//").ok_or(UrlError::UnsupportedProtocol)?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let mut start = 0;
    let mut auth = "";
    if let (Some(at), Some(c)) = (rest.find('@'), rest.find(':')) {
        if c < at && at < end {
            auth = &rest[..at];
            start = at + 1;
        }
    }
    let host_port = &rest[start..end];
    let (host, port_str) = if let Some(v6) = host_port.strip_prefix('[') {
        let close = v6.find(']').ok_or(UrlError::ExpectedCloseBracket)?;
        (&v6[..close], v6[close + 1..].strip_prefix(':'))
    } else {
        match host_port.find(':') {
            Some(i) => (&host_port[..i], Some(&host_port[i + 1..])),
            None => (host_port, None),
        }
    };
    let port = match port_str {
        Some(p) if !p.bytes().all(|c| c.is_ascii_digit()) => return Err(UrlError::InvalidPort),
        Some(p) => Some(p.parse().map_err(|_| UrlError::InvalidPort)?),
        None => None,
    };
    let mut path = rest[end..].to_owned();
    if path.is_empty() || path.starts_with(['?', '#']) {
        path.insert(0, '/');
    }
    Ok(UrlParts { protocol: protocol.to_owned(), auth: auth.to_owned(), host: host.to_owned(), port, path })
}

/// libtorrent `same_origin`: true if the two URLs have the same origin, i.e. the same scheme,
/// host and (effective) port. Scheme and host are compared case-insensitively. This is meant
/// for http/https URLs: a missing port is treated as 443 for "https" and 80 otherwise. False if
/// either URL fails to parse. Decides whether credentials may be forwarded across a redirect.
pub fn same_origin(a: &str, b: &str) -> bool {
    // userinfo, path and query are not part of the origin
    let origin = |u: &str| {
        let p = parse_url_components(u).ok()?;
        let port =
            p.port.unwrap_or(if string_equal_no_case(p.protocol.as_bytes(), b"https") { 443 } else { 80 });
        Some((p.protocol, p.host, port))
    };
    match (origin(a), origin(b)) {
        (Some(oa), Some(ob)) => {
            // parsed successfully (host non-empty)
            !oa.1.is_empty()
                && string_equal_no_case(oa.0.as_bytes(), ob.0.as_bytes())
                && string_equal_no_case(oa.1.as_bytes(), ob.1.as_bytes())
                && oa.2 == ob.2
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    //! Ported from libtorrent-rasterbar 2.0.11 test/test_string.cpp.
    use super::*;

    #[test]
    fn parse_decimal_cases() {
        // after ca5a92e4f8's test_string.cpp parse_decimal
        for (s, v) in [
            ("0", 0),
            ("80", 80),
            ("65535", 65535),
            ("2147483647", i32::MAX),
            ("-2147483648", i32::MIN),
            ("-4", -4),
            ("-1", -1),
            ("200 OK", 200),
            ("80/announce", 80),
        ] {
            assert_eq!(parse_decimal(s), Some(v), "{s:?}");
        }
        for s in ["", "abc", " 80", "+80", "-", "2147483648", "4294967376", "4294967496 Forbidden"] {
            assert_eq!(parse_decimal(s), None, "{s:?}");
        }
        assert_eq!(parse_decimal("99999999999999999999"), None);
    }

    #[test]
    fn maybe_url_encode_cases() {
        assert_eq!(
            maybe_url_encode("http://test:test@abc.com/abc<>abc"),
            "http://test:test@abc.com/abc%3c%3eabc"
        );
        assert_eq!(maybe_url_encode("http://abc.com/foo bar"), "http://abc.com/foo%20bar");
        assert_eq!(maybe_url_encode("http://abc.com:80/foo bar"), "http://abc.com:80/foo%20bar");
        assert_eq!(maybe_url_encode("http://abc.com:8080/foo bar"), "http://abc.com:8080/foo%20bar");
        assert_eq!(maybe_url_encode("abc"), "abc");
        assert_eq!(maybe_url_encode("http://abc.com/abc"), "http://abc.com/abc");
        assert_eq!(maybe_url_encode("http://bla.com/\n"), "http://bla.com/%0a");
        assert_eq!(maybe_url_encode("http://bla.com/foo%20bar"), "http://bla.com/foo%20bar");
        assert_eq!(
            maybe_url_encode("http://bla.com/foo%20bar?k=v&k2=v2"),
            "http://bla.com/foo%20bar?k=v&k2=v2"
        );
        assert_eq!(maybe_url_encode("?&"), "?&");
    }

    #[test]
    fn space_and_case() {
        for c in [b'C', 8, b'8', b'='] {
            assert!(!is_space(c));
        }
        for c in [b' ', b'\t', b'\n', b'\r', 0x0c, 0x0b] {
            assert!(is_space(c));
        }
        assert!(string_equal_no_case(b"foobar", b"FoobAR"));
        assert!(string_equal_no_case(b"foobar", b"foobar"));
        assert!(!string_equal_no_case(b"foobar", b"foobar "));
        assert!(!string_equal_no_case(b"foobar", b"F00"));
        assert!(!string_equal_no_case(b"foobar", b"foo"));
        assert!(!string_equal_no_case(b"foo", b"foobar"));
        assert!(string_begins_no_case(b"foobar", b"FoobAR --"));
        assert!(string_begins_no_case(b"foo", b"foobar"));
        assert!(!string_begins_no_case(b"foobar", b"F00"));
        assert!(!string_begins_no_case(b"foobar", b"foo"));
    }

    #[test]
    fn base64() {
        for (s, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64encode(s.as_bytes()), want);
            assert_eq!(base64decode(want.as_bytes()).unwrap(), s.as_bytes());
            assert_eq!(base64decode(want.trim_end_matches('=').as_bytes()).unwrap(), s.as_bytes());
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(base64decode(base64encode(&all).as_bytes()).unwrap(), all);
        for bad in ["Zg=a", "Z", "Zm9v Zg==", "Zm9v\nZg==", "Zm9v-Zg"] {
            assert_eq!(base64decode(bad.as_bytes()), None, "{bad}");
        }
    }

    #[test]
    fn base32() {
        let d = |s: &str| base32decode(s.as_bytes());
        assert_eq!(d(""), Some(vec![]));
        assert_eq!(d("MY======").unwrap(), b"f");
        assert_eq!(d("MZXQ====").unwrap(), b"fo");
        assert_eq!(d("MZXW6===").unwrap(), b"foo");
        assert_eq!(d("MZXW6YQ=").unwrap(), b"foob");
        assert_eq!(d("MZXW6YTB").unwrap(), b"fooba");
        assert_eq!(d("MZXW6YTBOI======").unwrap(), b"foobar");
        assert_eq!(d("MY").unwrap(), b"f");
        assert_eq!(d("MZXW6YQ").unwrap(), b"foob");
        assert_eq!(d("MZXW6YTBOI").unwrap(), b"foobar");
        assert_eq!(d("mZXw6yTBO1======").unwrap(), b"foobar");
        assert_eq!(d("mZXw6yTBO1{#&*()="), None);
    }

    #[test]
    fn escaping() {
        let test = b"!@#$%^&*()-_=+/,. %?";
        assert_eq!(escape_string(test), "!%40%23%24%25%5e%26*()-_%3d%2b%2f%2c.%20%25%3f");
        assert_eq!(escape_path(test), "!%40%23%24%25%5e%26*()-_%3d%2b/%2c.%20%25%3f");
        assert_eq!(unescape_string(escape_path(test).as_bytes()).unwrap(), test);
        assert_eq!(unescape_string(escape_string(test).as_bytes()).unwrap(), test);

        assert!(need_encoding(test));
        assert!(!need_encoding(b"!@$&()-_/,.%?"));
        assert!(need_encoding(b"\n"));

        for bad in ["%", "%0", "%GE", "%eg"] {
            assert_eq!(unescape_string(bad.as_bytes()), None, "{bad}");
        }
        assert_eq!(unescape_string(b"123+abc").unwrap(), b"123 abc");
    }

    #[test]
    fn split_and_trim() {
        assert_eq!(split_string("a b", ' '), ("a", "b"));
        assert_eq!(split_string("\"a b\" c", ' '), ("\"a", "b\" c"));
        assert_eq!(split_string("a\nb foobar", ' '), ("a\nb", "foobar"));
        assert_eq!(split_string("a b\"foo\"bar", '"'), ("a b", "foo\"bar"));
        assert_eq!(split_string("a", ' '), ("a", ""));
        assert_eq!(split_string("", ' '), ("", ""));

        assert_eq!(trim(""), "");
        assert_eq!(trim("\t "), "");
        assert_eq!(trim(" a"), "a");
        assert_eq!(trim("\t \na \t\r"), "a");
        assert_eq!(trim("a \t\n \tb"), "a \t\n \tb");
        assert_eq!(ltrim("  a "), "a ");

        let mut u = String::from("http://x");
        ensure_trailing_slash(&mut u);
        ensure_trailing_slash(&mut u);
        assert_eq!(u, "http://x/");
    }

    #[test]
    fn url_components() {
        let u = parse_url_components("http://user:pw@host.com:8080/a/b?c=d").unwrap();
        assert_eq!(
            u,
            UrlParts {
                protocol: "http".into(),
                auth: "user:pw".into(),
                host: "host.com".into(),
                port: Some(8080),
                path: "/a/b?c=d".into()
            }
        );
        let u = parse_url_components("udp://[2001:db8::1]:6969").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("2001:db8::1", Some(6969), "/"));
        let u = parse_url_components("http://host?x").unwrap();
        assert_eq!(u.path, "/?x");
        assert_eq!(parse_url_components("nope"), Err(UrlError::UnsupportedProtocol));
        assert_eq!(parse_url_components("http:/x"), Err(UrlError::UnsupportedProtocol));
        assert_eq!(parse_url_components("http://[::1"), Err(UrlError::ExpectedCloseBracket));
        assert_eq!(parse_url_components("http://h:8x/"), Err(UrlError::InvalidPort));
    }

    /// SEC-19: a percent-decoded magnet `tr=` must not smuggle lines into a request.
    #[test]
    fn url_control_characters_rejected() {
        for url in [
            "http://127.0.0.1:6379/announce\r\nCONFIG SET dir /tmp\r\n",
            "http://127.0.0.1:6379/announce\nX",
            "http://a.b/x\0y",
            "http://a.b/x\x7fy",
            "http://a\r\nb/announce",
            "http://u\n:p@a.b/",
        ] {
            assert_eq!(parse_url_components(url), Err(UrlError::InvalidCharacter), "{url:?}");
        }
        // trailing whitespace, common in announce lists, is trimmed rather than refused
        assert_eq!(parse_url_components("http://a.b/announce\r\n").unwrap().path, "/announce");
    }

    #[test]
    fn same_origin_cases() {
        // after libtorrent 2.0.13 test/test_http_parser.cpp (188c5226f7)
        // identical, and path/query/fragment differences don't matter
        assert!(same_origin("http://example.com/a", "http://example.com/a"));
        assert!(same_origin("http://example.com/a", "http://example.com/b?x=1#y"));
        assert!(same_origin("http://example.com", "http://example.com/b"));
        // scheme and host are case-insensitive
        assert!(same_origin("http://Example.COM/a", "HTTP://example.com/b"));
        // an explicit well-known port equals the implicit default
        assert!(same_origin("http://example.com/a", "http://example.com:80/b"));
        assert!(same_origin("https://example.com/a", "https://example.com:443/b"));
        // userinfo is not part of the origin
        assert!(same_origin("http://user:pass@example.com/a", "http://example.com/b"));

        // different host, scheme or port is a different origin
        assert!(!same_origin("http://example.com/a", "http://evil.com/a"));
        assert!(!same_origin("http://example.com/a", "http://example.com.evil.com/a"));
        assert!(!same_origin("http://example.com/a", "https://example.com/a"));
        assert!(!same_origin("http://example.com/a", "http://example.com:8080/a"));
        // an explicit non-default port differs from the implicit default
        assert!(!same_origin("https://example.com/a", "https://example.com:80/a"));
        // a redirect that smuggles a new host via userinfo is a different origin
        assert!(!same_origin("http://example.com/a", "http://example.com@evil.com/a"));
        // unparseable URLs are never same-origin
        assert!(!same_origin("not a url", "not a url"));
    }
}
