//! Sans-IO HTTP/1.1 server side (RFC 9110 / 9112): the request head parser, request bodies
//! (`application/x-www-form-urlencoded`, `multipart/form-data`), cookies, and responses.
//!
//! Everything here reads untrusted bytes from the network and is fuzzed (`fuzz/nox_http`).
//! Bodies are sized by `Content-Length` only: chunked request bodies are refused with 501, which
//! RFC 9112 section 6.3 allows for a server that does not implement them.

use std::fmt::Write as _;

/// Size limits for one request.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Request line plus headers.
    pub max_head: usize,
    pub max_headers: usize,
    pub max_body: usize,
    /// Largest WebSocket message a client may send (subscriptions are a few hundred bytes).
    pub max_message: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        // The body limit is the size of the largest .torrent qBittorrent accepts by default.
        Limits { max_head: 16 * 1024, max_headers: 100, max_body: 100 * 1024 * 1024, max_message: 64 * 1024 }
    }
}

/// Why a request was refused before it reached the API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpError {
    BadRequest(&'static str),
    HeadTooLarge,
    BodyTooLarge,
    /// A `Transfer-Encoding` (chunked) request body.
    NotImplemented,
    VersionNotSupported,
}

impl HttpError {
    pub fn status(self) -> u16 {
        match self {
            HttpError::BadRequest(_) => 400,
            HttpError::HeadTooLarge => 431,
            HttpError::BodyTooLarge => 413,
            HttpError::NotImplemented => 501,
            HttpError::VersionNotSupported => 505,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            HttpError::BadRequest(why) => why,
            HttpError::HeadTooLarge => "request header too large",
            HttpError::BodyTooLarge => "request body too large",
            HttpError::NotImplemented => "chunked request bodies are not supported",
            HttpError::VersionNotSupported => "only HTTP/1.0 and HTTP/1.1 are supported",
        }
    }
}

/// A parsed request line and header block.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Head {
    pub method: String,
    /// The percent-decoded path, without the query.
    pub path: String,
    /// The raw query string (after `?`), not decoded.
    pub query: String,
    /// 0 for HTTP/1.0, 1 for HTTP/1.1.
    pub minor: u8,
    /// Header names in lower case, values trimmed.
    pub headers: Vec<(String, String)>,
    pub content_length: usize,
    pub keep_alive: bool,
    pub expect_continue: bool,
}

impl Head {
    /// The first header named `name` (lower case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

fn is_tchar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}

/// Position just after the blank line ending the head (CRLF CRLF, or bare LF line ends).
fn head_end(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    while let Some(off) = buf[i..].iter().position(|&c| c == b'\n') {
        let nl = i + off;
        let next = nl + 1;
        if buf.get(next) == Some(&b'\n') {
            return Some(next + 1);
        }
        if buf.get(next) == Some(&b'\r') && buf.get(next + 1) == Some(&b'\n') {
            return Some(next + 2);
        }
        i = next;
    }
    None
}

