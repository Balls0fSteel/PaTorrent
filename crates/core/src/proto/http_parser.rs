// Ported from libtorrent-rasterbar 2.0.11: src/http_parser.cpp (the response header state
// machine of `incoming`, `parse_chunk_header`, `header_duration`, `is_ok_status`, `is_redirect`,
// `resolve_redirect_location`), src/parse_url.cpp (`split_url`)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the header is parsed in one call once its blank line is in the buffer (instead of
// line by line across calls); the body is left to the caller (web seeds use libtorrent's
// `dont_parse_chunks` mode and walk chunk headers themselves). Headers are a list of
// lower-cased names in arrival order instead of a multimap.

//! Incremental HTTP/1.x response parsing for streamed bodies (web seeds).

use crate::util::string::{ensure_trailing_slash, parse_url_components, string_begins_no_case};

/// Longest response header we buffer. libtorrent's web seed receive buffer is one block plus
/// `request_size_overhead` (5000 bytes) on top of what is already queued; headers beyond 16 KiB
/// are not something a file server sends.
pub const MAX_HEADER: usize = 16 * 1024;

/// Longest chunk header we buffer: the size line with its extensions, or for the last chunk
/// also its trailers. Servers send a few bytes; without a limit a server could stream an
/// endless header (SEC-22).
pub const MAX_CHUNK_HEADER: usize = 4 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The response is malformed (libtorrent `http_parser` `error_state`).
    Invalid(&'static str),
}

/// A parsed status line and header (libtorrent `http_parser` after `header_finished()`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResponseHead {
    pub protocol: String,
    /// `atoi` of the status field: 0 when it is not a number.
    pub status: i32,
    pub message: String,
    /// Lower-cased names, values with leading whitespace removed.
    pub headers: Vec<(String, String)>,
    /// -1 when unknown. A `Content-Range` sets it to the range's length.
    pub content_length: i64,
    /// The inclusive byte range of a `Content-Range` header.
    pub range: Option<(i64, i64)>,
    pub chunked: bool,
    /// `Connection: close`, or an HTTP/1.0 response.
    pub connection_close: bool,
}

impl ResponseHead {
    /// The first header named `name` (lower case), or "".
    pub fn header(&self, name: &str) -> &str {
        self.headers.iter().find(|(k, _)| k == name).map_or("", |(_, v)| v.as_str())
    }

    /// libtorrent `header_duration`: a positive number of seconds, else `None`.
    pub fn header_duration(&self, name: &str) -> Option<u32> {
        let v = atol(self.header(name));
        (v > 0).then(|| v.min(u32::MAX as i64) as u32)
    }
}

/// libtorrent `is_ok_status`: 200, 206 and every 3xx.
pub fn is_ok_status(status: i32) -> bool {
    status == 206 || status == 200 || (300..400).contains(&status)
}

/// libtorrent `is_redirect`.
pub fn is_redirect(status: i32) -> bool {
    (300..400).contains(&status)
}

/// One line ending in `\n` (a `\r` before it is dropped), and the position after it.
fn next_line(buf: &[u8], pos: usize) -> Option<(&[u8], usize)> {
    let nl = buf[pos..].iter().position(|&c| c == b'\n')? + pos;
    let mut end = nl;
    if end > pos && buf[end - 1] == b'\r' {
        end -= 1;
    }
    Some((&buf[pos..end], nl + 1))
}

/// libtorrent `read_until`: the text up to `delim` (or the end), skipping the delimiter.
fn read_until<'a>(s: &mut &'a [u8], delim: u8) -> &'a [u8] {
    let end = s.iter().position(|&c| c == delim).unwrap_or(s.len());
    let out = &s[..end];
    *s = s.get(end + 1..).unwrap_or_default();
    out
}

