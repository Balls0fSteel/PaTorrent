// PaTorrent's own code, not ported: libtorrent's `http_connection` is asynchronous and lives
// inside the session. This is a small blocking client for frontends (Add Torrent from URL, RSS
// feeds) on top of the same request format, response parser, OS TLS and proxy handshakes the
// engine's HTTP trackers use.

//! A blocking HTTP / HTTPS `GET` for frontends: `.torrent` downloads and RSS feeds.
//!
//! [`http_get`] follows redirects (up to [`FetchOptions::max_redirects`]), decodes chunked
//! bodies, caps the body at [`FetchOptions::max_size`] and gives up after
//! [`FetchOptions::timeout`] for the whole exchange, redirects included. HTTPS goes through the
//! OS TLS layer with certificate and host name checks (PLAN.md D9). A proxy from the engine's
//! settings ([`FetchProxy::from_settings`]: whenever one is set) is used the way HTTP trackers
//! use it: an HTTP proxy is asked for `http://` URLs directly, everything else is tunnelled
//! (SOCKS4/5, HTTP CONNECT).
//!
//! Call it from a worker thread: it blocks for up to the timeout.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::engine::proxy::{ProxyConfig, Tunnel};
use crate::net::tls::{self, TlsStream};
use crate::proto::http::{self, ParseError, Url};
use crate::proto::http_parser;
use crate::proto::proxy::Target;
use crate::settings::{ProxyType, Settings};

/// What [`http_get`] does; [`FetchOptions::default`] suits `.torrent` files and feeds.
#[derive(Clone, Debug)]
pub struct FetchOptions {
    /// Sent as `User-Agent` (default `PaTorrent/<version>`, PLAN.md D16).
    pub user_agent: String,
    /// For the whole exchange, every redirect included (default 30 s).
    pub timeout: Duration,
    /// The largest body accepted (default 16 MiB).
    pub max_size: usize,
    /// Redirects followed before giving up (default 5, libtorrent `http_connection`).
    pub max_redirects: u32,
    /// Sent as a `Cookie` header when not empty (`name=value; name2=value2`). Line breaks are
    /// dropped. They go only to the first URL's origin (scheme, host and port): a redirect
    /// elsewhere is followed without them, and one from https to http is refused (SEC-06).
    pub cookies: String,
    pub proxy: Option<FetchProxy>,
    /// Which addresses it may connect to (SEC-26).
    pub reach: Reach,
}

/// Which addresses a fetch may connect to (SEC-26). Loopback, private, link-local and other
/// non-public addresses are the user's own machine and network: a URL from someone else must
/// not make the client send requests there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reach {
    /// A URL the user gave (an add-by-URL, a feed subscription): any address. Once a request
    /// went to a public address, the redirects after it may only lead to public ones.
    #[default]
    User,
    /// A URL from someone else's content (a feed's items): public addresses only.
    Public,
}

impl Reach {
    /// For the links in a page fetched from `page_url`: [`Reach::User`] when the page itself
    /// is on this machine or network (an indexer such as Jackett on `localhost` links to
    /// itself), else [`Reach::Public`]. Resolves the host (blocking); a name that does not
    /// resolve counts as public.
    pub fn for_links_from(page_url: &str) -> Reach {
        let Some(url) = Url::parse(&clean_url(page_url)) else { return Reach::Public };
        let local = match host_ip(&url.host) {
            Some(ip) => !is_public(ip),
            None if is_localhost_name(&url.host) => true,
            None => resolve(&url.host, url.port).is_ok_and(|a| a.iter().any(|a| !is_public(a.ip()))),
        };
        if local { Reach::User } else { Reach::Public }
    }
}

/// A public unicast address: not loopback, private, link-local, CGNAT, unique-local,
/// unspecified, multicast or broadcast. NAT64 (`64:ff9b::/96`) and IPv4-mapped addresses are
/// judged by the IPv4 address inside.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let first = a.octets()[0];
            first != 0 && !a.is_broadcast() && crate::net::enum_net::is_global(&ip)
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let s = a.segments();
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [hi, lo] = [s[6], s[7]];
                return is_public(IpAddr::V4(std::net::Ipv4Addr::from(
                    (u32::from(hi) << 16) | u32::from(lo),
                )));
            }
            crate::net::enum_net::is_global(&ip)
        }
    }
}

