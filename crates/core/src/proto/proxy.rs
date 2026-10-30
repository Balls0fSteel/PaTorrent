// Ported from libtorrent-rasterbar 2.0.11: include/libtorrent/socks5_stream.hpp (SOCKS4 and
// SOCKS5 connect, the socks_error codes), src/socks5_stream.cpp (their messages),
// include/libtorrent/http_stream.hpp (HTTP CONNECT), src/udp_socket.cpp (the SOCKS5 UDP
// ASSOCIATE exchange, `wrap` and `unwrap`); fixes from 2.0.13 (bf2f3388cc)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: sans-IO state machines that say how many bytes they need next, so a caller that
// reads exactly that much never reads past the proxy's reply into the tunnelled stream (asio's
// `async_read` of a fixed size does the same). A host name target uses SOCKS4a (libtorrent's
// SOCKS4 only takes addresses). SOCKS5 replies of every address type are read in full
// (libtorrent's UDP associate asserts on an IPv6 bind address, and reads a domain reply shorter
// than three bytes past its buffer). HTTP CONNECT can name a host and send a `Host` header
// (`proxy_send_host_in_connect`). `unwrap` checks each length and drops unknown address types.

//! Proxy protocols: SOCKS4 / SOCKS4a (CONNECT), SOCKS5 (RFC 1928, with RFC 1929
//! username / password authentication; CONNECT and UDP ASSOCIATE) and HTTP CONNECT with basic
//! authentication, plus the SOCKS5 UDP datagram header.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::util::string::base64encode;

/// Where the proxy is asked to connect (or, for a UDP ASSOCIATE, where we send from).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Addr(SocketAddr),
    /// A host name the proxy resolves (`proxy_hostnames`).
    Host(String, u16),
}

impl Target {
    /// A host name that is an IP literal becomes an address.
    pub fn new(host: &str, port: u16) -> Target {
        let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
        match bare.parse::<IpAddr>() {
            Ok(ip) => Target::Addr(SocketAddr::new(ip, port)),
            Err(_) => Target::Host(host.to_owned(), port),
        }
    }

    pub fn port(&self) -> u16 {
        match self {
            Target::Addr(a) => a.port(),
            Target::Host(_, p) => *p,
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Addr(a) => write!(f, "{a}"),
            Target::Host(h, p) => write!(f, "{h}:{p}"),
        }
    }
}

/// What a proxy handshake can fail with: libtorrent's `socks_error` codes and the system errors
/// it maps SOCKS replies to, plus the HTTP proxy's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyError {
    UnsupportedVersion,
    UnsupportedAuthenticationMethod,
    UnsupportedAuthenticationVersion,
    AuthenticationError,
    UsernameRequired,
    GeneralFailure,
    CommandNotSupported,
    NoIdentd,
    IdentdError,
    /// SOCKS5 reply 2: the proxy's rules forbid the connection.
    NoPermission,
    NetworkUnreachable,
    HostUnreachable,
    ConnectionRefused,
    TimedOut,
    /// SOCKS5 reply 8, an IPv6 target for SOCKS4, or a reply with an unknown address type.
    AddressFamilyNotSupported,
    /// The HTTP proxy answered CONNECT with this status (libtorrent `operation_not_supported`).
    HttpStatus(u16),
    /// The HTTP proxy's answer is not an HTTP status line, or too long.
    InvalidReply,
}

impl ProxyError {
    /// The proxy itself refused or misunderstood us (version, authentication, a broken reply),
    /// rather than reporting that the target could not be reached through it.
    pub fn is_proxy_fault(&self) -> bool {
        use ProxyError::*;
        match self {
            UnsupportedVersion
            | UnsupportedAuthenticationMethod
            | UnsupportedAuthenticationVersion
            | AuthenticationError
            | UsernameRequired
            | CommandNotSupported
            | NoIdentd
            | IdentdError
            | InvalidReply => true,
            HttpStatus(s) => *s == 407 || *s < 100,
            _ => false,
        }
    }
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ProxyError::*;
        let s = match self {
            UnsupportedVersion => "SOCKS unsupported version",
            UnsupportedAuthenticationMethod => "SOCKS unsupported authentication method",
            UnsupportedAuthenticationVersion => "SOCKS unsupported authentication version",
            AuthenticationError => "SOCKS authentication error",
            UsernameRequired => "SOCKS username required",
            GeneralFailure => "SOCKS general failure",
            CommandNotSupported => "SOCKS command not supported",
            NoIdentd => "SOCKS no identd running",
            IdentdError => "SOCKS identd could not identify username",
            NoPermission => "Operation not permitted",
            NetworkUnreachable => "Network is unreachable",
            HostUnreachable => "No route to host",
            ConnectionRefused => "Connection refused",
            TimedOut => "Connection timed out",
            AddressFamilyNotSupported => "Address family not supported by protocol",
            HttpStatus(407) => "Operation not supported (HTTP proxy: 407 proxy authentication required)",
            HttpStatus(c) => return write!(f, "Operation not supported (HTTP proxy status {c})"),
            InvalidReply => "Operation not supported (invalid HTTP proxy reply)",
        };
        f.write_str(s)
    }
}