/// C `strtoll` on the start of `s`: leading whitespace, a sign, digits. Returns the value
/// (saturated at the `i64` limits, as `strtoll` does on overflow) and the bytes consumed (0 if
/// no digits were found).
fn strtoll(s: &[u8]) -> (i64, usize) {
    let mut i = 0;
    while i < s.len() && crate::util::string::is_space(s[i]) {
        i += 1;
    }
    let neg = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let start = i;
    let mut v: i64 = 0;
    let mut overflow = false;
    while i < s.len() && s[i].is_ascii_digit() {
        let d = (s[i] - b'0') as i64;
        match v.checked_mul(10).and_then(|v| if neg { v.checked_sub(d) } else { v.checked_add(d) }) {
            Some(n) => v = n,
            None => overflow = true,
        }
        i += 1;
    }
    if i == start {
        return (0, 0);
    }
    if overflow {
        v = if neg { i64::MIN } else { i64::MAX };
    }
    (v, i)
}

fn atol(s: &str) -> i64 {
    strtoll(s.as_bytes()).0
}

/// Parses a response header (libtorrent `http_parser::incoming` through `read_status` and
/// `read_header`). Returns `None` until the blank line that ends the header is in `buf`, then
/// the header and the offset of the body. A `100 Continue` response is skipped.
pub fn parse_head(buf: &[u8]) -> Result<Option<(ResponseHead, usize)>, ParseError> {
    let mut pos = 0;
    'response: loop {
        let Some((line, next)) = next_line(buf, pos) else {
            return if buf.len() > MAX_HEADER {
                Err(ParseError::Invalid("HTTP header too large"))
            } else {
                Ok(None)
            };
        };
        // libtorrent: an empty status line is an error
        if next == pos + 1 && buf[pos] == b'\n' {
            return Err(ParseError::Invalid("empty HTTP status line"));
        }
        pos = next;
        let mut head = ResponseHead { content_length: -1, ..ResponseHead::default() };
        let mut rest = line;
        let protocol = read_until(&mut rest, b' ');
        if !protocol.starts_with(b"HTTP/") {
            // libtorrent parses this as a request; a web seed or tracker never wants one
            return Err(ParseError::Invalid("not an HTTP response"));
        }
        head.protocol = String::from_utf8_lossy(protocol).into_owned();
        head.status = atol(&String::from_utf8_lossy(read_until(&mut rest, b' '))) as i32;
        head.message = String::from_utf8_lossy(read_until(&mut rest, b'\r')).into_owned();
        // HTTP 1.0 always closes the connection after each request
        head.connection_close = head.protocol == "HTTP/1.0";
        loop {
            let Some((line, next)) = next_line(buf, pos) else {
                return if buf.len() > MAX_HEADER {
                    Err(ParseError::Invalid("HTTP header too large"))
                } else {
                    Ok(None)
                };
            };
            pos = next;
            let Some(sep) = line.iter().position(|&c| c == b':') else {
                if head.status == 100 {
                    // for 100 Continue, we need to read another response header before
                    // reading the body
                    continue 'response;
                }
                // a blank line (or any line without a colon): the header is finished and the
                // body starts
                return Ok(Some((head, pos)));
            };
            let name = String::from_utf8_lossy(&line[..sep]).to_ascii_lowercase();
            let mut v = sep + 1;
            while v < line.len() && (line[v] == b' ' || line[v] == b'\t') {
                v += 1;
            }
            let value = String::from_utf8_lossy(&line[v..]).into_owned();
            match name.as_str() {
                "content-length" => {
                    let (n, _) = strtoll(value.as_bytes());
                    if n < 0 || n == i64::MAX {
                        return Err(ParseError::Invalid("invalid content-length"));
                    }
                    head.content_length = n;
                }
                "connection" => head.connection_close = string_begins_no_case(b"close", value.as_bytes()),
                "content-range" => {
                    let (start, end) = parse_content_range(value.as_bytes())?;
                    head.range = Some((start, end));
                    // the http range is inclusive
                    head.content_length = end - start + 1;
                }
                "transfer-encoding" => head.chunked = string_begins_no_case(b"chunked", value.as_bytes()),
                _ => {}
            }
            head.headers.push((name, value));
        }
    }
}