/// A host written as an IP address (`[::1]` too).
fn host_ip(host: &str) -> Option<IpAddr> {
    host.trim_start_matches('[').trim_end_matches(']').parse().ok()
}

/// `localhost` and names under it, which resolve to this machine.
fn is_localhost_name(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost")
}

impl Default for FetchOptions {
    fn default() -> FetchOptions {
        FetchOptions {
            user_agent: crate::USER_AGENT.to_owned(),
            timeout: Duration::from_secs(30),
            max_size: 16 * 1024 * 1024,
            max_redirects: http::MAX_REDIRECTS,
            cookies: String::new(),
            proxy: None,
            reach: Reach::User,
        }
    }
}

/// A proxy for [`http_get`], taken from the engine's `proxy_*` settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchProxy(ProxyConfig);

impl FetchProxy {
    /// The session's proxy whenever one is set (`proxy_type` not `None`), whatever
    /// `proxy_peer_connections` and `proxy_tracker_connections` say: those two scope the
    /// engine's peer and tracker connections only. libtorrent's `.torrent` URL download
    /// (`torrent::start_download_url`, before 2.0 removed it) passes the session's proxy to
    /// its `http_connection` unconditionally, and µTorrent's use-proxy boxes name peer and
    /// tracker connections only, so a `.torrent` link and an RSS feed always go through the
    /// proxy.
    pub fn from_settings(s: &Settings) -> Option<FetchProxy> {
        if s.proxy_type == ProxyType::None {
            return None;
        }
        ProxyConfig::from_settings(s).map(FetchProxy)
    }

    /// `socks5://host:port`, for messages.
    pub fn label(&self) -> String {
        self.0.label()
    }
}

/// A finished exchange: a 2xx answer, or a redirect to something other than `http(s)://` (an
/// indexer handing out a `magnet:` link), see [`FetchResponse::redirect`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchResponse {
    /// The URL that answered, after redirects.
    pub url: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl FetchResponse {
    /// The first header named `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// The `Location` of a redirect that was not followed (a `magnet:` link, say).
    pub fn redirect(&self) -> Option<&str> {
        if (300..400).contains(&self.status) { self.header("location") } else { None }
    }
}

#[derive(Debug)]
pub enum FetchError {
    /// Not an `http://` or `https://` URL.
    InvalidUrl(String),
    /// The name did not resolve.
    Resolve(String),
    Io(io::Error),
    /// The proxy refused or failed the tunnel.
    Proxy(String),
    /// A malformed response.
    Http(&'static str),
    /// A non-2xx answer (redirects aside).
    Status(u16, String),
    TooLarge,
    TimedOut,
    TooManyRedirects,
    /// The address is on this machine or network, and the URL may not go there (SEC-26).
    NotPublic(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::InvalidUrl(u) => write!(f, "{u}: only http:// and https:// URLs are supported"),
            FetchError::Resolve(h) => write!(f, "{h}: host not found"),
            FetchError::Io(e) => write!(f, "{e}"),
            FetchError::Proxy(e) => write!(f, "{e}"),
            FetchError::Http(e) => f.write_str(e),
            FetchError::Status(code, msg) if msg.is_empty() => write!(f, "HTTP {code}"),
            FetchError::Status(code, msg) => write!(f, "HTTP {code} {msg}"),
            FetchError::TooLarge => f.write_str("response too large"),
            FetchError::TimedOut => f.write_str("timed out"),
            FetchError::TooManyRedirects => f.write_str("too many redirects"),
            FetchError::NotPublic(h) => write!(f, "{h}: refused, a local network address"),
        }
    }
}

impl std::error::Error for FetchError {}

impl From<io::Error> for FetchError {
    fn from(e: io::Error) -> FetchError {
        // a read or write timeout surfaces as WouldBlock on Unix, TimedOut on Windows
        match e.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => FetchError::TimedOut,
            _ => FetchError::Io(e),
        }
    }
}