impl std::error::Error for ProxyError {}

/// The SOCKS5 command (libtorrent `socks5_stream::socks5_connect` / `socks5_udp_associate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Socks5Command {
    Connect = 1,
    UdpAssociate = 3,
}

/// The address a SOCKS5 proxy reports in its reply (`BND.ADDR` / `BND.PORT`): for a UDP
/// ASSOCIATE, where datagrams are to be sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bound {
    Addr(SocketAddr),
    Host(String, u16),
}

/// Progress of a handshake after some input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    NeedMore,
    /// The tunnel is up (or the association is made). SOCKS5 replies carry the bound address.
    Done(Option<Bound>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// SOCKS4: the 8-byte reply.
    Socks4Reply,
    /// SOCKS5: the chosen method.
    Method,
    /// SOCKS5: the username / password sub-negotiation's answer.
    Auth,
    /// SOCKS5: version, reply code, reserved, address type.
    ReplyHead,
    /// SOCKS5: the length of a domain name in the reply.
    ReplyNameLen,
    /// SOCKS5: the rest of the reply (address and port).
    ReplyTail(usize),
    /// HTTP: the response header, a byte at a time (libtorrent `http_stream::handshake2`).
    HttpHead,
    Done,
    Failed,
}

/// Longest HTTP CONNECT response header we wait for.
const MAX_HTTP_HEAD: usize = 8 * 1024;

/// The status code of an HTTP/1.x status line (RFC 9112: `HTTP/1.1 200 OK`; `HTTP` is
/// case-sensitive, one space on each side of the three-digit code, the reason phrase may be
/// left out). libtorrent reads the number after the first space of the line, so it took
/// replies such as `Foo 200` (SEC-15).
fn status_code(line: &[u8]) -> Option<u16> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let rest = line.strip_prefix(b"HTTP/1.")?;
    let (&minor, rest) = rest.split_first()?;
    let (code, reason) = rest.strip_prefix(b" ")?.split_at_checked(3)?;
    if !minor.is_ascii_digit() || !code.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if reason.first().is_some_and(|&b| b != b' ') {
        return None;
    }
    Some(code.iter().fold(0, |a, &b| a * 10 + u16::from(b - b'0')))
}

/// One proxy handshake over a TCP connection to the proxy.
#[derive(Clone, Debug)]
pub struct Handshake {
    stage: Stage,
    request: Vec<u8>,
    socks5_cmd: Option<Socks5Command>,
    target: Option<Target>,
    creds: Option<(String, String)>,
    /// The address type of a SOCKS5 reply.
    atyp: u8,
    buf: Vec<u8>,
}

fn write_target(out: &mut Vec<u8>, t: &Target) {
    match t {
        Target::Addr(SocketAddr::V4(a)) => {
            out.push(1);
            out.extend_from_slice(&a.ip().octets());
        }
        Target::Addr(SocketAddr::V6(a)) => {
            out.push(4);
            out.extend_from_slice(&a.ip().octets());
        }
        Target::Host(h, _) => {
            // libtorrent `set_dst_name` truncates to 255
            let h = &h.as_bytes()[..h.len().min(255)];
            out.push(3);
            out.push(h.len() as u8);
            out.extend_from_slice(h);
        }
    }
    out.extend_from_slice(&t.port().to_be_bytes());
}

fn push_short(out: &mut Vec<u8>, s: &str) {
    let s = &s.as_bytes()[..s.len().min(255)];
    out.push(s.len() as u8);
    out.extend_from_slice(s);
}