/// Parses a request head from the start of `buf`. `Ok(None)`: not complete yet. On success,
/// also returns how many bytes the head took; the body (`content_length` bytes) follows.
pub fn parse_head(buf: &[u8], limits: &Limits) -> Result<Option<(Head, usize)>, HttpError> {
    // RFC 9112 section 2.2: ignore empty lines before the request line.
    let skip = buf.iter().take_while(|&&c| c == b'\r' || c == b'\n').count();
    let rest = &buf[skip..];
    let Some(end) = head_end(rest) else {
        return if buf.len() > limits.max_head { Err(HttpError::HeadTooLarge) } else { Ok(None) };
    };
    if skip + end > limits.max_head {
        return Err(HttpError::HeadTooLarge);
    }
    let text = &rest[..end];
    let mut lines = text.split(|&c| c == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l));
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(|&c| c == b' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(HttpError::BadRequest("malformed request line"));
    };
    if method.is_empty() || !method.iter().all(|&c| is_tchar(c)) {
        return Err(HttpError::BadRequest("bad method"));
    }
    let minor = match version {
        b"HTTP/1.1" => 1,
        b"HTTP/1.0" => 0,
        v if v.starts_with(b"HTTP/") => return Err(HttpError::VersionNotSupported),
        _ => return Err(HttpError::BadRequest("bad version")),
    };
    if target.is_empty() || target.iter().any(|&c| c <= b' ' || c >= 0x7f) {
        return Err(HttpError::BadRequest("bad request target"));
    }
    // origin-form, or absolute-form (RFC 9112 section 3.2.2), of which only the path is used
    let target = match target {
        [b'/', ..] => target,
        t if t.len() > 7 && t[..7].eq_ignore_ascii_case(b"http://") => {
            let after = &t[7..];
            match after.iter().position(|&c| c == b'/') {
                Some(p) => &after[p..],
                None => b"/",
            }
        }
        _ => return Err(HttpError::BadRequest("bad request target")),
    };
    let (path, query) = match target.iter().position(|&c| c == b'?') {
        Some(q) => (&target[..q], &target[q + 1..]),
        None => (target, &b""[..]),
    };
    let path = String::from_utf8(percent_decode(path, false))
        .map_err(|_| HttpError::BadRequest("path is not UTF-8"))?;
    // Only ASCII is left in the target.
    let query = String::from_utf8_lossy(query).into_owned();

    let mut head =
        Head { method: String::from_utf8_lossy(method).into_owned(), path, query, minor, ..Head::default() };
    let mut content_length: Option<usize> = None;
    let mut connection_close = false;
    let mut connection_keep_alive = false;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if line[0] == b' ' || line[0] == b'\t' {
            return Err(HttpError::BadRequest("obsolete line folding"));
        }
        let colon =
            line.iter().position(|&c| c == b':').ok_or(HttpError::BadRequest("header without ':'"))?;
        let name = &line[..colon];
        if name.is_empty() || !name.iter().all(|&c| is_tchar(c)) {
            return Err(HttpError::BadRequest("bad header name"));
        }
        let value = line[colon + 1..].trim_ascii();
        if value.iter().any(|&c| (c < b' ' && c != b'\t') || c == 0x7f) {
            return Err(HttpError::BadRequest("control character in header"));
        }
        if head.headers.len() >= limits.max_headers {
            return Err(HttpError::HeadTooLarge);
        }
        let name = String::from_utf8_lossy(name).to_ascii_lowercase();
        let value = String::from_utf8_lossy(value).into_owned();
        match name.as_str() {
            "content-length" => {
                // A list of identical values is allowed (RFC 9110 section 8.6); anything else
                // is a smuggling attempt.
                for v in value.split(',') {
                    let v = v.trim();
                    if v.is_empty() || v.len() > 18 || !v.bytes().all(|c| c.is_ascii_digit()) {
                        return Err(HttpError::BadRequest("bad Content-Length"));
                    }
                    let n: usize = v.parse().map_err(|_| HttpError::BadRequest("bad Content-Length"))?;
                    if content_length.is_some_and(|c| c != n) {
                        return Err(HttpError::BadRequest("conflicting Content-Length"));
                    }
                    content_length = Some(n);
                }
            }
            "transfer-encoding" => return Err(HttpError::NotImplemented),
            "host" if head.header("host").is_some() => return Err(HttpError::BadRequest("duplicate Host")),
            "connection" => {
                for t in value.split(',') {
                    let t = t.trim();
                    connection_close |= t.eq_ignore_ascii_case("close");
                    connection_keep_alive |= t.eq_ignore_ascii_case("keep-alive");
                }
            }
            "expect" => {
                if value.eq_ignore_ascii_case("100-continue") {
                    head.expect_continue = true;
                } else {
                    return Err(HttpError::BadRequest("unsupported Expect"));
                }
            }
            _ => {}
        }
        head.headers.push((name, value));
    }
    if minor == 1 && head.header("host").is_none() {
        return Err(HttpError::BadRequest("missing Host"));
    }
    head.content_length = content_length.unwrap_or(0);
    if head.content_length > limits.max_body {
        return Err(HttpError::BodyTooLarge);
    }
    head.keep_alive = if minor == 1 { !connection_close } else { connection_keep_alive && !connection_close };
    head.expect_continue &= minor == 1;
    Ok(Some((head, skip + end)))
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decodes `%XX` escapes (and `+` as a space when `plus` is set). A `%` not followed by two
/// hex digits stays as it is.
pub fn percent_decode(s: &[u8], plus: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' => {
                match (s.get(i + 1).copied().and_then(hex_val), s.get(i + 2).copied().and_then(hex_val)) {
                    (Some(h), Some(l)) => {
                        out.push(h << 4 | l);
                        i += 3;
                        continue;
                    }
                    _ => out.push(b'%'),
                }
            }
            b'+' if plus => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// `application/x-www-form-urlencoded` pairs (also query strings). Names and values are