/// libtorrent's `content-range` branch: `[bytes ]start-end`, both non-negative, end >= start.
fn parse_content_range(v: &[u8]) -> Result<(i64, i64), ParseError> {
    let bad = Err(ParseError::Invalid("invalid content-range"));
    // apparently some web servers do not send the "bytes" in their content-range. Don't treat
    // it as an error if we can't find it, just assume the byte counters start immediately
    let mut p = v;
    if string_begins_no_case(b"bytes ", p) {
        p = &p[6..];
    }
    let (start, n) = strtoll(p);
    if start < 0 || start == i64::MAX || n == 0 || p.get(n) != Some(&b'-') {
        return bad;
    }
    let p = &p[n + 1..];
    let (end, n) = strtoll(p);
    if end < 0 || end == i64::MAX || n == 0 || end < start {
        return bad;
    }
    Ok((start, end))
}

/// A chunk header of a chunked body (libtorrent `parse_chunk_header`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chunk {
    /// More bytes are needed.
    Incomplete,
    /// A chunk of `size` bytes follows the `header_len` bytes of its header. Size 0 ends the
    /// body; its header then includes the trailer headers and the blank line after them.
    Header { size: u64, header_len: usize, trailers: Vec<(String, String)> },
}

/// libtorrent `parse_chunk_header`: one optional CRLF (the end of the previous chunk), the hex
/// size, an optional `;extension`, and for the last chunk the trailer headers up to a blank
/// line. A size that is not hex, or does not fit, is an error (libtorrent reports size -1), and
/// so is a header longer than [`MAX_CHUNK_HEADER`], complete or not (SEC-22).
pub fn parse_chunk_header(buf: &[u8]) -> Result<Chunk, ParseError> {
    let too_long = Err(ParseError::Invalid("chunk header too long"));
    match chunk_header(buf)? {
        Chunk::Incomplete if buf.len() > MAX_CHUNK_HEADER => too_long,
        Chunk::Header { header_len, .. } if header_len > MAX_CHUNK_HEADER => too_long,
        c => Ok(c),
    }
}

fn chunk_header(buf: &[u8]) -> Result<Chunk, ParseError> {
    let mut pos = 0;
    // ignore one optional new-line. This is since each chunk is terminated by a newline. we're
    // likely to see one before the actual header.
    if buf.first() == Some(&b'\r') {
        pos += 1;
    }
    if buf.get(pos) == Some(&b'\n') {
        pos += 1;
    }
    if pos == buf.len() {
        return Ok(Chunk::Incomplete);
    }
    let Some(nl) = buf[pos..].iter().position(|&c| c == b'\n') else { return Ok(Chunk::Incomplete) };
    let newline = pos + nl + 1;
    let mut size: u64 = 0;
    for &c in &buf[pos..newline] {
        if c == b'\r' || c == b'\n' {
            continue;
        }
        if c == b';' {
            break;
        }
        let digit = (c as char).to_digit(16).ok_or(ParseError::Invalid("invalid chunk size"))?;
        if size >= i64::MAX as u64 / 16 {
            return Err(ParseError::Invalid("invalid chunk size"));
        }
        size = size * 16 + digit as u64;
    }
    if size != 0 {
        return Ok(Chunk::Header { size, header_len: newline, trailers: Vec::new() });
    }
    // this is the terminator of the stream. Also read headers
    let mut trailers = Vec::new();
    let mut pos = newline;
    while let Some((line, next)) = next_line(buf, pos) {
        pos = next;
        match line.iter().position(|&c| c == b':') {
            None => return Ok(Chunk::Header { size: 0, header_len: pos, trailers }),
            Some(sep) => {
                let name = String::from_utf8_lossy(&line[..sep]).to_ascii_lowercase();
                let value = String::from_utf8_lossy(&line[sep + 1..]).trim_start().to_owned();
                trailers.push((name, value));
            }
        }
    }
    Ok(Chunk::Incomplete)
}