impl Handshake {
    /// SOCKS4 CONNECT (libtorrent `socks5_stream` with version 4). The user id is sent as is;
    /// a host name makes it SOCKS4a. SOCKS4 has no IPv6.
    pub fn socks4(target: &Target, user: &str) -> Result<Handshake, ProxyError> {
        let mut request = vec![4, Socks5Command::Connect as u8];
        request.extend_from_slice(&target.port().to_be_bytes());
        match target {
            Target::Addr(SocketAddr::V4(a)) => request.extend_from_slice(&a.ip().octets()),
            Target::Addr(SocketAddr::V6(_)) => return Err(ProxyError::AddressFamilyNotSupported),
            // SOCKS4a: 0.0.0.x with x != 0, the name after the user id
            Target::Host(..) => request.extend_from_slice(&[0, 0, 0, 1]),
        }
        request.extend_from_slice(user.as_bytes());
        request.push(0);
        if let Target::Host(h, _) = target {
            request.extend_from_slice(h.as_bytes());
            request.push(0);
        }
        Ok(Handshake {
            stage: Stage::Socks4Reply,
            request,
            socks5_cmd: None,
            target: None,
            creds: None,
            atyp: 0,
            buf: Vec::new(),
        })
    }

    /// SOCKS5 with `cmd`; `creds` offers username / password authentication besides none
    /// (libtorrent offers it for `socks5_pw` only).
    pub fn socks5(cmd: Socks5Command, target: &Target, creds: Option<(&str, &str)>) -> Handshake {
        let request = match creds {
            None => vec![5, 1, 0],
            Some(_) => vec![5, 2, 0, 2],
        };
        Handshake {
            stage: Stage::Method,
            request,
            socks5_cmd: Some(cmd),
            target: Some(target.clone()),
            creds: creds.map(|(u, p)| (u.to_owned(), p.to_owned())),
            atyp: 0,
            buf: Vec::new(),
        }
    }

    /// HTTP CONNECT (libtorrent `http_stream`), with basic authentication if `creds` is given
    /// and a `Host` header if `send_host`.
    pub fn http_connect(target: &Target, creds: Option<(&str, &str)>, send_host: bool) -> Handshake {
        let mut r = format!("CONNECT {target} HTTP/1.0\r\n");
        if send_host {
            r.push_str(&format!("Host: {target}\r\n"));
        }
        if let Some((u, p)) = creds {
            r.push_str("Proxy-Authorization: Basic ");
            r.push_str(&base64encode(format!("{u}:{p}").as_bytes()));
            r.push_str("\r\n");
        }
        r.push_str("\r\n");
        Handshake {
            stage: Stage::HttpHead,
            request: r.into_bytes(),
            socks5_cmd: None,
            target: None,
            creds: None,
            atyp: 0,
            buf: Vec::new(),
        }
    }