/// decoded; invalid UTF-8 is replaced.
pub fn parse_urlencoded(s: &[u8]) -> Vec<(String, String)> {
    s.split(|&c| c == b'&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = match p.iter().position(|&c| c == b'=') {
                Some(e) => (&p[..e], &p[e + 1..]),
                None => (p, &b""[..]),
            };
            (
                String::from_utf8_lossy(&percent_decode(k, true)).into_owned(),
                String::from_utf8_lossy(&percent_decode(v, true)).into_owned(),
            )
        })
        .collect()
}

/// A media type's main part (lower case) and its parameters (names lower case), from a
/// `Content-Type` or `Content-Disposition` value.
pub fn parse_header_params(v: &str) -> (String, Vec<(String, String)>) {
    let mut it = HeaderParams { s: v.as_bytes(), i: 0 };
    let main = it.token_until_semicolon();
    let mut params = Vec::new();
    while let Some(p) = it.next_param() {
        params.push(p);
        if params.len() >= 32 {
            break;
        }
    }
    (main.trim().to_ascii_lowercase(), params)
}

struct HeaderParams<'a> {
    s: &'a [u8],
    i: usize,
}

impl HeaderParams<'_> {
    fn token_until_semicolon(&mut self) -> String {
        let start = self.i;
        while self.i < self.s.len() && self.s[self.i] != b';' {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }

    fn next_param(&mut self) -> Option<(String, String)> {
        loop {
            while self.i < self.s.len() && matches!(self.s[self.i], b';' | b' ' | b'\t') {
                self.i += 1;
            }
            if self.i >= self.s.len() {
                return None;
            }
            let start = self.i;
            while self.i < self.s.len() && !matches!(self.s[self.i], b'=' | b';') {
                self.i += 1;
            }
            let name = String::from_utf8_lossy(&self.s[start..self.i]).trim().to_ascii_lowercase();
            if self.s.get(self.i) != Some(&b'=') {
                continue; // a parameter without a value
            }
            self.i += 1;
            let value = if self.s.get(self.i) == Some(&b'"') {
                self.i += 1;
                let mut v = Vec::new();
                while self.i < self.s.len() && self.s[self.i] != b'"' {
                    if self.s[self.i] == b'\\' && self.i + 1 < self.s.len() {
                        self.i += 1;
                    }
                    v.push(self.s[self.i]);
                    self.i += 1;
                }
                self.i += 1; // the closing quote
                String::from_utf8_lossy(&v).into_owned()
            } else {
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i] != b';' {
                    self.i += 1;
                }
                String::from_utf8_lossy(&self.s[start..self.i]).trim().to_owned()
            };
            if !name.is_empty() {
                return Some((name, value));
            }
        }
    }
}

/// One `multipart/form-data` part.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

/// Most parts one request may have.
pub const MAX_PARTS: usize = 1000;

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        let at = i + hay[i..hay.len() - needle.len() + 1].iter().position(|&c| c == first)?;
        if &hay[at..at + needle.len()] == needle {
            return Some(at);
        }
        i = at + 1;
    }
    None
}

/// The boundary of a `multipart/form-data` content type (RFC 2046 section 5.1.1).
pub fn multipart_boundary(content_type: &str) -> Option<String> {
    let (main, params) = parse_header_params(content_type);
    if main != "multipart/form-data" {
        return None;
    }
    let b = params.into_iter().find(|(n, _)| n == "boundary")?.1;
    (!b.is_empty() && b.len() <= 70).then_some(b)
}

