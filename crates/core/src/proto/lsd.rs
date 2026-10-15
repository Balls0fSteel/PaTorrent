// Ported from libtorrent-rasterbar 2.0.11: src/lsd.cpp (render_lsd_packet, on_announce), the
// request-header rules of src/http_parser.cpp that LSD relies on
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the packet check is a function of its own (libtorrent parses inside the socket
// callback), returning every reason a packet is ignored so the engine can log it.

//! BEP 14 local service discovery messages, sans-IO.

use std::net::{Ipv4Addr, Ipv6Addr};

use crate::types::Sha1Hash;

/// The LSD port (BEP 14).
pub const PORT: u16 = 6771;
pub const MULTICAST_V4: Ipv4Addr = Ipv4Addr::new(239, 192, 152, 143);
pub const MULTICAST_V6: Ipv6Addr = Ipv6Addr::new(0xff15, 0, 0, 0, 0, 0, 0xefc0, 0x988f);

/// libtorrent `render_lsd_packet`: one info-hash per packet, our cookie in lower-case hex.
pub fn render(listen_port: u16, info_hash: &Sha1Hash, cookie: u32, v4: bool) -> Vec<u8> {
    let host = if v4 { "239.192.152.143" } else { "[ff15::efc0:988f]" };
    format!(
        "BT-SEARCH * HTTP/1.1\r\nHost: {host}:6771\r\nPort: {listen_port}\r\nInfohash: {}\r\ncookie: {cookie:x}\r\n\r\n\r\n",
        info_hash.to_hex()
    )
    .into_bytes()
}

/// A valid BT-SEARCH request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announce {
    pub port: u16,
    /// The sender's cookie as `strtol(.., 16)` reads it; ours means the packet is our own.
    pub cookie: Option<i64>,
    /// The well-formed, non-zero info-hashes (BEP 14 allows several headers).
    pub info_hashes: Vec<Sha1Hash>,
}

/// Why a packet is not a valid BT-SEARCH request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// No request line, or the header never ended.
    Incomplete,
    Method,
    MissingPort,
    BadPort,
}