/// libtorrent `resolve_redirect_location`: a full URL is returned as is; an absolute path
/// replaces the referrer's path; a relative one replaces its last path element.
pub fn resolve_redirect_location(referrer: &str, location: &str) -> String {
    if location.is_empty() {
        return referrer.to_owned();
    }
    // if location is a full URL, just return it
    if parse_url_components(location).is_ok() {
        return location.to_owned();
    }
    let mut url = referrer.to_owned();
    // the referrer must have a URL scheme, otherwise return the location verbatim
    let Some(scheme) = url.find("://") else { return location.to_owned() };
    if location.starts_with('/') {
        // an absolute path: replace the path component of the referrer
        if let Some(i) = url[scheme + 3..].find('/') {
            url.truncate(scheme + 3 + i);
        }
        url.push_str(location);
    } else {
        // some web servers send out relative paths in the location header: remove the leaf
        // filename. If the last '/' is part of the scheme, there is no '/' in the path.
        if let Some(end) = url.rfind('/').filter(|&e| e > scheme + 2) {
            url.truncate(end);
        }
        ensure_trailing_slash(&mut url);
        url.push_str(location);
    }
    url
}

/// libtorrent `split_url`: `(scheme://[auth@]host[:port], /path)`. A URL that does not parse
/// is returned whole with an empty path, and an error.
pub fn split_url(url: &str) -> Result<(String, String), (String, String)> {
    // the first ':' must be followed by "//"
    let start = match url.find(':') {
        Some(i) if url[i + 1..].starts_with("//") => i + 3,
        _ => return Err((url.to_owned(), String::new())),
    };
    match url[start..].find('/') {
        Some(i) => Ok((url[..start + i].to_owned(), url[start + i..].to_owned())),
        None => Ok((url.to_owned(), String::new())),
    }
}

#[cfg(test)]
mod tests {
    //! After libtorrent-rasterbar 2.0.11 test/test_http_parser.cpp (the response, content-range,
    //! chunk header, split_url, resolve_redirect_location and is_ok_status cases).
    use super::*;

    fn head(s: &str) -> Result<Option<(ResponseHead, usize)>, ParseError> {
        parse_head(s.as_bytes())
    }

    #[test]
    fn status_and_headers() {
        let r = "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nX-Thing:   y\r\n\r\ntest";
        let (h, body) = head(r).unwrap().unwrap();
        assert_eq!((h.status, h.message.as_str(), h.content_length), (200, "OK", 4));
        assert_eq!(h.header("x-thing"), "y");
        assert_eq!(&r[body..], "test");
        assert!(!h.connection_close);
        // byte by byte, nothing until the blank line
        for n in 0..r.len() - 4 {
            assert_eq!(head(&r[..n]), Ok(None), "{n}");
        }
        let (h, _) = head("HTTP/1.0 200 OK\n\n").unwrap().unwrap();
        assert!(h.connection_close, "HTTP/1.0 closes");
        let (h, _) =
            head("HTTP/1.1 503 Busy\r\nConnection: Close\r\nRetry-After: 7\r\n\r\n").unwrap().unwrap();
        assert!(h.connection_close);
        assert_eq!(h.header_duration("retry-after"), Some(7));
        assert_eq!(h.header_duration("missing"), None);
        assert!(!h.chunked);
        assert!(matches!(head("SSH-2.0-OpenSSH\r\n\r\n"), Err(ParseError::Invalid(_))));
        assert!(matches!(head("\n"), Err(ParseError::Invalid(_))));
        let big = format!("HTTP/1.1 200 OK\r\nX: {}", "a".repeat(MAX_HEADER));
        assert!(matches!(head(&big), Err(ParseError::Invalid(_))));
    }

