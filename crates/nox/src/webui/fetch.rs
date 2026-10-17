//! Downloads a `.torrent` named by an `http://` URL for `torrents/add`. HTTPS is not
//! supported yet: the engine's TLS is internal to `patorrent-core` (PLAN.md Known TODOs).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use patorrent_core::USER_AGENT;

use super::http::Limits;

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

pub enum Fetched {
    Torrent(Vec<u8>),
    /// The URL redirected to a magnet link (indexers do this).
    Magnet(String),
}

pub fn fetch(url: &str) -> Result<Fetched, String> {
    let max = Limits::default().max_body;
    let mut url = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        if url.starts_with("magnet:") {
            return Ok(Fetched::Magnet(url));
        }
        let Some(rest) = url.strip_prefix("http://") else {
            return Err(format!("{url}: only http:// and magnet: links are supported"));
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let path = path.split('#').next().unwrap_or("/");
        let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let hostport =
            if authority.rsplit_once(':').is_some_and(|(_, p)| p.bytes().all(|c| c.is_ascii_digit()))
                && !authority.ends_with(']')
            {
                authority.to_owned()
            } else {
                format!("{authority}:80")
            };
        let addr = hostport
            .to_socket_addrs()
            .map_err(|e| format!("{authority}: {e}"))?
            .next()
            .ok_or_else(|| format!("{authority}: no address"))?;
        let mut s = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| e.to_string())?;
        let _ = s.set_read_timeout(Some(TIMEOUT));
        let _ = s.set_write_timeout(Some(TIMEOUT));
        let req = format!(
            "GET {path} HTTP/1.0\r\nHost: {authority}\r\nUser-Agent: {USER_AGENT}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
        s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + TIMEOUT;
        let mut data = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            if Instant::now() > deadline {
                return Err("timed out".into());
            }
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => data.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.to_string()),
            }
            if data.len() > max + 64 * 1024 {
                return Err("too large".into());
            }
        }
        let end = data.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no response header")? + 4;
        let head = String::from_utf8_lossy(&data[..end]).into_owned();
        let mut lines = head.split("\r\n");
        let status: u16 = lines
            .next()
            .and_then(|l| l.split(' ').nth(1))
            .and_then(|c| c.parse().ok())
            .ok_or("bad status line")?;
        let header = |name: &str| {
            head.split("\r\n")
                .skip(1)
                .filter_map(|l| l.split_once(':'))
                .find(|(n, _)| n.trim().eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim().to_owned())
        };
        match status {
            200 => {
                let mut body = data[end..].to_vec();
                if let Some(len) = header("content-length").and_then(|l| l.parse::<usize>().ok()) {
                    body.truncate(len);
                }
                return Ok(Fetched::Torrent(body));
            }
            301 | 302 | 303 | 307 | 308 => {
                let loc = header("location").ok_or("redirect without Location")?;
                url = if loc.contains("://") || loc.starts_with("magnet:") {
                    loc
                } else if loc.starts_with('/') {
                    format!("http://{authority}{loc}")
                } else {
                    return Err("unsupported redirect".into());
                };
            }
            s => return Err(format!("HTTP {s}")),
        }
    }
    Err("too many redirects".into())
}
