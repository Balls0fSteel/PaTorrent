// Ported from libtorrent-rasterbar 2.0.11: src/http_parser.cpp (response parsing, chunked
// transfer encoding), src/http_connection.cpp (request format, redirects)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: parses a complete buffered response instead of incrementally; trackers and web
// seeds only need whole, small bodies here. Responses are capped at `MAX_RESPONSE`.

//! Minimal HTTP/1.1 client protocol for trackers.

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
pub fn parse_response(buf: &[u8], eof: bool) -> Result<Response, ParseError> {
    let Some(head_end) = find(buf, b"\r\n\r\n") else {
        return Err(if eof { ParseError::Invalid("truncated HTTP header") } else { ParseError::Incomplete });
    };
    let head =
        std::str::from_utf8(&buf[..head_end]).map_err(|_| ParseError::Invalid("invalid HTTP header"))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let proto = parts.next().unwrap_or_default();
    if !proto.starts_with("HTTP/") {
        return Err(ParseError::Invalid("not an HTTP response"));
    }
    let status =
        parts.next().and_then(|s| s.parse::<u16>().ok()).ok_or(ParseError::Invalid("invalid HTTP status"))?;
    let message = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let mut resp = Response { status, message, headers, body: Vec::new() };
    let rest = &buf[head_end + 4..];
    let chunked =
        resp.header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    if chunked {
        resp.body = decode_chunked(rest, eof)?;
    } else if let Some(len) = resp.header("content-length") {
        let len: usize = len.parse().map_err(|_| ParseError::Invalid("invalid content-length"))?;
        if len > MAX_RESPONSE {
            return Err(ParseError::Invalid("HTTP response too large"));
        }
        if rest.len() < len {
            return Err(if eof {
                ParseError::Invalid("truncated HTTP body")
            } else {
                ParseError::Incomplete
            });
        }
        resp.body = rest[..len].to_vec();
    } else if eof || status == 204 || status == 304 {
        resp.body = rest.to_vec();
    } else {
        return Err(ParseError::Incomplete);
    }
    Ok(resp)
}

fn decode_chunked(mut buf: &[u8], eof: bool) -> Result<Vec<u8>, ParseError> {
    let incomplete =
        || if eof { ParseError::Invalid("truncated chunked body") } else { ParseError::Incomplete };
    let mut out = Vec::new();
    loop {
        let line_end = find(buf, b"\r\n").ok_or_else(incomplete)?;
        let line =
            std::str::from_utf8(&buf[..line_end]).map_err(|_| ParseError::Invalid("invalid chunk header"))?;
        let size_str = line.split(';').next().unwrap_or_default().trim();
        let size =
            usize::from_str_radix(size_str, 16).map_err(|_| ParseError::Invalid("invalid chunk size"))?;
        buf = &buf[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if out.len() + size > MAX_RESPONSE {
            return Err(ParseError::Invalid("HTTP response too large"));
        }
        if buf.len() < size + 2 {
            return Err(incomplete());
        }
        out.extend_from_slice(&buf[..size]);
        buf = &buf[size + 2..];
    }
}

/// Resolves a `Location` header against the request URL (absolute, or absolute-path).
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
        assert!(matches!(parse_response(b"SSH-2.0\r\n\r\n", true), Err(ParseError::Invalid(_))));
    }

    #[test]
    fn redirects() {
        let base = Url::parse("http://a.b:81/dir/announce?x").unwrap();
        assert_eq!(resolve_redirect(&base, "https://c.d/e").unwrap(), "https://c.d/e");
        assert_eq!(resolve_redirect(&base, "/other").unwrap(), "http://a.b:81/other");
        assert_eq!(resolve_redirect(&base, "rel?y").unwrap(), "http://a.b:81/dir/rel?y");
    }
}