    #[test]
    fn content_range_case_insensitive() {
        let r = "HTTP/1.1 206 OK\ncontEnt-rAngE: bYTes 0-4\nconTent-TyPe: test/plain\n\n\ntest";
        let (h, body) = head(r).unwrap().unwrap();
        assert_eq!(h.range, Some((0, 4)));
        assert_eq!(h.content_length, 5);
        assert_eq!(&r[body..], "\ntest");
        // without "bytes "
        let (h, _) = head("HTTP/1.1 206 OK\nContent-Range: 10-19/100\n\n").unwrap().unwrap();
        assert_eq!((h.range, h.content_length), (Some((10, 19)), 10));
    }

    #[test]
    fn invalid_ranges_and_lengths() {
        for r in [
            "HTTP/1.1 206 OK\ncontent-range: bytes 4-0\ncontent-type: test/plain\n\n\ntest",
            "HTTP/1.1 206\ncontent-range: bytes 4-0\n\n",
            "HTTP/1.1\ncontent-range: bytes 4-0\n\n",
            "HTTP/1.1 206 OK\nContent-Range: bYTes -3-4\n\n",
            "HTTP/1.1 206 OK\nContent-Range: bYTes 3--434\n\n",
            "HTTP/1.1 206 OK\nContent-Range: bytes 0-999999999999999999999999\n\n",
            "HTTP/1.1 206 OK\nContent-Range: bytes 999999999999999999999999-0\n\n",
            "HTTP/1.1 206 OK\nContent-Range: bytes x-3\n\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: -45345\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 9999999999999999999999999999\r\n\r\n",
        ] {
            assert!(matches!(head(r), Err(ParseError::Invalid(_))), "{r:?}");
        }
    }

    #[test]
    fn one_hundred_continue() {
        let r = "HTTP/1.1 100 Continue\n\r\nHTTP/1.1 200 OK\nContent-Length: 4\r\nContent-Type: test/plain\r\n\r\ntest";
        let (h, body) = head(r).unwrap().unwrap();
        assert_eq!((h.status, h.content_length), (200, 4));
        assert_eq!(&r[body..], "test");
    }

    #[test]
    fn chunk_headers() {
        let h1 = b"f;this is a comment\r\n";
        assert_eq!(parse_chunk_header(&h1[..10]), Ok(Chunk::Incomplete));
        assert_eq!(
            parse_chunk_header(h1),
            Ok(Chunk::Header { size: 15, header_len: h1.len(), trailers: Vec::new() })
        );
        let h2 = b"0;this is a comment\r\ntest1: foo\r\ntest2: bar\r\n\r\n";
        let Ok(Chunk::Header { size: 0, header_len, trailers }) = parse_chunk_header(h2) else { panic!() };
        assert_eq!(header_len, h2.len());
        assert_eq!(trailers, [("test1".into(), "foo".into()), ("test2".into(), "bar".into())]);
        assert_eq!(parse_chunk_header(&h2[..h2.len() - 2]), Ok(Chunk::Incomplete));
        // the CRLF ending the previous chunk is skipped
        assert_eq!(
            parse_chunk_header(b"\r\n1a\r\nxyz"),
            Ok(Chunk::Header { size: 26, header_len: 6, trailers: Vec::new() })
        );
        assert_eq!(parse_chunk_header(b"\r\n"), Ok(Chunk::Incomplete));
        assert_eq!(parse_chunk_header(b"\n"), Ok(Chunk::Incomplete));
        assert!(parse_chunk_header(b"-53465234545\r\nfoobar").is_err());
        assert!(parse_chunk_header(b"ffffffffffffffffffff\r\n").is_err());
        assert!(parse_chunk_header(b"zz\r\n").is_err());
    }