/// `GET url`. Returns 2xx answers and unfollowed redirects (see [`FetchResponse`]); any other
/// status is [`FetchError::Status`].
pub fn http_get(url: &str, opts: &FetchOptions) -> Result<FetchResponse, FetchError> {
    let deadline = Instant::now() + opts.timeout;
    let mut url = clean_url(url);
    let mut redirects = 0;
    let has_cookies = !opts.cookies.trim().is_empty();
    let mut origin = None;
    let mut public_only = opts.reach == Reach::Public;
    loop {
        let parsed = Url::parse(&url).ok_or_else(|| FetchError::InvalidUrl(url.clone()))?;
        let origin = origin.get_or_insert_with(|| Origin::of(&parsed));
        let cookies = has_cookies && *origin == Origin::of(&parsed);
        let (resp, reached_public) = get_once(&parsed, opts, cookies, deadline, public_only)?;
        // a public server may not send the client into its own network (SEC-26)
        public_only |= reached_public;
        if (200..300).contains(&resp.status) {
            return Ok(FetchResponse { url, status: resp.status, headers: resp.headers, body: resp.body });
        }
        if !resp.is_redirect() {
            return Err(FetchError::Status(resp.status, resp.message));
        }
        let location = resp.header("location").ok_or(FetchError::Http("redirect without a Location"))?;
        if other_scheme(location) {
            return Ok(FetchResponse { url, status: resp.status, headers: resp.headers, body: resp.body });
        }
        redirects += 1;
        if redirects > opts.max_redirects {
            return Err(FetchError::TooManyRedirects);
        }
        let next =
            http::resolve_redirect(&parsed, location).ok_or(FetchError::Http("invalid redirect location"))?;
        url = clean_url(&next);
        if has_cookies && downgrades(&parsed, &url) {
            return Err(FetchError::Http("refused a redirect from https to http"));
        }
    }
}

/// A redirect from https to plain http: refused when cookies are in play, since a site that
/// hands them out over https does not send its users to http (and an on-path attacker could).
fn downgrades(from: &Url, to: &str) -> bool {
    from.scheme == "https" && Url::parse(to).is_some_and(|u| u.scheme == "http")
}

/// Where cookies may go: the scheme, host and port of the URL they were given for.
#[derive(PartialEq, Eq)]
struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    fn of(url: &Url) -> Origin {
        let host = url.host.trim_end_matches('.').to_ascii_lowercase();
        Origin { scheme: url.scheme.clone(), host, port: url.port }
    }
}

/// Drops the fragment (the client's, never sent) and percent-encodes the bytes that may not
/// appear in a URL at all (spaces, controls, non-ASCII, `"<>\^`{|}`), leaving its structure and
/// any existing escapes alone: feeds are full of enclosure URLs with raw spaces in them.
/// (libtorrent's `maybe_url_encode` re-escapes the whole path and query, `?&=` included.)
fn clean_url(url: &str) -> String {
    let url = url.trim();
    let url = url.split('#').next().unwrap_or_default();
    let mut out = String::with_capacity(url.len());
    for &c in url.as_bytes() {
        if c <= b' ' || c >= 0x7f || b"\"<>\\^`{|}".contains(&c) {
            out.push_str(&format!("%{c:02X}"));
        } else {
            out.push(c as char);
        }
    }
    out
}

/// A `Location` with a scheme other than http(s), which `resolve_redirect` would otherwise
/// take for a relative path (`magnet:?xt=...`).
fn other_scheme(location: &str) -> bool {
    let Some((scheme, _)) = location.split_once(':') else { return false };
    let is_scheme = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.bytes().all(|c| c.is_ascii_alphanumeric() || b"+-.".contains(&c));
    is_scheme && !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https")
}

fn remaining(deadline: Instant) -> Result<Duration, FetchError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() { Err(FetchError::TimedOut) } else { Ok(left) }
}

/// The request bytes: libtorrent's `GET` (or the absolute-URL form for an HTTP proxy), with the
/// cookies added when `cookies` says so.
fn request(url: &Url, opts: &FetchOptions, cookies: bool, via_http_proxy: Option<&ProxyConfig>) -> Vec<u8> {
    let user_agent = opts.user_agent.replace(['\r', '\n'], "");
    let mut r = match via_http_proxy {
        Some(p) => http::proxy_get_request(url, &user_agent, p.creds()),
        None => http::get_request(url, &user_agent),
    };
    let cookies = if cookies { opts.cookies.replace(['\r', '\n'], "") } else { String::new() };
    if !cookies.trim().is_empty() {
        // before the blank line that ends the head
        r.truncate(r.len() - 2);
        r.extend_from_slice(format!("Cookie: {}\r\n\r\n", cookies.trim()).as_bytes());
    }
    r
}

fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, FetchError> {
    let addrs: Vec<SocketAddr> =
        (host, port).to_socket_addrs().map_err(|_| FetchError::Resolve(host.to_owned()))?.collect();
    if addrs.is_empty() {
        return Err(FetchError::Resolve(host.to_owned()));
    }
    Ok(addrs)
}

/// Connects to the first address that answers.
fn connect(addrs: &[SocketAddr], deadline: Instant) -> Result<TcpStream, FetchError> {
    let mut last = FetchError::TimedOut;
    for a in addrs {
        match TcpStream::connect_timeout(a, remaining(deadline)?) {
            Ok(s) => return Ok(s),
            Err(e) => last = e.into(),
        }
    }
    Err(last)
}

/// `addrs` may be reached: all public when `public_only`. Whether they are all public.
fn check_reach(host: &str, addrs: &[IpAddr], public_only: bool) -> Result<bool, FetchError> {
    let public = addrs.iter().all(|&a| is_public(a));
    if public_only && !public {
        return Err(FetchError::NotPublic(host.to_owned()));
    }
    Ok(public)
}

/// The addresses a proxy will be sent to for `host` when it resolves the name itself: the
/// address of an IP literal, a loopback address for `localhost`; none for other names, which
/// only the proxy resolves.
fn named_target(host: &str) -> Vec<IpAddr> {
    match host_ip(host) {
        Some(ip) => vec![ip],
        None if is_localhost_name(host) => vec![IpAddr::from([127, 0, 0, 1])],
        None => Vec::new(),
    }
}

/// One request and its response, without following redirects; with whether the server's
/// address is public. `public_only` refuses any other (SEC-26).
fn get_once(
    url: &Url,
    opts: &FetchOptions,
    cookies: bool,
    deadline: Instant,
    public_only: bool,
) -> Result<(http::Response, bool), FetchError> {
    let https = url.scheme == "https";
    let proxy = opts.proxy.as_ref().map(|p| &p.0);
    let plain_http_proxy = proxy.filter(|p| p.plain_http(https));
    let req = request(url, opts, cookies, plain_http_proxy);
    let public;
    let sock = match proxy {
        None => {
            let addrs = resolve(&url.host, url.port)?;
            let ips: Vec<IpAddr> = addrs.iter().map(SocketAddr::ip).collect();
            public = check_reach(&url.host, &ips, public_only)?;
            connect(&addrs, deadline)?
        }
        Some(p) => {
            // the proxy is told a name when it resolves names, an address otherwise
            let target = match Target::new(&url.host, url.port) {
                Target::Host(..) if plain_http_proxy.is_none() && !p.resolves_names() => {
                    Target::Addr(resolve(&url.host, url.port)?[0])
                }
                t => t,
            };
            let ips = match &target {
                Target::Addr(a) => vec![a.ip()],
                Target::Host(..) => named_target(&url.host),
            };
            public = check_reach(&url.host, &ips, public_only)?;
            let mut sock = connect(&resolve(&p.host, p.port)?, deadline)?;
            if plain_http_proxy.is_none() {
                let hs = p.tunnel(&target).map_err(|e| FetchError::Proxy(format!("proxy: {e}")))?;
                set_timeouts(&sock, deadline)?;
                match Tunnel::new(hs).drive(&mut sock) {
                    Ok(Some(_)) => {}
                    // the blocking socket's timeout ran out
                    Ok(None) => return Err(FetchError::TimedOut),
                    Err(e) => return Err(FetchError::Proxy(e.to_string())),
                }
            }
            sock
        }
    };
    let resp = if https {
        let session = tls::client(&url.host)?;
        exchange(TlsStream::new(sock, session), tls_socket, &req, opts.max_size, deadline)
    } else {
        exchange(sock, plain_socket, &req, opts.max_size, deadline)
    }?;
    Ok((resp, public))
}

// The socket under a stream, for its timeouts (a `try_clone` would not do: on Windows the clone
// is a separate socket with its own options).
fn plain_socket(s: &mut TcpStream) -> &TcpStream {
    s
}

fn tls_socket(s: &mut TlsStream<TcpStream>) -> &TcpStream {
    s.get_mut()
}

