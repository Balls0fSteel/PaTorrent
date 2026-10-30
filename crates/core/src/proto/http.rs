// Ported from libtorrent-rasterbar 2.0.11: src/http_parser.cpp (response parsing, chunked
// transfer encoding), src/http_connection.cpp (request format, redirects)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the caller keeps the whole response in one buffer (trackers and UPnP only need
// whole, small bodies) and `ResponseParser` resumes where it stopped after each read, as
// libtorrent's `http_parser` does. Responses are capped at `MAX_RESPONSE`.

//! Minimal HTTP/1.1 client protocol for trackers.

use super::http_parser::MAX_HEADER;
use crate::util::string::{UrlParts, parse_url_components};

/// Largest tracker response we accept (libtorrent `max_http_recv_buffer_size` is 6 MB for
/// trackers; peer lists are far smaller).
pub const MAX_RESPONSE: usize = 4 * 1024 * 1024;

/// libtorrent `http_connection` follows up to 5 redirects by default.
pub const MAX_REDIRECTS: u32 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// Path and query, starting with `/`.
    pub path: String,
}

impl Url {
    pub fn parse(url: &str) -> Option<Url> {
        let UrlParts { protocol, host, port, path, .. } = parse_url_components(url).ok()?;
        let scheme = protocol.to_ascii_lowercase();
        let default_port = match scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => return None,
        };
        if host.is_empty() {
            return None;
        }
        Some(Url {
            scheme,
            host,
            port: port.unwrap_or(default_port),
            path: if path.is_empty() { "/".into() } else { path },
        })
    }

    /// Host header value: the port is included only when it is not the default.
    pub fn host_header(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let default = matches!((self.scheme.as_str(), self.port), ("http", 80) | ("https", 443));
        if default { host } else { format!("{host}:{}", self.port) }
    }
}

/// A `GET` request (libtorrent `http_connection::get`). We never ask for compression, so
/// trackers answer with identity encoding and no inflater is needed.
pub fn get_request(url: &Url, user_agent: &str) -> Vec<u8> {
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        url.path,
        url.host_header(),
        user_agent
    )
    .into_bytes()
}