    #[test]
    fn chunk_headers_are_limited() {
        // SEC-22: an endless extension, or endless trailers, fail once past the limit
        let mut ext = b"1;".to_vec();
        ext.resize(MAX_CHUNK_HEADER, b'x');
        assert_eq!(parse_chunk_header(&ext), Ok(Chunk::Incomplete));
        ext.push(b'x');
        assert!(parse_chunk_header(&ext).is_err());
        ext.extend_from_slice(b"\r\n");
        assert!(parse_chunk_header(&ext).is_err(), "complete, but too long");
        let mut trailers = b"0\r\n".to_vec();
        while trailers.len() <= MAX_CHUNK_HEADER {
            trailers.extend_from_slice(b"a: b\r\n");
        }
        assert!(parse_chunk_header(&trailers).is_err());
        assert!(parse_chunk_header(&trailers[..100]).is_ok());
    }

    #[test]
    fn split_urls() {
        let ok =
            |u: &str, a: &str, b: &str| assert_eq!(split_url(u), Ok((a.to_owned(), b.to_owned())), "{u}");
        ok("http://foo:bar@host.com:80/path/to/file", "http://foo:bar@host.com:80", "/path/to/file");
        ok("http://host.com/path/to/file", "http://host.com", "/path/to/file");
        ok("ftp://host.com:21/path/to/file", "ftp://host.com:21", "/path/to/file");
        ok("http://host.com/path?foo:bar@foo:", "http://host.com", "/path?foo:bar@foo:");
        ok("http://192.168.0.1/path/to/file", "http://192.168.0.1", "/path/to/file");
        ok("http://[2001:ff00::1]:42/path/to/file", "http://[2001:ff00::1]:42", "/path/to/file");
        ok("http://[2001:ff00::1]:42", "http://[2001:ff00::1]:42", "");
        ok("bla://[2001:ff00::1]:42/path/to/file", "bla://[2001:ff00::1]:42", "/path/to/file");
        for bad in ["foo:/[2001:ff00::1]:42/path/to/file", "foo:/", "//[2001:ff00::1]:42/path/to/file"] {
            assert_eq!(split_url(bad), Err((bad.to_owned(), String::new())));
        }
        assert_eq!(
            split_url("//host.com/path?foo:bar@foo:"),
            Err(("//host.com/path?foo:bar@foo:".to_owned(), String::new()))
        );
    }

    #[test]
    fn redirect_locations() {
        let r = resolve_redirect_location;
        assert_eq!(r("http://example.com/a/b", "a"), "http://example.com/a/a");
        assert_eq!(r("http://example.com/a/b", "c/d/e/"), "http://example.com/a/c/d/e/");
        assert_eq!(r("http://example.com/a/b", "../a"), "http://example.com/a/../a");
        assert_eq!(r("http://example.com/a/b", "/c"), "http://example.com/c");
        assert_eq!(r("http://example.com/a/b", "http://test.com/d"), "http://test.com/d");
        assert_eq!(r("my-custom-scheme://example.com/a/b", "http://test.com/d"), "http://test.com/d");
        assert_eq!(r("http://example.com", "/d"), "http://example.com/d");
        assert_eq!(r("http://example.com", "d"), "http://example.com/d");
        assert_eq!(r("my-custom-scheme://example.com/a/b", "/d"), "my-custom-scheme://example.com/d");
        assert_eq!(r("my-custom-scheme://example.com/a/b", "c/d"), "my-custom-scheme://example.com/a/c/d");
        // if the referrer is invalid, just respond the verbatim location
        assert_eq!(r("example.com/a/b", "/c/d"), "/c/d");
        assert_eq!(r("http://example.com/a", ""), "http://example.com/a");
    }

    #[test]
    fn ok_status() {
        for (s, ok) in [(200, true), (206, true), (299, false), (300, true), (399, true), (400, false)] {
            assert_eq!(is_ok_status(s), ok, "{s}");
        }
        assert!(is_redirect(302) && !is_redirect(200) && !is_redirect(400));
    }
}