/// C `strtol` on the start of `s`: leading whitespace, a sign and as many digits as there are;
/// 0 when there are none. Overflow saturates.
fn strtol(s: &str, radix: u32) -> i64 {
    let s = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (neg, mut s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if radix == 16
        && (s.starts_with("0x") || s.starts_with("0X"))
        && s[2..].starts_with(|c: char| c.is_ascii_hexdigit())
    {
        s = &s[2..];
    }
    let mut v: i64 = 0;
    for c in s.chars() {
        let Some(d) = c.to_digit(radix) else { break };
        v = v.saturating_mul(radix as i64).saturating_add(d as i64);
    }
    if neg { -v } else { v }
}

/// Parses a datagram the way libtorrent's `http_parser` reads a request header, then applies
/// `lsd::on_announce`'s checks. Info-hash headers that are not 40 hex digits, or all zeros,
/// are skipped; the request is still valid.
pub fn parse(buf: &[u8]) -> Result<Announce, Invalid> {
    let mut lines = buf.split(|&b| b == b'\n');
    // every line must end in '\n'; the part after the last one is not a line
    let complete = buf.iter().filter(|&&b| b == b'\n').count();
    let mut line_no = 0;
    let mut next_line = || -> Option<&[u8]> {
        if line_no >= complete {
            return None;
        }
        line_no += 1;
        let l = lines.next()?;
        Some(l.strip_suffix(b"\r").unwrap_or(l))
    };
    let request = next_line().ok_or(Invalid::Incomplete)?;
    if request.is_empty() {
        return Err(Invalid::Incomplete);
    }
    let method = request.split(|&b| b == b' ').next().unwrap_or_default();
    let mut port = None;
    let mut cookie = None;
    let mut hashes = Vec::new();
    let mut finished = false;
    while let Some(line) = next_line() {
        let line = String::from_utf8_lossy(line);
        let Some((name, value)) = line.split_once(':') else {
            // a blank (or colon-less) line ends the header
            finished = true;
            break;
        };
        let value = value.trim_start_matches([' ', '\t']);
        match name.to_ascii_lowercase().as_str() {
            "port" if port.is_none() => port = Some(value.to_owned()),
            "cookie" if cookie.is_none() => cookie = Some(strtol(value, 16)),
            "infohash" => {
                let h = Sha1Hash::from_hex(value).filter(|h| value.len() == 40 && !h.is_zero());
                hashes.extend(h);
            }
            "content-length" => {
                let n = strtol(value, 10);
                if n < 0 || n == i64::MAX {
                    return Err(Invalid::Incomplete);
                }
            }
            _ => {}
        }
    }
    if !finished {
        return Err(Invalid::Incomplete);
    }
    if !method.eq_ignore_ascii_case(b"bt-search") {
        return Err(Invalid::Method);
    }
    let port = port.filter(|p| !p.is_empty()).ok_or(Invalid::MissingPort)?;
    let port = strtol(&port, 10);
    if port <= 0 || port >= u16::MAX as i64 {
        return Err(Invalid::BadPort);
    }
    Ok(Announce { port: port as u16, cookie, info_hashes: hashes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ih(c: u8) -> Sha1Hash {
        Sha1Hash::from_slice(&[c; 20]).unwrap()
    }

    #[test]
    fn rendered_packet() {
        let p = render(6881, &ih(0xab), 0x1234abcd, true);
        assert_eq!(
            std::str::from_utf8(&p).unwrap(),
            "BT-SEARCH * HTTP/1.1\r\nHost: 239.192.152.143:6771\r\nPort: 6881\r\n\
             Infohash: abababababababababababababababababababab\r\ncookie: 1234abcd\r\n\r\n\r\n"
        );
        let v6 = render(1, &ih(1), 7, false);
        assert!(std::str::from_utf8(&v6).unwrap().contains("Host: [ff15::efc0:988f]:6771\r\n"));
        assert_eq!(
            parse(&p),
            Ok(Announce { port: 6881, cookie: Some(0x1234abcd), info_hashes: vec![ih(0xab)] })
        );
    }

    #[test]
    fn several_hashes_and_loose_formatting() {
        let p = b"bt-search * HTTP/1.1\nHOST: 239.192.152.143:6771\nPORT:\t51413\n\
                  Infohash: 0101010101010101010101010101010101010101\n\
                  infohash: 0000000000000000000000000000000000000000\n\
                  Infohash: 02020202\n\
                  Infohash: 0202020202020202020202020202020202020202\n\n";
        assert_eq!(parse(p), Ok(Announce { port: 51413, cookie: None, info_hashes: vec![ih(1), ih(2)] }));
    }

    #[test]
    fn rejected_packets() {
        let ok =
            "BT-SEARCH * HTTP/1.1\r\nPort: 6881\r\nInfohash: 0101010101010101010101010101010101010101\r\n";
        // the header must end with a blank line
        assert_eq!(parse(ok.as_bytes()), Err(Invalid::Incomplete));
        assert!(parse(format!("{ok}\r\n").as_bytes()).is_ok());
        assert_eq!(parse(b""), Err(Invalid::Incomplete));
        assert_eq!(parse(b"\r\n\r\n"), Err(Invalid::Incomplete));
        assert_eq!(parse(b"NOTIFY * HTTP/1.1\r\nPort: 1\r\n\r\n"), Err(Invalid::Method));
        assert_eq!(parse(b"HTTP/1.1 200 OK\r\nPort: 1\r\n\r\n"), Err(Invalid::Method));
        assert_eq!(parse(b"BT-SEARCH * HTTP/1.1\r\nHost: x\r\n\r\n"), Err(Invalid::MissingPort));
        assert_eq!(parse(b"BT-SEARCH * HTTP/1.1\r\nPort: \r\n\r\n"), Err(Invalid::MissingPort));
        for bad in ["0", "-5", "65535", "70000", "x"] {
            let p = format!("BT-SEARCH * HTTP/1.1\r\nPort: {bad}\r\n\r\n");
            assert_eq!(parse(p.as_bytes()), Err(Invalid::BadPort), "{bad}");
        }
        // strtol reads the leading digits
        let p = b"BT-SEARCH * HTTP/1.1\r\nPort: 6881abc\r\ncookie: zz\r\n\r\n";
        assert_eq!(parse(p), Ok(Announce { port: 6881, cookie: Some(0), info_hashes: vec![] }));
    }

    #[test]
    fn strtol_matches_c() {
        assert_eq!(strtol("  42xyz", 10), 42);
        assert_eq!(strtol("-7", 10), -7);
        assert_eq!(strtol("0x1F", 16), 31);
        assert_eq!(strtol("0xg", 16), 0);
        assert_eq!(strtol("ffffffffffffffffffff", 16), i64::MAX);
        assert_eq!(strtol("", 10), 0);
    }
}