    /// Appends the first message to `out`.
    pub fn start(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.request);
    }

    pub fn is_done(&self) -> bool {
        self.stage == Stage::Done
    }

    /// How many bytes the handshake needs before it can go on: read exactly this many (or fewer)
    /// and nothing of the tunnelled stream is taken. 0 once it is over.
    pub fn want(&self) -> usize {
        let need = match self.stage {
            Stage::Socks4Reply => 8,
            Stage::Method | Stage::Auth => 2,
            Stage::ReplyHead => 4,
            Stage::ReplyNameLen => 1,
            Stage::ReplyTail(n) => n,
            Stage::HttpHead => 1,
            Stage::Done | Stage::Failed => return 0,
        };
        need - self.buf.len()
    }

    /// Takes bytes from the proxy; any message to send in reply is appended to `out`. Returns
    /// how many bytes of `data` were used (at most what the handshake needs; the rest belongs
    /// to the tunnel) and where it stands.
    pub fn feed(&mut self, mut data: &[u8], out: &mut Vec<u8>) -> Result<(usize, Step), ProxyError> {
        let mut used = 0;
        loop {
            let want = self.want();
            if want == 0 {
                return match self.stage {
                    Stage::Done => Ok((used, Step::Done(None))),
                    _ => Err(ProxyError::GeneralFailure),
                };
            }
            let n = want.min(data.len());
            self.buf.extend_from_slice(&data[..n]);
            data = &data[n..];
            used += n;
            if n < want {
                return Ok((used, Step::NeedMore));
            }
            match self.advance(out) {
                Ok(Some(step)) => return Ok((used, step)),
                Ok(None) => {}
                Err(e) => {
                    self.stage = Stage::Failed;
                    return Err(e);
                }
            }
        }
    }

    /// Acts on a complete `buf` for the current stage. `Some` once done.
    fn advance(&mut self, out: &mut Vec<u8>) -> Result<Option<Step>, ProxyError> {
        let buf = std::mem::take(&mut self.buf);
        match self.stage {
            Stage::Socks4Reply => {
                // libtorrent `connect2`, version 4
                if buf[0] != 0 {
                    return Err(ProxyError::GeneralFailure);
                }
                match buf[1] {
                    90 => {}
                    91 => return Err(ProxyError::ConnectionRefused),
                    92 => return Err(ProxyError::NoIdentd),
                    93 => return Err(ProxyError::IdentdError),
                    _ => return Err(ProxyError::GeneralFailure),
                }
                self.stage = Stage::Done;
                Ok(Some(Step::Done(None)))
            }
            Stage::Method => {
                // libtorrent `handshake2`
                if buf[0] != 5 {
                    return Err(ProxyError::UnsupportedVersion);
                }
                match buf[1] {
                    0 => self.send_command(out),
                    2 => {
                        let Some((user, pass)) = &self.creds else {
                            return Err(ProxyError::UsernameRequired);
                        };
                        out.push(1);
                        push_short(out, user);
                        push_short(out, pass);
                        self.stage = Stage::Auth;
                    }
                    _ => return Err(ProxyError::UnsupportedAuthenticationMethod),
                }
                Ok(None)
            }
            Stage::Auth => {
                // libtorrent `handshake4`
                if buf[0] != 1 {
                    return Err(ProxyError::UnsupportedAuthenticationVersion);
                }
                if buf[1] != 0 {
                    return Err(ProxyError::AuthenticationError);
                }
                self.send_command(out);
                Ok(None)
            }
            Stage::ReplyHead => {
                // libtorrent `connect2`, version 5
                if buf[0] != 5 {
                    return Err(ProxyError::UnsupportedVersion);
                }
                match buf[1] {
                    0 => {}
                    2 => return Err(ProxyError::NoPermission),
                    3 => return Err(ProxyError::NetworkUnreachable),
                    4 => return Err(ProxyError::HostUnreachable),
                    5 => return Err(ProxyError::ConnectionRefused),
                    6 => return Err(ProxyError::TimedOut),
                    7 => return Err(ProxyError::CommandNotSupported),
                    8 => return Err(ProxyError::AddressFamilyNotSupported),
                    _ => return Err(ProxyError::GeneralFailure),
                }
                self.atyp = buf[3];
                self.stage = match buf[3] {
                    1 => Stage::ReplyTail(4 + 2),
                    4 => Stage::ReplyTail(16 + 2),
                    3 => Stage::ReplyNameLen,
                    _ => return Err(ProxyError::AddressFamilyNotSupported),
                };
                Ok(None)
            }
            Stage::ReplyNameLen => {
                // the length byte stays in `buf`: the tail (name and port) comes after it
                self.stage = Stage::ReplyTail(1 + buf[0] as usize + 2);
                self.buf = buf;
                Ok(None)
            }
            Stage::ReplyTail(_) => {
                let atyp = self.atyp;
                let port = u16::from_be_bytes([buf[buf.len() - 2], buf[buf.len() - 1]]);
                let bound = match atyp {
                    1 => Bound::Addr(SocketAddr::new(
                        Ipv4Addr::new(buf[0], buf[1], buf[2], buf[3]).into(),
                        port,
                    )),
                    4 => {
                        let mut o = [0u8; 16];
                        o.copy_from_slice(&buf[..16]);
                        Bound::Addr(SocketAddr::new(Ipv6Addr::from(o).into(), port))
                    }
                    _ => {
                        let name = String::from_utf8_lossy(&buf[1..buf.len() - 2]).into_owned();
                        match name.parse::<IpAddr>() {
                            Ok(ip) => Bound::Addr(SocketAddr::new(ip, port)),
                            Err(_) => Bound::Host(name, port),
                        }
                    }
                };
                self.stage = Stage::Done;
                Ok(Some(Step::Done(Some(bound))))
            }
            Stage::HttpHead => {
                let mut head = std::mem::take(&mut self.request);
                head.extend_from_slice(&buf);
                let n = head.len();
                // libtorrent: "\n\n" or "\r\n\r\n" ends the header
                let end =
                    n > 2 && head[n - 1] == b'\n' && (head[n - 2] == b'\n' || head.ends_with(b"\r\n\r\n"));
                if !end {
                    if n > MAX_HTTP_HEAD {
                        return Err(ProxyError::InvalidReply);
                    }
                    self.request = head;
                    return Ok(None);
                }
                let line = head.split(|&b| b == b'\n').next().unwrap_or_default();
                let Some(code) = status_code(line) else {
                    return Err(ProxyError::InvalidReply);
                };
                if code != 200 {
                    return Err(ProxyError::HttpStatus(code));
                }
                self.stage = Stage::Done;
                Ok(Some(Step::Done(None)))
            }
            Stage::Done | Stage::Failed => Err(ProxyError::GeneralFailure),
        }
    }

    /// libtorrent `socks_connect` (version 5): the command with the target.
    fn send_command(&mut self, out: &mut Vec<u8>) {
        let cmd = self.socks5_cmd.expect("SOCKS5");
        let target = self.target.take().expect("set for SOCKS5");
        out.extend_from_slice(&[5, cmd as u8, 0]);
        write_target(out, &target);
        self.creds = None;
        self.stage = Stage::ReplyHead;
    }
}