fn set_timeouts(sock: &TcpStream, deadline: Instant) -> Result<(), FetchError> {
    let left = remaining(deadline)?;
    sock.set_read_timeout(Some(left))?;
    sock.set_write_timeout(Some(left))?;
    Ok(())
}

/// Sends the request and reads until the response is complete: its `Content-Length` is in,
/// the last chunk arrived, or the server closed the connection.
fn exchange<S: Read + Write>(
    mut s: S,
    sock: fn(&mut S) -> &TcpStream,
    req: &[u8],
    max: usize,
    deadline: Instant,
) -> Result<http::Response, FetchError> {
    set_timeouts(sock(&mut s), deadline)?;
    s.write_all(req)?;
    s.flush()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    // the head, once it is in: (content length or -1, chunked, body offset)
    let mut framing: Option<(i64, bool, usize)> = None;
    // keeps its place across reads, so a chunked body is decoded once, not after every read
    let mut parser = http::ResponseParser::new(max);
    let result = loop {
        set_timeouts(sock(&mut s), deadline)?;
        let n = match s.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        if n == 0 {
            break parser.parse(&buf, true);
        }
        buf.extend_from_slice(&chunk[..n]);
        if framing.is_none() {
            match http_parser::parse_head(&buf) {
                Ok(Some((head, body))) => framing = Some((head.content_length, head.chunked, body)),
                Ok(None) => {}
                Err(http_parser::ParseError::Invalid(e)) => return Err(FetchError::Http(e)),
            }
        }
        let Some((len, chunked, body)) = framing else { continue };
        if buf.len() - body > max.saturating_add(64 * 1024) || (!chunked && len > max as i64) {
            return Err(FetchError::TooLarge);
        }
        if chunked {
            match parser.parse(&buf, false) {
                Err(ParseError::Incomplete) => {}
                r => break r,
            }
        } else if len >= 0 && (buf.len() - body) as i64 >= len {
            break parser.parse(&buf, false);
        }
    };
    match result {
        Ok(r) => Ok(r),
        Err(ParseError::Invalid("HTTP response too large")) => Err(FetchError::TooLarge),
        Err(ParseError::Invalid(e)) => Err(FetchError::Http(e)),
        Err(ParseError::Incomplete) => Err(FetchError::Http("truncated HTTP response")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    /// A loopback server answering each connection with the next canned response; the request
    /// heads it saw come back through the channel.
    fn serve(responses: Vec<Vec<u8>>) -> (String, mpsc::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for resp in responses {
                let Ok((mut s, _)) = l.accept() else { return };
                let head = read_head(&mut s);
                let _ = tx.send(head);
                let _ = s.write_all(&resp);
            }
        });
        (base, rx)
    }

    fn read_head(s: &mut TcpStream) -> String {
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") && s.read(&mut b).unwrap_or(0) == 1 {
            head.push(b[0]);
        }
        String::from_utf8_lossy(&head).into_owned()
    }

    fn opts() -> FetchOptions {
        FetchOptions { timeout: Duration::from_secs(10), ..FetchOptions::default() }
    }

    #[test]
    fn content_length_body_and_request_head() {
        let (base, rx) = serve(vec![b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".to_vec()]);
        let o = FetchOptions { cookies: "uid=1; pass=x\r\nEvil: 1".into(), ..opts() };
        let r = http_get(&format!("{base}/a b/feed.xml?x=1#frag"), &o).unwrap();
        assert_eq!((r.status, &r.body[..]), (200, &b"hello"[..]));
        let head = rx.recv().unwrap();
        assert!(head.starts_with("GET /a%20b/feed.xml?x=1 HTTP/1.1\r\n"), "{head}");
        assert!(head.contains(&format!("\r\nUser-Agent: {}\r\n", crate::USER_AGENT)), "{head}");
        assert!(head.contains("\r\nCookie: uid=1; pass=xEvil: 1\r\n\r\n"), "{head}");
        assert!(!head.contains("\r\nEvil:"));
    }

    /// The server keeps the connection open: the body's length (or its last chunk) ends it.
    #[test]
    fn framing_without_close() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        thread::spawn(move || {
            let mut keep = Vec::new();
            for resp in [
                &b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc"[..],
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n",
            ] {
                let (mut s, _) = l.accept().unwrap();
                read_head(&mut s);
                s.write_all(resp).unwrap();
                keep.push(s);
            }
            thread::sleep(Duration::from_secs(5));
        });
        let o = FetchOptions { timeout: Duration::from_secs(3), ..opts() };
        let t = Instant::now();
        assert_eq!(http_get(&base, &o).unwrap().body, b"abc");
        assert_eq!(http_get(&base, &o).unwrap().body, b"Wikipedia");
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn eof_framed_body() {
        let (base, _rx) = serve(vec![b"HTTP/1.0 200 OK\r\n\r\n<rss/>".to_vec()]);
        assert_eq!(http_get(&base, &opts()).unwrap().body, b"<rss/>");
    }

    #[test]
    fn redirects_and_magnets() {
        let (base, rx) = serve(vec![
            b"HTTP/1.1 302 Found\r\nLocation: /dir/x.torrent\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 301 Moved\r\nLocation: y.torrent\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nd1".to_vec(),
            b"HTTP/1.1 302 Found\r\nLocation: magnet:?xt=urn:btih:abc\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ]);
        let r = http_get(&format!("{base}/start"), &opts()).unwrap();
        assert_eq!(
            (r.status, &r.body[..], r.url.as_str()),
            (200, &b"d1"[..], &*format!("{base}/dir/y.torrent"))
        );
        let heads: Vec<String> = rx.try_iter().collect();
        assert!(heads[1].starts_with("GET /dir/x.torrent "), "{heads:?}");
        assert!(heads[2].starts_with("GET /dir/y.torrent "), "{heads:?}");
        let r = http_get(&base, &opts()).unwrap();
        assert_eq!((r.status, r.redirect()), (302, Some("magnet:?xt=urn:btih:abc")));
    }

    /// SEC-06: cookies go to the origin they were given for. A redirect to another origin (here
    /// another port) is followed without them; one on the same origin keeps them.
    #[test]
    fn cookies_stay_on_their_origin() {
        let (other, other_rx) = serve(vec![b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nd1".to_vec()]);
        let (base, rx) = serve(vec![
            b"HTTP/1.1 302 Found\r\nLocation: /dl/x.torrent\r\nContent-Length: 0\r\n\r\n".to_vec(),
            format!("HTTP/1.1 302 Found\r\nLocation: {other}/steal\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        ]);
        let o = FetchOptions { cookies: "uid=1; pass=secret".into(), ..opts() };
        let r = http_get(&format!("{base}/start"), &o).unwrap();
        assert_eq!(&r.body[..], b"d1");
        let heads: Vec<String> = rx.try_iter().collect();
        assert_eq!(heads.len(), 2);
        assert!(heads.iter().all(|h| h.contains("\r\nCookie: uid=1; pass=secret\r\n")), "{heads:?}");
        let stolen = other_rx.recv().unwrap();
        assert!(stolen.starts_with("GET /steal ") && !stolen.contains("Cookie"), "{stolen}");
    }

    /// SEC-06: with cookies, an https page redirecting to http is refused rather than followed.
    #[test]
    fn https_to_http_redirects() {
        let from = Url::parse("https://tracker.example/dl.php?id=1").unwrap();
        assert!(downgrades(&from, "http://tracker.example/x.torrent"));
        assert!(!downgrades(&from, "https://cdn.example/x.torrent"));
        let plain = Url::parse("http://tracker.example/dl.php").unwrap();
        assert!(!downgrades(&plain, "http://tracker.example/x.torrent"));
    }

    #[test]
    fn redirect_limit() {
        let loop_resp = b"HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\n\r\n".to_vec();
        let (base, _rx) = serve(vec![loop_resp; 4]);
        let o = FetchOptions { max_redirects: 2, ..opts() };
        assert!(matches!(http_get(&base, &o), Err(FetchError::TooManyRedirects)));
    }

    #[test]
    fn errors() {
        let (base, _rx) = serve(vec![
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\n\r\n0123456789abcdef".to_vec(),
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10\r\n0123456789abcdef\r\n0\r\n\r\n"
                .to_vec(),
            b"SSH-2.0-OpenSSH\r\n\r\n".to_vec(),
        ]);
        let e = http_get(&base, &opts()).unwrap_err();
        assert_eq!(e.to_string(), "HTTP 404 Not Found");
        let small = FetchOptions { max_size: 10, ..opts() };
        assert!(matches!(http_get(&base, &small), Err(FetchError::TooLarge)));
        assert!(matches!(http_get(&base, &small), Err(FetchError::TooLarge)));
        assert!(matches!(http_get(&base, &small), Err(FetchError::TooLarge)));
        assert!(matches!(http_get(&base, &opts()), Err(FetchError::Http(_))));
        for bad in ["ftp://example.org/x", "magnet:?xt=urn:btih:abc", "nonsense", "http://"] {
            assert!(matches!(http_get(bad, &opts()), Err(FetchError::InvalidUrl(_))), "{bad}");
        }
    }

    #[test]
    fn timeout() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            read_head(&mut s);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc");
            thread::sleep(Duration::from_secs(5));
        });
        let o = FetchOptions { timeout: Duration::from_millis(500), ..opts() };
        let t = Instant::now();
        let r = http_get(&base, &o);
        assert!(matches!(r, Err(FetchError::TimedOut)), "{r:?}");
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    fn proxy_settings(kind: ProxyType, port: u16) -> Settings {
        Settings {
            proxy_type: kind,
            proxy_hostname: "127.0.0.1".into(),
            proxy_port: port,
            proxy_username: "u".into(),
            proxy_password: "p".into(),
            proxy_tracker_connections: true,
            ..Settings::default()
        }
    }

    #[test]
    fn proxy_selection() {
        let mut s = proxy_settings(ProxyType::Socks5, 1080);
        assert_eq!(FetchProxy::from_settings(&s).unwrap().label(), "socks5://127.0.0.1:1080");
        // the peer and tracker scopes do not apply to web fetches
        s.proxy_tracker_connections = false;
        s.proxy_peer_connections = false;
        assert_eq!(FetchProxy::from_settings(&s).unwrap().label(), "socks5://127.0.0.1:1080");
        assert!(FetchProxy::from_settings(&Settings::default()).is_none());
    }

    #[test]
    fn public_addresses() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        for local in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "0.1.2.3",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
        ] {
            assert!(!is_public(ip(local)), "{local}");
        }
        for public in ["8.8.8.8", "203.0.113.5", "2001:db8::1", "::ffff:8.8.8.8", "64:ff9b::808:808"] {
            assert!(is_public(ip(public)), "{public}");
        }
        for (page, reach) in [
            ("http://localhost:9117/api", Reach::User),
            ("http://Indexer.LOCALHOST./api", Reach::User),
            ("http://127.0.0.1/rss", Reach::User),
            ("http://[::1]:8080/rss", Reach::User),
            ("http://192.168.1.10/rss:COOKIE:x=1", Reach::User),
            ("https://8.8.8.8/rss", Reach::Public),
            ("http://feeds.example.invalid/rss", Reach::Public),
            ("not a url", Reach::Public),
        ] {
            assert_eq!(Reach::for_links_from(page), reach, "{page}");
        }
    }

    /// A URL from content reaches public addresses only, and nothing is sent to the refused
    /// address (SEC-26).
    #[test]
    fn content_urls_reach_public_addresses_only() {
        let (base, rx) = serve(vec![b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()]);
        let public = FetchOptions { reach: Reach::Public, ..opts() };
        let e = http_get(&format!("{base}/x.torrent"), &public).unwrap_err();
        assert!(matches!(e, FetchError::NotPublic(_)), "{e}");
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "nothing was sent");
        // the user's own URL may go there
        assert_eq!(http_get(&format!("{base}/x.torrent"), &opts()).unwrap().body, b"ok");
    }

    /// Once a request went to a public address, a redirect into this machine or network is
    /// refused; one from a local server to another local one is followed (SEC-26).
    #[test]
    fn a_public_server_cannot_redirect_into_the_network() {
        let (local, _local_rx) = serve(vec![b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()]);
        // the proxy stands in for a public server: it answers for a public name
        let (proxy, rx) =
            serve(vec![format!("HTTP/1.1 302 Found\r\nLocation: {local}/admin\r\n\r\n").into_bytes()]);
        let port: u16 = proxy.rsplit(':').next().unwrap().parse().unwrap();
        let o = FetchOptions {
            proxy: FetchProxy::from_settings(&proxy_settings(ProxyType::Http, port)),
            ..opts()
        };
        let e = http_get("http://feeds.example.invalid/rss", &o).unwrap_err();
        assert!(matches!(&e, FetchError::NotPublic(h) if h == "127.0.0.1"), "{e}");
        assert!(rx.recv().unwrap().starts_with("GET http://feeds.example.invalid/rss "));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "the redirect was not followed");
        let (start, _) =
            serve(vec![format!("HTTP/1.1 302 Found\r\nLocation: {local}/feed\r\n\r\n").into_bytes()]);
        assert_eq!(http_get(&format!("{start}/rss"), &opts()).unwrap().body, b"ok");
    }

    /// An HTTP proxy gets `http://` URLs in absolute form, with its credentials.
    #[test]
    fn plain_http_through_an_http_proxy() {
        let (proxy, rx) = serve(vec![b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()]);
        let port: u16 = proxy.rsplit(':').next().unwrap().parse().unwrap();
        let o = FetchOptions {
            proxy: FetchProxy::from_settings(&proxy_settings(ProxyType::HttpPw, port)),
            ..opts()
        };
        let r = http_get("http://feeds.example.invalid:8080/rss?x", &o).unwrap();
        assert_eq!(r.body, b"ok");
        let head = rx.recv().unwrap();
        assert!(head.starts_with("GET http://feeds.example.invalid:8080/rss?x HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("\r\nProxy-Authorization: Basic dTpw\r\n"), "{head}");
    }

    /// A SOCKS5 proxy (no authentication) is asked to CONNECT, then carries the request.
    #[test]
    fn tunnel_through_socks5() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut greeting = [0u8; 3];
            s.read_exact(&mut greeting).unwrap();
            assert_eq!(greeting, [5, 1, 0]);
            s.write_all(&[5, 0]).unwrap();
            // CONNECT to a host name: 5 1 0 3 len name port
            let mut hdr = [0u8; 5];
            s.read_exact(&mut hdr).unwrap();
            let mut name = vec![0u8; hdr[4] as usize + 2];
            s.read_exact(&mut name).unwrap();
            tx.send(String::from_utf8_lossy(&name[..name.len() - 2]).into_owned()).unwrap();
            s.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80]).unwrap();
            let head = read_head(&mut s);
            tx.send(head).unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nfeed").unwrap();
        });
        let mut settings = proxy_settings(ProxyType::Socks5, port);
        settings.proxy_hostnames = true;
        let o = FetchOptions { proxy: FetchProxy::from_settings(&settings), ..opts() };
        let r = http_get("http://feeds.example.invalid/rss", &o).unwrap();
        assert_eq!(r.body, b"feed");
        assert_eq!(rx.recv().unwrap(), "feeds.example.invalid");
        assert!(rx.recv().unwrap().starts_with("GET /rss HTTP/1.1\r\nHost: feeds.example.invalid\r\n"));
    }

    #[test]
    fn url_cleanup() {
        assert_eq!(clean_url(" http://a.b/x y/é?q=a b&c=%20#frag "), "http://a.b/x%20y/%C3%A9?q=a%20b&c=%20");
        assert_eq!(clean_url("http://[::1]:80/a|b"), "http://[::1]:80/a%7Cb");
    }

    #[test]
    fn schemes() {
        assert!(other_scheme("magnet:?xt=urn:btih:abc"));
        assert!(other_scheme("ftp://x/y"));
        assert!(!other_scheme("HTTPS://x/y"));
        assert!(!other_scheme("/path:with:colons"));
        assert!(!other_scheme("rel?a=b:c"));
    }

    // --- Live check (network): set LT_NETWORK_TESTS=1 and run with --ignored ---------------

    #[test]
    #[ignore = "network"]
    fn live_https_get() {
        if std::env::var_os("LT_NETWORK_TESTS").is_none() {
            eprintln!("skipped: set LT_NETWORK_TESTS=1 to run network tests");
            return;
        }
        let url = std::env::var("LT_FETCH_URL").unwrap_or_else(|_| "https://www.cloudflare.com/".into());
        let r = http_get(&url, &FetchOptions::default()).unwrap();
        eprintln!("{} {} bytes from {}", r.status, r.body.len(), r.url);
        assert!(!r.body.is_empty());
    }
}