/// Splits a `multipart/form-data` body (RFC 7578).
pub fn parse_multipart(body: &[u8], boundary: &str) -> Result<Vec<Part>, &'static str> {
    let delim = format!("--{boundary}");
    let delim = delim.as_bytes();
    let mut pos = find(body, delim).ok_or("no multipart boundary")?;
    let mut parts = Vec::new();
    loop {
        pos += delim.len();
        let rest = &body[pos..];
        if rest.starts_with(b"--") {
            return Ok(parts); // close delimiter; the epilogue is ignored
        }
        // transport padding, then the line end
        let line_end = rest.iter().position(|&c| c == b'\n').ok_or("truncated multipart body")?;
        if !rest[..line_end].iter().all(|&c| c == b' ' || c == b'\t' || c == b'\r') {
            return Err("junk after a multipart boundary");
        }
        pos += line_end + 1;
        let rest = &body[pos..];
        let hend = head_end_from_start(rest).ok_or("truncated part header")?;
        let headers = &rest[..hend];
        let content = &rest[hend..];
        // the part ends at CRLF + delimiter (or LF + delimiter)
        let mut close = Vec::with_capacity(delim.len() + 1);
        close.push(b'\n');
        close.extend_from_slice(delim);
        let end = find(content, &close).ok_or("unterminated part")?;
        let data_end = if end > 0 && content[end - 1] == b'\r' { end - 1 } else { end };
        let data = &content[..data_end];

        let mut part = Part { name: String::new(), filename: None, content_type: None, data: data.to_vec() };
        let mut has_disposition = false;
        for line in headers.split(|&c| c == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l)) {
            let Some(colon) = line.iter().position(|&c| c == b':') else { continue };
            let name = String::from_utf8_lossy(&line[..colon]).trim().to_ascii_lowercase();
            let value = String::from_utf8_lossy(&line[colon + 1..]).trim().to_owned();
            match name.as_str() {
                "content-disposition" => {
                    let (kind, params) = parse_header_params(&value);
                    if kind != "form-data" {
                        return Err("part is not form-data");
                    }
                    has_disposition = true;
                    for (n, v) in params {
                        match n.as_str() {
                            "name" => part.name = v,
                            "filename" => part.filename = Some(v),
                            _ => {}
                        }
                    }
                }
                "content-type" => part.content_type = Some(value),
                _ => {}
            }
        }
        if !has_disposition {
            return Err("part without Content-Disposition");
        }
        parts.push(part);
        if parts.len() > MAX_PARTS {
            return Err("too many parts");
        }
        pos += hend + end + 1; // just past the LF before the delimiter
    }
}

/// Like [`head_end`], but a part may have no headers at all (an immediate blank line).
fn head_end_from_start(buf: &[u8]) -> Option<usize> {
    if buf.starts_with(b"\r\n") {
        return Some(2);
    }
    if buf.starts_with(b"\n") {
        return Some(1);
    }
    head_end(buf)
}

/// The value of cookie `name` in a `Cookie` header (RFC 6265 section 5.4).
pub fn cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').filter_map(|c| c.trim().split_once('=')).find(|(n, _)| n.trim() == name).map(
        |(_, v)| {
            let v = v.trim();
            v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v)
        },
    )
}

/// A response to write back.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Response {
        Response { status, content_type, headers: Vec::new(), body: body.into() }
    }

    pub fn text(status: u16, body: impl Into<Vec<u8>>) -> Response {
        Response::new(status, "text/plain; charset=UTF-8", body)
    }

    pub fn ok() -> Response {
        Response::text(200, "")
    }

    pub fn json(v: &super::json::Json) -> Response {
        Response::new(200, "application/json", v.to_json())
    }

    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Response {
        self.headers.push((name, value.into()));
        self
    }

    /// Serializes the status line, headers and (unless `head_only`) the body.
    pub fn write(&self, keep_alive: bool, head_only: bool, out: &mut Vec<u8>) {
        let mut h = String::with_capacity(256);
        let _ = write!(h, "HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        if !self.content_type.is_empty() {
            let _ = write!(h, "Content-Type: {}\r\n", self.content_type);
        }
        let _ = write!(h, "Content-Length: {}\r\n", self.body.len());
        h.push_str(if keep_alive { "Connection: keep-alive\r\n" } else { "Connection: close\r\n" });
        for (n, v) in &self.headers {
            // Values come from our own code; strip line breaks anyway so nothing can inject
            // a header.
            let v: String = v.chars().filter(|&c| c != '\r' && c != '\n').collect();
            let _ = write!(h, "{n}: {v}\r\n");
        }
        h.push_str("\r\n");
        out.extend_from_slice(h.as_bytes());
        if !head_only {
            out.extend_from_slice(&self.body);
        }
    }
}

pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(s: &str) -> Result<Option<(Head, usize)>, HttpError> {
        parse_head(s.as_bytes(), &Limits::default())
    }

    #[test]
    fn parses_a_request_head() {
        let req = "GET /api/v2/app/version?x=1&y=%20 HTTP/1.1\r\nHost: localhost:8080\r\nCookie: SID=ab\r\n\r\nrest";
        let (h, n) = head(req).unwrap().unwrap();
        assert_eq!(n, req.len() - 4);
        assert_eq!(h.method, "GET");
        assert_eq!(h.path, "/api/v2/app/version");
        assert_eq!(h.query, "x=1&y=%20");
        assert_eq!(h.minor, 1);
        assert!(h.keep_alive);
        assert_eq!(h.header("host"), Some("localhost:8080"));
        assert_eq!(h.header("cookie"), Some("SID=ab"));
        assert_eq!(h.content_length, 0);
    }

    #[test]
    fn incomplete_heads_wait_and_leading_blank_lines_are_skipped() {
        assert_eq!(head("GET / HTTP/1.1\r\nHost: a\r\n"), Ok(None));
        assert_eq!(head(""), Ok(None));
        let (h, n) = head("\r\n\r\nGET / HTTP/1.0\n\n").unwrap().unwrap();
        assert_eq!((h.minor, h.keep_alive, n), (0, false, 20));
        let (h, _) = head("GET / HTTP/1.0\r\nConnection: Keep-Alive\r\n\r\n").unwrap().unwrap();
        assert!(h.keep_alive);
        let (h, _) = head("GET / HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n").unwrap().unwrap();
        assert!(!h.keep_alive);
    }

    #[test]
    fn content_length_and_expect() {
        let (h, _) =
            head("POST /x HTTP/1.1\r\nHost: a\r\nContent-Length: 12, 12\r\nExpect: 100-continue\r\n\r\n")
                .unwrap()
                .unwrap();
        assert_eq!(h.content_length, 12);
        assert!(h.expect_continue);
        for bad in [
            "POST /x HTTP/1.1\r\nHost: a\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
            "POST /x HTTP/1.1\r\nHost: a\r\nContent-Length: -1\r\n\r\n",
            "POST /x HTTP/1.1\r\nHost: a\r\nContent-Length: 1 2\r\n\r\n",
            "POST /x HTTP/1.1\r\nHost: a\r\nContent-Length: 99999999999999999999\r\n\r\n",
            "POST /x HTTP/1.1\r\nHost: a\r\nExpect: something\r\n\r\n",
        ] {
            assert!(matches!(head(bad), Err(HttpError::BadRequest(_))), "{bad:?}");
        }
        assert_eq!(
            head("POST /x HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Err(HttpError::NotImplemented)
        );
        let big = format!("POST /x HTTP/1.1\r\nHost: a\r\nContent-Length: {}\r\n\r\n", 101 * 1024 * 1024);
        assert_eq!(head(&big), Err(HttpError::BodyTooLarge));
    }

    #[test]
    fn rejects_malformed_heads() {
        for bad in [
            "GET /\r\n\r\n",
            "GET  / HTTP/1.1\r\nHost: a\r\n\r\n",
            "G(T / HTTP/1.1\r\nHost: a\r\n\r\n",
            "GET x HTTP/1.1\r\nHost: a\r\n\r\n",
            "GET / HTTX/1.1\r\nHost: a\r\n\r\n",
            "GET / HTTP/1.1\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: a\r\n folded\r\n\r\n",
            "GET / HTTP/1.1\r\nHost a\r\n\r\n",
            "GET / HTTP/1.1\r\nBad Name: a\r\nHost: a\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: a\u{1}b\r\n\r\n",
            "GET /%ff HTTP/1.1\r\nHost: a\r\n\r\n",
        ] {
            assert!(matches!(head(bad), Err(HttpError::BadRequest(_))), "{bad:?}");
        }
        assert_eq!(head("GET / HTTP/2.0\r\n\r\n"), Err(HttpError::VersionNotSupported));
        let limits = Limits { max_head: 64, max_headers: 2, max_body: 10, max_message: 10 };
        assert_eq!(parse_head(&[b'a'; 65], &limits), Err(HttpError::HeadTooLarge));
        assert_eq!(
            parse_head(b"GET / HTTP/1.1\r\nHost: a\r\nA: 1\r\nB: 2\r\n\r\n", &limits),
            Err(HttpError::HeadTooLarge)
        );
    }

    #[test]
    fn absolute_form_targets_keep_the_path() {
        let (h, _) =
            head("GET http://example.com:8080/api/v2/x?a HTTP/1.1\r\nHost: a\r\n\r\n").unwrap().unwrap();
        assert_eq!((h.path.as_str(), h.query.as_str()), ("/api/v2/x", "a"));
    }

    #[test]
    fn urlencoded_forms() {
        assert_eq!(
            parse_urlencoded(b"a=1&b=x+y%2Bz&&c&d=%zz%4"),
            vec![
                ("a".into(), "1".into()),
                ("b".into(), "x y+z".into()),
                ("c".into(), "".into()),
                ("d".into(), "%zz%4".into())
            ]
        );
        assert_eq!(percent_decode(b"%", false), b"%");
        assert_eq!(percent_decode(b"a%2", false), b"a%2");
        assert_eq!(percent_decode(b"a+b", false), b"a+b");
    }

    #[test]
    fn multipart_forms() {
        let ct = "multipart/form-data; boundary=\"XyZ\"";
        let b = multipart_boundary(ct).unwrap();
        assert_eq!(b, "XyZ");
        let body =
            b"preamble\r\n--XyZ\r\nContent-Disposition: form-data; name=\"savepath\"\r\n\r\n/tmp/a b\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"torrents\"; filename=\"x.torrent\"\r\n\
Content-Type: application/x-bittorrent\r\n\r\nd4:infod\r\n--XYe\r\n--XyZ--\r\nepilogue";
        let parts = parse_multipart(body, &b).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "savepath");
        assert_eq!(parts[0].data, b"/tmp/a b");
        assert_eq!(parts[1].filename.as_deref(), Some("x.torrent"));
        assert_eq!(parts[1].content_type.as_deref(), Some("application/x-bittorrent"));
        assert_eq!(parts[1].data, b"d4:infod\r\n--XYe");
        assert!(
            parse_multipart(b"--XyZ\r\nContent-Disposition: form-data; name=a\r\n\r\nno end", "XyZ").is_err()
        );
        assert!(parse_multipart(b"--XyZ\r\nX: y\r\n\r\nv\r\n--XyZ--", "XyZ").is_err());
        assert!(parse_multipart(b"nothing", "XyZ").is_err());
        assert_eq!(parse_multipart(b"--XyZ--", "XyZ").unwrap(), vec![]);
        assert!(multipart_boundary("application/x-www-form-urlencoded").is_none());
        assert!(multipart_boundary("multipart/form-data").is_none());
    }

    #[test]
    fn cookies() {
        assert_eq!(cookie("a=1; SID=abc; b=2", "SID"), Some("abc"));
        assert_eq!(cookie("SID=\"q\"", "SID"), Some("q"));
        assert_eq!(cookie("XSID=1", "SID"), None);
    }

    #[test]
    fn writes_responses() {
        let mut out = Vec::new();
        Response::text(404, "nope").with_header("X-A", "b\r\nEvil: 1").write(false, false, &mut out);
        let s = String::from_utf8(out).unwrap();
        assert_eq!(
            s,
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: 4\r\n\
Connection: close\r\nX-A: bEvil: 1\r\n\r\nnope"
        );
    }
}