/// A `GET` request to an HTTP proxy for a plain `http://` URL (libtorrent `http_connection::get`
/// with an HTTP proxy): the absolute URL as the target, basic authentication for the proxy.
pub fn proxy_get_request(url: &Url, user_agent: &str, proxy_auth: Option<(&str, &str)>) -> Vec<u8> {
    let mut r = format!("GET {}://{}{} HTTP/1.1\r\n", url.scheme, url.host_header(), url.path);
    if let Some((user, pass)) = proxy_auth {
        r.push_str("Proxy-Authorization: Basic ");
        r.push_str(&crate::util::string::base64encode(format!("{user}:{pass}").as_bytes()));
        r.push_str("\r\n");
    }
    r.push_str(&format!(
        "Host: {}\r\nUser-Agent: {}\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        url.host_header(),
        user_agent
    ));
    r.into_bytes()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub message: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn is_redirect(&self) -> bool {
        matches!(self.status, 301 | 302 | 303 | 307 | 308)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The response is not complete yet.
    Incomplete,
    Invalid(&'static str),
}

/// Parses a response. With `eof`, a body without `Content-Length` or chunked framing ends at
/// the end of the buffer (the server closed the connection).
///
/// One-shot: a caller that reads the response piece by piece keeps a [`ResponseParser`]
/// instead, so each read costs only the bytes it added. Only the tests and the fuzz target
/// parse in one go.
#[cfg(any(test, feature = "fuzzing"))]
pub fn parse_response(buf: &[u8], eof: bool) -> Result<Response, ParseError> {
    parse_response_limited(buf, eof, MAX_RESPONSE)
}

/// [`parse_response`] with a body limit other than [`MAX_RESPONSE`].
#[cfg(any(test, feature = "fuzzing"))]
pub fn parse_response_limited(buf: &[u8], eof: bool, max: usize) -> Result<Response, ParseError> {
    ResponseParser::new(max).parse(buf, eof)
}

/// How the body of a parsed head ends.
#[derive(Clone, Copy, Debug)]
enum Framing {
    Chunked,
    Length(usize),
    /// At the end of the connection (or right away, for 204 and 304).
    Eof,
}

/// Where the chunked decoder is (libtorrent's `http_parser` keeps the same state in
/// `m_chunked_size`, `m_chunk_header_size` and `m_partial_chunk_header`).
#[derive(Clone, Copy, Debug)]
enum Chunk {
    /// Looking for the end of the chunk-size line that starts at the parser's position; the
    /// bytes before `scan` were searched already.
    Header { scan: usize },
    /// `left` bytes of chunk data still to copy.
    Data { left: usize },
    /// `left` bytes of the CRLF ending a chunk's data still to skip.
    Crlf { left: usize },
}

/// A buffered HTTP response parser that keeps its place across reads, like libtorrent's
/// `http_parser::incoming`. The caller appends what it reads to one buffer and passes the whole
/// buffer each time; the parser resumes where it stopped, so a body trickled in one byte at a
/// time costs O(n) in total. Before (security review 2026-10-22) the whole chunked body was
/// decoded again after every read: O(n²) up to [`MAX_RESPONSE`].
#[derive(Debug)]
pub struct ResponseParser {
    max: usize,
    /// Where the next search for the end of the header starts.
    head_scan: usize,
    /// The parsed head, the offset of the body, and how the body ends. The response's `body`
    /// collects the decoded chunks.
    head: Option<(Response, usize, Framing)>,
    /// The offset in the buffer up to which the chunked body is decoded.
    pos: usize,
    chunk: Chunk,
    /// Bytes searched or copied so far (the tests bound the work per read with it).
    work: usize,
}

impl Default for ResponseParser {
    fn default() -> ResponseParser {
        ResponseParser::new(MAX_RESPONSE)
    }
}

impl ResponseParser {
    /// A parser for a body of at most `max` bytes.
    pub fn new(max: usize) -> ResponseParser {
        ResponseParser { max, head_scan: 0, head: None, pos: 0, chunk: Chunk::Header { scan: 0 }, work: 0 }
    }

    /// Bytes of the buffer searched or copied so far, over every call.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn bytes_scanned(&self) -> usize {
        self.work
    }

    /// Parses `buf`, the whole response read so far: the bytes passed to the previous calls
    /// followed by any new ones (the buffer only grows; a new connection needs a new parser).
    /// With `eof`, a body without `Content-Length` or chunked framing ends at the end of the
    /// buffer. Returns the response once it is complete; an `Invalid` error is final.
    pub fn parse(&mut self, buf: &[u8], eof: bool) -> Result<Response, ParseError> {
        let (body_start, framing, status) = match &self.head {
            Some((resp, body_start, framing)) => (*body_start, *framing, resp.status),
            None => {
                let head = self.parse_head(buf, eof)?;
                let r = (head.1, head.2, head.0.status);
                self.head = Some(head);
                r
            }
        };
        let rest = buf.get(body_start..).unwrap_or_default();
        let body = match framing {
            Framing::Chunked => {
                self.decode_chunked(buf, eof)?;
                return Ok(self.take_response());
            }
            Framing::Length(len) => {
                if rest.len() < len {
                    return Err(if eof {
                        ParseError::Invalid("truncated HTTP body")
                    } else {
                        ParseError::Incomplete
                    });
                }
                rest[..len].to_vec()
            }
            Framing::Eof if eof || status == 204 || status == 304 => {
                if rest.len() > self.max {
                    return Err(ParseError::Invalid("HTTP response too large"));
                }
                rest.to_vec()
            }
            Framing::Eof => return Err(ParseError::Incomplete),
        };
        self.work += body.len();
        let mut resp = self.take_response();
        resp.body = body;
        Ok(resp)
    }

    fn take_response(&mut self) -> Response {
        let Some((resp, ..)) = self.head.as_mut() else { unreachable!("the head is parsed") };
        Response {
            status: resp.status,
            message: std::mem::take(&mut resp.message),
            headers: std::mem::take(&mut resp.headers),
            body: std::mem::take(&mut resp.body),
        }
    }

    fn parse_head(&mut self, buf: &[u8], eof: bool) -> Result<(Response, usize, Framing), ParseError> {
        let from = self.head_scan.min(buf.len());
        let found = find(&buf[from..], b"\r\n\r\n");
        // what the search looked at: up to the end of the marker, or all of the new part
        self.work += found.map_or(buf.len() - from, |i| i + 4);
        let Some(head_end) = found.map(|i| i + from) else {
            // the end marker may straddle this read and the next: back up three bytes
            self.head_scan = buf.len().saturating_sub(3);
            // Without a cap a server that never ends the header would keep us reading up to
            // `max`. Same cap as the incremental parser (`http_parser::MAX_HEADER`).
            if buf.len() > MAX_HEADER {
                return Err(ParseError::Invalid("HTTP header too large"));
            }
            return Err(if eof {
                ParseError::Invalid("truncated HTTP header")
            } else {
                ParseError::Incomplete
            });
        };
        if head_end > MAX_HEADER {
            return Err(ParseError::Invalid("HTTP header too large"));
        }
        let head =
            std::str::from_utf8(&buf[..head_end]).map_err(|_| ParseError::Invalid("invalid HTTP header"))?;
        let mut lines = head.split("\r\n");
        let status_line = lines.next().unwrap_or_default();
        let mut parts = status_line.splitn(3, ' ');
        let proto = parts.next().unwrap_or_default();
        if !proto.starts_with("HTTP/") {
            return Err(ParseError::Invalid("not an HTTP response"));
        }
        let status = parts
            .next()
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or(ParseError::Invalid("invalid HTTP status"))?;
        let message = parts.next().unwrap_or_default().to_owned();
        let mut headers = Vec::new();
        for line in lines {
            // a bare CR or LF would carry into a redirect's request line (SEC-19)
            if line.contains(['\r', '\n']) {
                return Err(ParseError::Invalid("invalid HTTP header"));
            }
            if let Some((k, v)) = line.split_once(':') {
                headers.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
        let resp = Response { status, message, headers, body: Vec::new() };
        let chunked =
            resp.header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
        let framing = if matches!(status, 204 | 304) {
            // no body, whatever the head says: a 304's Content-Length is the cached copy's
            // (RFC 9112 6.3)
            Framing::Eof
        } else if chunked {
            Framing::Chunked
        } else if let Some(len) = resp.header("content-length") {
            let len: usize = len.parse().map_err(|_| ParseError::Invalid("invalid content-length"))?;
            if len > self.max {
                return Err(ParseError::Invalid("HTTP response too large"));
            }
            Framing::Length(len)
        } else {
            Framing::Eof
        };
        let body_start = head_end + 4;
        self.pos = body_start;
        self.chunk = Chunk::Header { scan: body_start };
        Ok((resp, body_start, framing))
    }

    /// Decodes the chunks that arrived since the last call into the response's body; `Ok`
    /// once the last (zero-size) chunk's header is in.
    fn decode_chunked(&mut self, buf: &[u8], eof: bool) -> Result<(), ParseError> {
        let incomplete =
            || if eof { ParseError::Invalid("truncated chunked body") } else { ParseError::Incomplete };
        let max = self.max;
        let Some((resp, ..)) = self.head.as_mut() else { unreachable!("the head is parsed") };
        let out = &mut resp.body;
        loop {
            match self.chunk {
                Chunk::Header { scan } => {
                    // the CRLF may straddle this read and the last: back up one byte
                    let from = scan.saturating_sub(1).max(self.pos).min(buf.len());
                    let found = find(&buf[from..], b"\r\n");
                    self.work += found.map_or(buf.len() - from, |i| i + 2);
                    let Some(line_end) = found.map(|i| i + from) else {
                        self.chunk = Chunk::Header { scan: buf.len() };
                        return Err(incomplete());
                    };
                    let line = std::str::from_utf8(&buf[self.pos..line_end])
                        .map_err(|_| ParseError::Invalid("invalid chunk header"))?;
                    let size_str = line.split(';').next().unwrap_or_default().trim();
                    let size = usize::from_str_radix(size_str, 16)
                        .map_err(|_| ParseError::Invalid("invalid chunk size"))?;
                    if size == 0 {
                        self.pos = line_end + 2;
                        return Ok(());
                    }
                    if size > max || out.len() + size > max {
                        return Err(ParseError::Invalid("HTTP response too large"));
                    }
                    self.pos = line_end + 2;
                    self.chunk = Chunk::Data { left: size };
                }
                Chunk::Data { left } => {
                    let n = left.min(buf.len().saturating_sub(self.pos));
                    out.extend_from_slice(&buf[self.pos..self.pos + n]);
                    self.work += n;
                    self.pos += n;
                    if n < left {
                        self.chunk = Chunk::Data { left: left - n };
                        return Err(incomplete());
                    }
                    self.chunk = Chunk::Crlf { left: 2 };
                }
                Chunk::Crlf { left } => {
                    // skipped unread, as the whole-buffer decoder always did
                    let n = left.min(buf.len().saturating_sub(self.pos));
                    self.pos += n;
                    if n < left {
                        self.chunk = Chunk::Crlf { left: left - n };
                        return Err(incomplete());
                    }
                    self.chunk = Chunk::Header { scan: self.pos };
                }
            }
        }
    }
}

// Resolves a `Location` header against the request URL (absolute, or absolute-path).
pub fn resolve_redirect(base: &Url, location: &str) -> Option<String> {
    if location.contains("://") {
        return Some(location.to_owned());
    }
    let origin = format!("{}://{}", base.scheme, base.host_header());
    if location.starts_with('/') {
        return Some(format!("{origin}{location}"));
    }
    let dir = base.path.split('?').next().unwrap_or("/");
    let dir = &dir[..dir.rfind('/').map_or(0, |i| i + 1)];
    Some(format!("{origin}{dir}{location}"))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u = Url::parse("http://tracker.example.org:6969/announce?x=1").unwrap();
        assert_eq!(
            (u.scheme.as_str(), u.host.as_str(), u.port, u.path.as_str()),
            ("http", "tracker.example.org", 6969, "/announce?x=1")
        );
        assert_eq!(u.host_header(), "tracker.example.org:6969");
        let u = Url::parse("https://example.org").unwrap();
        assert_eq!((u.port, u.path.as_str(), u.host_header().as_str()), (443, "/", "example.org"));
        assert!(Url::parse("udp://example.org:80").is_none());
        assert!(Url::parse("ftp://example.org/").is_none());
        let req = String::from_utf8(get_request(&Url::parse("http://a.b/x").unwrap(), "UA/1")).unwrap();
        assert!(req.starts_with("GET /x HTTP/1.1\r\nHost: a.b\r\nUser-Agent: UA/1\r\n"));
        assert!(req.ends_with("\r\n\r\n"));
        let u = Url::parse("http://a.b:8080/ann?k=1").unwrap();
        let req = String::from_utf8(proxy_get_request(&u, "UA/1", Some(("Aladdin", "open sesame")))).unwrap();
        assert_eq!(
            req,
            "GET http://a.b:8080/ann?k=1 HTTP/1.1\r\nProxy-Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==\r\n\
             Host: a.b:8080\r\nUser-Agent: UA/1\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n"
        );
        let req =
            String::from_utf8(proxy_get_request(&Url::parse("http://[::1]/a").unwrap(), "U", None)).unwrap();
        assert!(req.starts_with("GET http://[::1]/a HTTP/1.1\r\nHost: [::1]\r\n"));
    }

    // In the spirit of libtorrent test_http_parser.cpp: content-length, chunked, EOF framing.
    #[test]
    fn responses() {
        let r = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nX: y\r\n\r\nabcdEXTRA";
        let resp = parse_response(r, false).unwrap();
        assert_eq!((resp.status, resp.message.as_str(), &resp.body[..]), (200, "OK", &b"abcd"[..]));
        assert_eq!(resp.header("x"), Some("y"));
        assert_eq!(parse_response(&r[..40], false), Err(ParseError::Incomplete));
        assert!(matches!(
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nab", true),
            Err(ParseError::Invalid(_))
        ));

        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5;ext=1\r\npedia\r\n0\r\n\r\n";
        assert_eq!(parse_response(chunked, false).unwrap().body, b"Wikipedia");
        assert_eq!(parse_response(&chunked[..60], false), Err(ParseError::Incomplete));

        let eof = b"HTTP/1.0 404 Not Found\r\nServer: x\r\n\r\nnope";
        assert_eq!(parse_response(eof, false), Err(ParseError::Incomplete));
        let resp = parse_response(eof, true).unwrap();
        assert_eq!((resp.status, &resp.body[..]), (404, &b"nope"[..]));
        // a 304's Content-Length (the cached copy's) frames no body
        let resp =
            parse_response(b"HTTP/1.1 304 Not Modified\r\nContent-Length: 4096\r\n\r\n", false).unwrap();
        assert_eq!((resp.status, resp.body.len()), (304, 0));
        assert!(matches!(parse_response(b"SSH-2.0\r\n\r\n", true), Err(ParseError::Invalid(_))));
    }

    /// SEC-19: neither a magnet's tracker nor a tracker's redirect can put a line break into the
    /// request line we send (which would speak to Redis or similar on 127.0.0.1).
    #[test]
    fn no_line_breaks_reach_a_request() {
        let magnet = crate::metainfo::magnet::parse_magnet_uri(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567\
             &tr=http%3A%2F%2F127.0.0.1%3A6379%2Fannounce%0D%0ACONFIG%20SET%20dir%20%2Ftmp%0D%0A",
        )
        .unwrap();
        assert!(magnet.trackers[0].contains("\r\n"));
        assert!(Url::parse(&magnet.trackers[0]).is_none());

        let redirect = b"HTTP/1.1 302 Found\r\nLocation: /announce\nFLUSHALL\r\n\r\n";
        assert!(matches!(parse_response(redirect, true), Err(ParseError::Invalid(_))));
        let base = Url::parse("http://127.0.0.1:6379/announce").unwrap();
        let next = resolve_redirect(&base, "/announce\nFLUSHALL").unwrap();
        assert!(Url::parse(&next).is_none());
    }

    /// Security review 2026-10-22: a header that never ends is refused once it passes
    /// `MAX_HEADER`, instead of being searched again after every read up to `MAX_RESPONSE`.
    #[test]
    fn an_endless_header_is_refused() {
        let mut r = b"HTTP/1.1 200 OK\r\n".to_vec();
        while r.len() <= MAX_HEADER {
            r.extend_from_slice(b"X-Padding: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
        }
        assert_eq!(parse_response(&r, false), Err(ParseError::Invalid("HTTP header too large")));
        r.extend_from_slice(b"\r\nbody");
        assert_eq!(parse_response(&r, true), Err(ParseError::Invalid("HTTP header too large")));
        // a header just under the cap still parses
        let mut ok = b"HTTP/1.1 200 OK\r\n".to_vec();
        while ok.len() < MAX_HEADER - 100 {
            ok.extend_from_slice(b"X: y\r\n");
        }
        ok.extend_from_slice(b"\r\nbody");
        assert_eq!(parse_response(&ok, true).unwrap().body, b"body");
    }

    const TRICKLED: &[u8] = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nX: y\r\n\r\n\
        4\r\nWiki\r\n5;ext=1\r\npedia\r\n0e\r\n in\r\n\r\nchunks.\r\n0\r\n\r\n";

    /// Feeds `resp` to one parser `step` bytes at a time, as the tracker and UPnP loops do.
    fn trickle(resp: &[u8], step: usize) -> (Result<Response, ParseError>, usize, ResponseParser) {
        let mut p = ResponseParser::default();
        let mut reads = 0;
        for end in (step..resp.len()).step_by(step).chain([resp.len()]) {
            reads += 1;
            match p.parse(&resp[..end], false) {
                Err(ParseError::Incomplete) => {}
                r => return (r, reads, p),
            }
        }
        (Err(ParseError::Incomplete), reads, p)
    }

    /// Security review 2026-10-22: a chunked body trickled in one byte at a time decodes to the
    /// same body as the whole response at once, CRLFs split across reads included.
    #[test]
    fn a_body_trickled_one_byte_at_a_time_decodes() {
        let whole = parse_response(TRICKLED, false).unwrap();
        assert_eq!(whole.body, b"Wikipedia in\r\n\r\nchunks.");
        for step in 1..8 {
            let (r, _, _) = trickle(TRICKLED, step);
            assert_eq!(r.unwrap(), whole, "step {step}");
        }
        // content-length and end-of-connection framing through the same parser
        let cl = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabcd";
        assert_eq!(trickle(cl, 1).0.unwrap().body, b"abcd");
        let mut p = ResponseParser::default();
        let eof = b"HTTP/1.0 200 OK\r\n\r\nrest";
        assert_eq!(p.parse(&eof[..10], false), Err(ParseError::Incomplete));
        assert_eq!(p.parse(eof, false), Err(ParseError::Incomplete));
        assert_eq!(p.parse(eof, true).unwrap().body, b"rest");
        // errors are still errors when trickled
        let bad = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\nzz\r\n";
        assert_eq!(trickle(bad, 1).0, Err(ParseError::Invalid("invalid chunk size")));
        let mut p = ResponseParser::default();
        assert_eq!(
            p.parse(&TRICKLED[..TRICKLED.len() - 9], true),
            Err(ParseError::Invalid("truncated chunked body"))
        );
    }

    /// Security review 2026-10-22: the work per read is bounded by what the read added. A large
    /// chunked body (1 MiB in 1 KiB chunks) trickled in 100-byte reads scans each byte a small,
    /// constant number of times; re-decoding it after every read would scan ~5 GiB.
    #[test]
    fn the_work_per_read_is_bounded() {
        let mut resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        let mut body = Vec::new();
        for i in 0..1024u32 {
            let chunk: Vec<u8> = (0..1024u32).map(|j| (i ^ j) as u8).collect();
            resp.extend_from_slice(b"400\r\n");
            resp.extend_from_slice(&chunk);
            resp.extend_from_slice(b"\r\n");
            body.extend_from_slice(&chunk);
        }
        resp.extend_from_slice(b"0\r\n\r\n");
        let (r, reads, p) = trickle(&resp, 100);
        assert_eq!(r.unwrap().body, body);
        assert!(reads > 10_000);
        // every byte copied once, chunk headers and the head searched about once
        assert!(p.bytes_scanned() <= resp.len() + 4 * reads, "{} for {}", p.bytes_scanned(), resp.len());
        // the same for a header trickled in
        let (_, reads, p) = trickle(&resp[..40], 1);
        assert!(p.bytes_scanned() <= 40 + 3 * reads);
    }

    #[test]
    fn redirects() {
        let base = Url::parse("http://a.b:81/dir/announce?x").unwrap();
        assert_eq!(resolve_redirect(&base, "https://c.d/e").unwrap(), "https://c.d/e");
        assert_eq!(resolve_redirect(&base, "/other").unwrap(), "http://a.b:81/other");
        assert_eq!(resolve_redirect(&base, "rel?y").unwrap(), "http://a.b:81/dir/rel?y");
    }
}