/// The SOCKS5 UDP header for a datagram to an IPv4 address (libtorrent
/// `TORRENT_SOCKS5_HEADER` plus 4).
pub const UDP_HEADER_V4: usize = 10;

/// Appends a datagram for `target` with its SOCKS5 UDP header (libtorrent `udp_socket::wrap`).
pub fn udp_wrap(target: &Target, payload: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&[0, 0, 0]); // reserved, fragment 0
    write_target(out, target);
    out.extend_from_slice(payload);
}

/// Where a datagram from the proxy came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UdpSource<'a> {
    Addr(SocketAddr),
    Host(&'a str, u16),
}

/// Strips the SOCKS5 UDP header (libtorrent `udp_socket::unwrap`). `None` for a datagram that
/// is not one (too short, fragmented, an unknown address type).
pub fn udp_unwrap(packet: &[u8]) -> Option<(UdpSource<'_>, &[u8])> {
    if packet.len() <= UDP_HEADER_V4 || packet[2] != 0 {
        return None;
    }
    let p = &packet[4..];
    let (src, rest) = match packet[3] {
        1 => {
            let ip = Ipv4Addr::new(p[0], p[1], p[2], p[3]);
            (UdpSource::Addr(SocketAddr::new(ip.into(), u16::from_be_bytes([p[4], p[5]]))), &p[6..])
        }
        4 => {
            let (a, rest) = p.split_at_checked(18)?;
            let mut o = [0u8; 16];
            o.copy_from_slice(&a[..16]);
            let port = u16::from_be_bytes([a[16], a[17]]);
            (UdpSource::Addr(SocketAddr::new(Ipv6Addr::from(o).into(), port)), rest)
        }
        3 => {
            let len = p[0] as usize;
            let (name, rest) = p[1..].split_at_checked(len)?;
            let (port, rest) = rest.split_at_checked(2)?;
            let port = u16::from_be_bytes([port[0], port[1]]);
            let name = std::str::from_utf8(name).ok()?;
            let src = match name.parse::<IpAddr>() {
                Ok(ip) => UdpSource::Addr(SocketAddr::new(ip, port)),
                Err(_) => UdpSource::Host(name, port),
            };
            (src, rest)
        }
        _ => return None,
    };
    Some((src, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    /// Feeds the proxy's bytes one at a time, as `want` asks, and collects what we send.
    fn run(hs: &mut Handshake, reply: &[u8]) -> (Vec<u8>, Result<Step, ProxyError>, usize) {
        let mut out = Vec::new();
        hs.start(&mut out);
        let mut used = 0;
        let mut step = Ok(Step::NeedMore);
        while used < reply.len() && hs.want() > 0 {
            let n = hs.want().min(reply.len() - used);
            match hs.feed(&reply[used..used + n], &mut out) {
                Ok((k, s)) => {
                    assert_eq!(k, n);
                    used += k;
                    step = Ok(s);
                }
                Err(e) => {
                    step = Err(e);
                    break;
                }
            }
        }
        (out, step, used)
    }

    #[test]
    fn targets() {
        assert_eq!(Target::new("1.2.3.4", 80), Target::Addr(addr("1.2.3.4:80")));
        assert_eq!(Target::new("[::1]", 80), Target::Addr(addr("[::1]:80")));
        assert_eq!(Target::new("::1", 80), Target::Addr(addr("[::1]:80")));
        assert_eq!(Target::new("tracker.example", 80), Target::Host("tracker.example".into(), 80));
        assert_eq!(Target::new("tracker.example", 80).to_string(), "tracker.example:80");
        assert_eq!(Target::Addr(addr("[::1]:6881")).to_string(), "[::1]:6881");
    }

    #[test]
    fn socks4_connect() {
        let mut hs = Handshake::socks4(&Target::Addr(addr("10.0.0.1:6881")), "user").unwrap();
        let (out, step, used) = run(&mut hs, &[0, 90, 0, 0, 0, 0, 0, 0, b'x']);
        assert_eq!(out, [4, 1, 0x1a, 0xe1, 10, 0, 0, 1, b'u', b's', b'e', b'r', 0]);
        assert_eq!(step, Ok(Step::Done(None)));
        assert_eq!(used, 8, "the byte after the reply belongs to the tunnel");
        assert!(hs.is_done());

        // SOCKS4a
        let mut hs = Handshake::socks4(&Target::Host("h.example".into(), 80), "").unwrap();
        let (out, step, _) = run(&mut hs, &[0, 90, 0, 0, 0, 0, 0, 0]);
        assert_eq!(out, b"\x04\x01\x00\x50\x00\x00\x00\x01\x00h.example\x00");
        assert_eq!(step, Ok(Step::Done(None)));

        assert_eq!(
            Handshake::socks4(&Target::Addr(addr("[::1]:1")), "").err(),
            Some(ProxyError::AddressFamilyNotSupported)
        );
        for (code, err) in [
            (91, ProxyError::ConnectionRefused),
            (92, ProxyError::NoIdentd),
            (93, ProxyError::IdentdError),
            (94, ProxyError::GeneralFailure),
        ] {
            let mut hs = Handshake::socks4(&Target::Addr(addr("10.0.0.1:1")), "").unwrap();
            assert_eq!(run(&mut hs, &[0, code, 0, 0, 0, 0, 0, 0]).1, Err(err));
        }
        // the reply's version byte must be 0
        let mut hs = Handshake::socks4(&Target::Addr(addr("10.0.0.1:1")), "").unwrap();
        assert_eq!(run(&mut hs, &[4, 90, 0, 0, 0, 0, 0, 0]).1, Err(ProxyError::GeneralFailure));
    }

    #[test]
    fn socks5_connect_without_auth() {
        let t = Target::Addr(addr("1.2.3.4:6881"));
        let mut hs = Handshake::socks5(Socks5Command::Connect, &t, None);
        let reply = [5, 0, 5, 0, 0, 1, 9, 9, 9, 9, 0, 80, 0xff];
        let (out, step, used) = run(&mut hs, &reply);
        assert_eq!(out, [5, 1, 0, 5, 1, 0, 1, 1, 2, 3, 4, 0x1a, 0xe1]);
        assert_eq!(step, Ok(Step::Done(Some(Bound::Addr(addr("9.9.9.9:80"))))));
        assert_eq!(used, reply.len() - 1);
    }

    #[test]
    fn socks5_connect_with_auth_and_host_name() {
        let t = Target::Host("example.org".into(), 443);
        let mut hs = Handshake::socks5(Socks5Command::Connect, &t, Some(("al", "pw")));
        // method 2, auth ok, reply with an IPv6 bind address
        let mut reply = vec![5, 2, 1, 0, 5, 0, 0, 4];
        reply.extend_from_slice(&[0; 15]);
        reply.extend_from_slice(&[1, 0x1f, 0x90]);
        let (out, step, used) = run(&mut hs, &reply);
        let mut want = vec![5, 2, 0, 2, 1, 2, b'a', b'l', 2, b'p', b'w', 5, 1, 0, 3, 11];
        want.extend_from_slice(b"example.org");
        want.extend_from_slice(&[1, 0xbb]);
        assert_eq!(out, want);
        assert_eq!(step, Ok(Step::Done(Some(Bound::Addr(addr("[::1]:8080"))))));
        assert_eq!(used, reply.len());

        // a domain name in the reply
        let mut hs = Handshake::socks5(Socks5Command::UdpAssociate, &Target::Addr(addr("0.0.0.0:0")), None);
        let mut reply = vec![5, 0, 5, 0, 0, 3, 5];
        reply.extend_from_slice(b"relay");
        reply.extend_from_slice(&[0x10, 0x00]);
        let (out, step, _) = run(&mut hs, &reply);
        assert_eq!(out, [5, 1, 0, 5, 3, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(step, Ok(Step::Done(Some(Bound::Host("relay".into(), 4096)))));
        // an empty domain name
        let mut hs = Handshake::socks5(Socks5Command::UdpAssociate, &Target::Addr(addr("0.0.0.0:0")), None);
        let (_, step, used) = run(&mut hs, &[5, 0, 5, 0, 0, 3, 0, 0, 7]);
        assert_eq!(step, Ok(Step::Done(Some(Bound::Host(String::new(), 7)))));
        assert_eq!(used, 9);
    }

    #[test]
    fn socks5_failures() {
        let t = Target::Addr(addr("1.2.3.4:1"));
        let fail = |creds: Option<(&str, &str)>, reply: &[u8]| {
            let mut hs = Handshake::socks5(Socks5Command::Connect, &t, creds);
            run(&mut hs, reply).1
        };
        assert_eq!(fail(None, &[4, 0]), Err(ProxyError::UnsupportedVersion));
        assert_eq!(fail(None, &[5, 2]), Err(ProxyError::UsernameRequired));
        assert_eq!(fail(None, &[5, 0xff]), Err(ProxyError::UnsupportedAuthenticationMethod));
        assert_eq!(fail(Some(("u", "p")), &[5, 2, 0, 0]), Err(ProxyError::UnsupportedAuthenticationVersion));
        assert_eq!(fail(Some(("u", "p")), &[5, 2, 1, 1]), Err(ProxyError::AuthenticationError));
        assert_eq!(fail(None, &[5, 0, 4, 0, 0, 1]), Err(ProxyError::UnsupportedVersion));
        // 2.0.13 (bf2f3388cc): the version must be exactly 5, not at least 5
        assert_eq!(fail(None, &[6, 0]), Err(ProxyError::UnsupportedVersion));
        assert_eq!(fail(None, &[5, 0, 6, 0, 0, 1]), Err(ProxyError::UnsupportedVersion));
        for (code, e) in [
            (1, ProxyError::GeneralFailure),
            (2, ProxyError::NoPermission),
            (3, ProxyError::NetworkUnreachable),
            (4, ProxyError::HostUnreachable),
            (5, ProxyError::ConnectionRefused),
            (6, ProxyError::TimedOut),
            (7, ProxyError::CommandNotSupported),
            (8, ProxyError::AddressFamilyNotSupported),
            (9, ProxyError::GeneralFailure),
        ] {
            assert_eq!(fail(None, &[5, 0, 5, code, 0, 1]), Err(e));
        }
        assert_eq!(fail(None, &[5, 0, 5, 0, 0, 2]), Err(ProxyError::AddressFamilyNotSupported));
        // nothing more is taken after a failure
        let mut hs = Handshake::socks5(Socks5Command::Connect, &t, None);
        assert!(run(&mut hs, &[4, 0]).1.is_err());
        assert_eq!(hs.want(), 0);
        assert!(hs.feed(&[1], &mut Vec::new()).is_err());
    }

    #[test]
    fn http_connect() {
        let t = Target::Addr(addr("[2001:db8::1]:6881"));
        let mut hs = Handshake::http_connect(&t, Some(("Aladdin", "open sesame")), false);
        let reply = b"HTTP/1.0 200 Connection established\r\nProxy-agent: x\r\n\r\nBT";
        let (out, step, used) = run(&mut hs, reply);
        assert_eq!(
            out,
            b"CONNECT [2001:db8::1]:6881 HTTP/1.0\r\n\
              Proxy-Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==\r\n\r\n"
        );
        assert_eq!(step, Ok(Step::Done(None)));
        assert_eq!(used, reply.len() - 2, "the tunnelled bytes are left alone");

        let mut hs = Handshake::http_connect(&Target::Host("t.example".into(), 443), None, true);
        let (out, step, _) = run(&mut hs, b"HTTP/1.1 200 OK\n\n");
        assert_eq!(out, b"CONNECT t.example:443 HTTP/1.0\r\nHost: t.example:443\r\n\r\n");
        assert_eq!(step, Ok(Step::Done(None)));

        let mut hs = Handshake::http_connect(&t, None, false);
        let (_, step, _) = run(&mut hs, b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n");
        assert_eq!(step, Err(ProxyError::HttpStatus(407)));
        assert!(ProxyError::HttpStatus(407).is_proxy_fault());
        assert!(!ProxyError::HttpStatus(502).is_proxy_fault());
        let mut hs = Handshake::http_connect(&t, None, false);
        assert_eq!(run(&mut hs, b"garbage\r\n\r\n").1, Err(ProxyError::InvalidReply));
        let mut hs = Handshake::http_connect(&t, None, false);
        let long = vec![b'a'; MAX_HTTP_HEAD + 10];
        assert_eq!(run(&mut hs, &long).1, Err(ProxyError::InvalidReply));
    }

    // SEC-15: only an HTTP/1.x status line with the code 200 opens the tunnel
    #[test]
    fn http_connect_needs_a_status_line() {
        let t = Target::Addr(addr("1.2.3.4:6881"));
        let reply = |r: &[u8]| run(&mut Handshake::http_connect(&t, None, false), r).1;
        for bad in [
            &b"Foo 200 OK\r\n\r\n"[..],
            b"<html> 200\r\n\r\n",
            b"http/1.1 200 OK\r\n\r\n",
            b"HTTP/2 200\r\n\r\n",
            b"HTTP/1.x 200 OK\r\n\r\n",
            b"HTTP/1.1  200 OK\r\n\r\n",
            b"HTTP/1.1 2000 OK\r\n\r\n",
            b"HTTP/1.1 200OK\r\n\r\n",
            b"HTTP/1.1 20\r\n\r\n",
            b"X-Status: HTTP/1.1 200\r\n\r\n",
        ] {
            assert_eq!(reply(bad), Err(ProxyError::InvalidReply), "{}", String::from_utf8_lossy(bad));
        }
        for good in [
            &b"HTTP/1.0 200\r\n\r\n"[..],
            b"HTTP/1.1 200 \n\n",
            b"HTTP/1.1 200 Connection established\r\n\r\n",
        ] {
            assert_eq!(reply(good), Ok(Step::Done(None)), "{}", String::from_utf8_lossy(good));
        }
        assert_eq!(reply(b"HTTP/1.1 502 Bad Gateway\r\n\r\n"), Err(ProxyError::HttpStatus(502)));
        assert_eq!(reply(b"HTTP/1.0 407\n\n"), Err(ProxyError::HttpStatus(407)));
    }

    #[test]
    fn feeding_whole_buffers() {
        // a caller may hand over more than `want`: only the handshake's bytes are used
        let mut hs = Handshake::socks5(Socks5Command::Connect, &Target::Addr(addr("1.2.3.4:1")), None);
        let mut out = Vec::new();
        hs.start(&mut out);
        let (used, step) = hs.feed(&[5, 0, 5, 0, 0, 1, 1, 2, 3, 4, 0, 1, 42, 43], &mut out).unwrap();
        assert_eq!((used, step), (12, Step::Done(Some(Bound::Addr(addr("1.2.3.4:1"))))));
    }

    #[test]
    fn udp_framing() {
        let mut out = Vec::new();
        udp_wrap(&Target::Addr(addr("1.2.3.4:6881")), b"hi", &mut out);
        assert_eq!(out, [0, 0, 0, 1, 1, 2, 3, 4, 0x1a, 0xe1, b'h', b'i']);
        assert_eq!(out.len(), UDP_HEADER_V4 + 2);
        assert_eq!(udp_unwrap(&out), Some((UdpSource::Addr(addr("1.2.3.4:6881")), &b"hi"[..])));

        let mut out = Vec::new();
        udp_wrap(&Target::Addr(addr("[::1]:5")), b"x", &mut out);
        assert_eq!(udp_unwrap(&out), Some((UdpSource::Addr(addr("[::1]:5")), &b"x"[..])));

        let mut out = Vec::new();
        udp_wrap(&Target::Host("tracker.example".into(), 80), b"pkt", &mut out);
        assert_eq!(out[3..5], [3, 15]);
        assert_eq!(udp_unwrap(&out), Some((UdpSource::Host("tracker.example", 80), &b"pkt"[..])));

        // fragments, short packets, unknown address types and truncated names are dropped
        let mut frag = Vec::new();
        udp_wrap(&Target::Addr(addr("1.2.3.4:1")), b"abc", &mut frag);
        frag[2] = 1;
        assert_eq!(udp_unwrap(&frag), None);
        assert_eq!(udp_unwrap(&[0, 0, 0, 1, 1, 2, 3, 4, 0, 1]), None);
        assert_eq!(udp_unwrap(&[0, 0, 0, 9, 1, 2, 3, 4, 0, 1, 1]), None);
        assert_eq!(udp_unwrap(&[0, 0, 0, 4, 1, 2, 3, 4, 0, 1, 1]), None);
        assert_eq!(udp_unwrap(&[0, 0, 0, 3, 200, 2, 3, 4, 0, 1, 1]), None);
    }
}
