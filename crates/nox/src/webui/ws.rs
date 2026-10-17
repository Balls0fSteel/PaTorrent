//! Sans-IO WebSocket (RFC 6455) server side: the opening handshake (`Sec-WebSocket-Accept` is
//! SHA-1 plus base64, both our own), frame parsing with the client masking RFC 6455 section
//! 5.1 makes mandatory, message assembly from fragments, and frame writing. Everything here
//! reads untrusted bytes from the network and is fuzzed (`fuzz/nox_ws`, when it lands).
//!
//! The protocol on top (`webui/native.rs`) uses text frames only; a binary data frame is
//! answered with a 1003 close.

use patorrent_core::crypto::Sha1;

use super::http::Head;

/// RFC 6455 section 1.3.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Standard base64 with padding (RFC 4648 section 4).
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, &b)| acc | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The `Sec-WebSocket-Accept` value for a client's `Sec-WebSocket-Key` (RFC 6455 section 4.2.2).
pub fn accept_key(key: &str) -> String {
    let mut h = Sha1::new();
    h.update(key.trim().as_bytes()).update(GUID.as_bytes());
    base64(&h.finalize().0)
}

/// Why an upgrade request was refused, with the status to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeError {
    /// No `Upgrade: websocket` / `Connection: upgrade` pair, or no key: 400.
    NotAnUpgrade,
    /// A `Sec-WebSocket-Version` other than 13: 426.
    Version,
}

impl HandshakeError {
    pub fn status(self) -> u16 {
        match self {
            HandshakeError::NotAnUpgrade => 400,
            HandshakeError::Version => 426,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            HandshakeError::NotAnUpgrade => "expected a WebSocket upgrade",
            HandshakeError::Version => "unsupported WebSocket version",
        }
    }
}

fn has_token(value: &str, token: &str) -> bool {
    value.split(',').any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// Checks the upgrade headers of a `GET` (RFC 6455 section 4.2.1) and returns the client's key.
/// Cookies, `Origin` and the host are the caller's business.
pub fn handshake_key(head: &Head) -> Result<&str, HandshakeError> {
    let upgrade = head.header("upgrade").is_some_and(|v| has_token(v, "websocket"));
    let connection = head.header("connection").is_some_and(|v| has_token(v, "upgrade"));
    let key = head.header("sec-websocket-key").map(str::trim).filter(|k| k.len() == 24);
    if head.method != "GET" || !upgrade || !connection {
        return Err(HandshakeError::NotAnUpgrade);
    }
    let Some(key) = key else { return Err(HandshakeError::NotAnUpgrade) };
    if head.header("sec-websocket-version").map(str::trim) != Some("13") {
        return Err(HandshakeError::Version);
    }
    Ok(key)
}

/// The `101 Switching Protocols` response for `key`.
pub fn handshake_response(key: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
Sec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    )
    .into_bytes()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opcode {
    Continuation = 0,
    Text = 1,
    Binary = 2,
    Close = 8,
    Ping = 9,
    Pong = 10,
}

impl Opcode {
    fn from_u8(b: u8) -> Option<Opcode> {
        Some(match b {
            0 => Opcode::Continuation,
            1 => Opcode::Text,
            2 => Opcode::Binary,
            8 => Opcode::Close,
            9 => Opcode::Ping,
            10 => Opcode::Pong,
            _ => return None,
        })
    }

    pub fn is_control(self) -> bool {
        (self as u8) >= 8
    }
}

/// One parsed (unmasked) frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: Opcode,
    pub payload: Vec<u8>,
}

/// A protocol violation, with the close code to send (RFC 6455 section 7.4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Reserved bits set, an unknown opcode, a fragmented or oversized control frame, a bad
    /// fragment sequence: 1002.
    Protocol(&'static str),
    /// A client frame without a mask: 1002 (RFC 6455 section 5.1).
    Unmasked,
    /// A binary data message: 1003.
    Unsupported,
    /// Text that is not UTF-8: 1007.
    BadText,
    /// A frame or message over the limit: 1009.
    TooLarge,
}

impl FrameError {
    pub fn code(self) -> u16 {
        match self {
            FrameError::Protocol(_) | FrameError::Unmasked => 1002,
            FrameError::Unsupported => 1003,
            FrameError::BadText => 1007,
            FrameError::TooLarge => 1009,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            FrameError::Protocol(why) => why,
            FrameError::Unmasked => "client frames must be masked",
            FrameError::Unsupported => "binary messages are not supported",
            FrameError::BadText => "text is not UTF-8",
            FrameError::TooLarge => "message too large",
        }
    }
}

/// Parses one client frame from the start of `buf`. `Ok(None)`: not complete yet. On success,
/// also returns how many bytes the frame took. Payloads over `max_payload` are refused before
/// they are buffered.
pub fn parse_frame(buf: &[u8], max_payload: usize) -> Result<Option<(Frame, usize)>, FrameError> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let (b0, b1) = (buf[0], buf[1]);
    if b0 & 0x70 != 0 {
        return Err(FrameError::Protocol("reserved bits set"));
    }
    let fin = b0 & 0x80 != 0;
    let opcode = Opcode::from_u8(b0 & 0x0f).ok_or(FrameError::Protocol("unknown opcode"))?;
    if b1 & 0x80 == 0 {
        return Err(FrameError::Unmasked);
    }
    let (len, mut at) = match b1 & 0x7f {
        n @ 0..=125 => (n as u64, 2usize),
        126 => {
            if buf.len() < 4 {
                return Ok(None);
            }
            (u16::from_be_bytes([buf[2], buf[3]]) as u64, 4)
        }
        _ => {
            if buf.len() < 10 {
                return Ok(None);
            }
            let n = u64::from_be_bytes(buf[2..10].try_into().expect("8 bytes"));
            if n >> 63 != 0 {
                return Err(FrameError::Protocol("bad length"));
            }
            (n, 10)
        }
    };
    if opcode.is_control() {
        if !fin {
            return Err(FrameError::Protocol("fragmented control frame"));
        }
        if len > 125 {
            return Err(FrameError::Protocol("control frame too large"));
        }
    }
    if len > max_payload as u64 {
        return Err(FrameError::TooLarge);
    }
    let len = len as usize;
    if buf.len() < at + 4 + len {
        return Ok(None);
    }
    let mask: [u8; 4] = buf[at..at + 4].try_into().expect("4 bytes");
    at += 4;
    let payload: Vec<u8> = buf[at..at + len].iter().enumerate().map(|(i, &b)| b ^ mask[i % 4]).collect();
    Ok(Some((Frame { fin, opcode, payload }, at + len)))
}

/// Appends an unmasked server frame.
pub fn write_frame(opcode: Opcode, payload: &[u8], out: &mut Vec<u8>) {
    out.push(0x80 | opcode as u8);
    let len = payload.len();
    if len < 126 {
        out.push(len as u8);
    } else if len <= u16::MAX as usize {
        out.push(126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
}

pub fn write_text(text: &str, out: &mut Vec<u8>) {
    write_frame(Opcode::Text, text.as_bytes(), out);
}

/// A close frame with a status code and a short reason (RFC 6455 section 5.5.1).
pub fn write_close(code: u16, reason: &str, out: &mut Vec<u8>) {
    let mut payload = code.to_be_bytes().to_vec();
    // A control frame holds at most 125 bytes; cut the reason on a character boundary.
    let mut reason = reason;
    while reason.len() > 123 {
        let mut end = reason.len() - 1;
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        reason = &reason[..end];
    }
    payload.extend_from_slice(reason.as_bytes());
    write_frame(Opcode::Close, &payload, out);
}

/// A complete client message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// The status code (1005 when the frame carried none) and the reason.
    Close(u16, String),
}

/// Assembles messages from frames, text only, fragments allowed, control frames in between.
#[derive(Debug)]
pub struct Assembler {
    max_message: usize,
    /// A text message in progress (its fragments so far).
    partial: Option<Vec<u8>>,
}

impl Assembler {
    pub fn new(max_message: usize) -> Assembler {
        Assembler { max_message, partial: None }
    }

    /// Feeds one frame; a complete message comes back when the frame finished one.
    pub fn push(&mut self, f: Frame) -> Result<Option<Message>, FrameError> {
        match f.opcode {
            Opcode::Ping => return Ok(Some(Message::Ping(f.payload))),
            Opcode::Pong => return Ok(Some(Message::Pong(f.payload))),
            Opcode::Close => {
                let (code, reason) = match f.payload.len() {
                    0 => (1005, String::new()),
                    1 => return Err(FrameError::Protocol("close frame with a one-byte payload")),
                    _ => {
                        let code = u16::from_be_bytes([f.payload[0], f.payload[1]]);
                        if !valid_close_code(code) {
                            return Err(FrameError::Protocol("bad close code"));
                        }
                        let reason =
                            String::from_utf8(f.payload[2..].to_vec()).map_err(|_| FrameError::BadText)?;
                        (code, reason)
                    }
                };
                return Ok(Some(Message::Close(code, reason)));
            }
            Opcode::Binary => return Err(FrameError::Unsupported),
            Opcode::Text => {
                if self.partial.is_some() {
                    return Err(FrameError::Protocol("new message inside a fragmented one"));
                }
                self.partial = Some(Vec::new());
            }
            Opcode::Continuation => {
                if self.partial.is_none() {
                    return Err(FrameError::Protocol("continuation without a message"));
                }
            }
        }
        let partial = self.partial.as_mut().expect("set above");
        if partial.len() + f.payload.len() > self.max_message {
            self.partial = None;
            return Err(FrameError::TooLarge);
        }
        partial.extend_from_slice(&f.payload);
        if !f.fin {
            return Ok(None);
        }
        let bytes = self.partial.take().expect("set above");
        let text = String::from_utf8(bytes).map_err(|_| FrameError::BadText)?;
        Ok(Some(Message::Text(text)))
    }
}

/// Codes a close frame may carry (RFC 6455 section 7.4).
fn valid_close_code(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1011 | 3000..=4999)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webui::http::{Limits, parse_head};

    #[test]
    fn base64_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn accept_key_rfc6455_example() {
        // RFC 6455 section 1.3
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        let r = String::from_utf8(handshake_response("dGhlIHNhbXBsZSBub25jZQ==")).unwrap();
        assert!(r.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
        assert!(r.contains("\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
        assert!(r.ends_with("\r\n\r\n"));
    }

    fn head(extra: &str) -> Head {
        let req = format!("GET /pa/ws HTTP/1.1\r\nHost: a\r\n{extra}\r\n");
        parse_head(req.as_bytes(), &Limits::default()).unwrap().unwrap().0
    }

    #[test]
    fn handshake_headers() {
        let ok = "Upgrade: WebSocket\r\nConnection: keep-alive, Upgrade\r\n\
Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n";
        assert_eq!(handshake_key(&head(ok)), Ok("dGhlIHNhbXBsZSBub25jZQ=="));
        let no_upgrade = "Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
Sec-WebSocket-Version: 13\r\n";
        assert_eq!(handshake_key(&head(no_upgrade)), Err(HandshakeError::NotAnUpgrade));
        let short_key = "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: abc\r\n\
Sec-WebSocket-Version: 13\r\n";
        assert_eq!(handshake_key(&head(short_key)), Err(HandshakeError::NotAnUpgrade));
        let old = "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
Sec-WebSocket-Version: 8\r\n";
        assert_eq!(handshake_key(&head(old)), Err(HandshakeError::Version));
        assert_eq!(HandshakeError::Version.status(), 426);
        let mut post = head(ok);
        post.method = "POST".into();
        assert_eq!(handshake_key(&post), Err(HandshakeError::NotAnUpgrade));
    }

    fn masked(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode];
        let len = payload.len();
        if len < 126 {
            out.push(0x80 | len as u8);
        } else if len <= u16::MAX as usize {
            out.push(0x80 | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(0x80 | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, &b)| b ^ mask[i % 4]));
        out
    }

    #[test]
    fn parses_masked_frames() {
        // RFC 6455 section 5.7: a single-frame masked text message
        let rfc = [0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58];
        let (f, n) = parse_frame(&rfc, 1024).unwrap().unwrap();
        assert_eq!(n, rfc.len());
        assert_eq!(f, Frame { fin: true, opcode: Opcode::Text, payload: b"Hello".to_vec() });
        assert_eq!(masked(true, 1, b"Hello"), rfc);

        // incomplete frames wait
        assert_eq!(parse_frame(&rfc[..1], 1024), Ok(None));
        assert_eq!(parse_frame(&rfc[..6], 1024), Ok(None));
        assert_eq!(parse_frame(&rfc[..10], 1024), Ok(None));

        // 16-bit and 64-bit lengths
        let big = vec![7u8; 300];
        let (f, n) = parse_frame(&masked(true, 2, &big), 1024).unwrap().unwrap();
        assert_eq!((f.opcode, f.payload.len(), n), (Opcode::Binary, 300, 2 + 2 + 4 + 300));
        let huge = vec![1u8; 70_000];
        let (f, n) = parse_frame(&masked(true, 1, &huge), 100_000).unwrap().unwrap();
        assert_eq!((f.payload.len(), n), (70_000, 2 + 8 + 4 + 70_000));
        assert_eq!(parse_frame(&masked(true, 1, &huge), 1024), Err(FrameError::TooLarge));
        assert_eq!(parse_frame(&masked(true, 1, &huge)[..4], 1024), Ok(None));
    }

    #[test]
    fn refuses_bad_frames() {
        // RFC 6455 section 5.7: the same text unmasked
        let unmasked = [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
        assert_eq!(parse_frame(&unmasked, 1024), Err(FrameError::Unmasked));
        let mut rsv = masked(true, 1, b"x");
        rsv[0] |= 0x40;
        assert!(matches!(parse_frame(&rsv, 1024), Err(FrameError::Protocol(_))));
        assert!(matches!(parse_frame(&masked(true, 3, b"x"), 1024), Err(FrameError::Protocol(_))));
        assert!(matches!(parse_frame(&masked(false, 9, b"x"), 1024), Err(FrameError::Protocol(_))));
        assert!(matches!(parse_frame(&masked(true, 9, &[0; 126]), 1024), Err(FrameError::Protocol(_))));
        let mut neg = masked(true, 1, &[0; 70_000]);
        neg[2] = 0x80;
        assert!(matches!(parse_frame(&neg, usize::MAX), Err(FrameError::Protocol(_))));
        assert_eq!(FrameError::Unmasked.code(), 1002);
        assert_eq!(FrameError::TooLarge.code(), 1009);
    }

    fn frame(fin: bool, opcode: Opcode, payload: &[u8]) -> Frame {
        Frame { fin, opcode, payload: payload.to_vec() }
    }

    #[test]
    fn assembles_messages() {
        let mut a = Assembler::new(64);
        assert_eq!(a.push(frame(true, Opcode::Text, b"{}")).unwrap(), Some(Message::Text("{}".into())));
        assert_eq!(a.push(frame(false, Opcode::Text, b"ab")).unwrap(), None);
        assert_eq!(a.push(frame(true, Opcode::Ping, b"p")).unwrap(), Some(Message::Ping(b"p".to_vec())));
        assert_eq!(a.push(frame(false, Opcode::Continuation, b"c")).unwrap(), None);
        assert_eq!(
            a.push(frame(true, Opcode::Continuation, b"d")).unwrap(),
            Some(Message::Text("abcd".into()))
        );
        assert!(matches!(a.push(frame(true, Opcode::Continuation, b"x")), Err(FrameError::Protocol(_))));
        assert_eq!(a.push(frame(false, Opcode::Text, b"ab")).unwrap(), None);
        assert!(matches!(a.push(frame(true, Opcode::Text, b"x")), Err(FrameError::Protocol(_))));
        let mut a = Assembler::new(64);
        assert_eq!(a.push(frame(true, Opcode::Binary, b"x")), Err(FrameError::Unsupported));
        assert_eq!(a.push(frame(true, Opcode::Text, &[0xff])), Err(FrameError::BadText));
        assert_eq!(a.push(frame(false, Opcode::Text, &[0; 40])).unwrap(), None);
        assert_eq!(a.push(frame(true, Opcode::Continuation, &[0; 40])), Err(FrameError::TooLarge));
        assert_eq!(a.push(frame(true, Opcode::Pong, b"")).unwrap(), Some(Message::Pong(Vec::new())));
        // close frames
        assert_eq!(
            a.push(frame(true, Opcode::Close, b"")).unwrap(),
            Some(Message::Close(1005, String::new()))
        );
        assert_eq!(
            a.push(frame(true, Opcode::Close, b"\x03\xe8bye")).unwrap(),
            Some(Message::Close(1000, "bye".into()))
        );
        assert!(matches!(a.push(frame(true, Opcode::Close, b"\x03")), Err(FrameError::Protocol(_))));
        assert!(matches!(a.push(frame(true, Opcode::Close, b"\x03\xed")), Err(FrameError::Protocol(_))));
    }

    #[test]
    fn writes_frames() {
        let mut out = Vec::new();
        write_text("Hello", &mut out);
        assert_eq!(out, [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]);
        out.clear();
        write_frame(Opcode::Pong, &[0; 200], &mut out);
        assert_eq!(&out[..4], &[0x8a, 126, 0, 200]);
        assert_eq!(out.len(), 204);
        out.clear();
        write_frame(Opcode::Text, &[0; 70_000], &mut out);
        assert_eq!(&out[..10], &[0x81, 127, 0, 0, 0, 0, 0, 1, 0x11, 0x70]);
        out.clear();
        write_close(1000, "bye", &mut out);
        assert_eq!(out, [0x88, 5, 0x03, 0xe8, b'b', b'y', b'e']);
        out.clear();
        write_close(1009, &"é".repeat(70), &mut out);
        assert_eq!(out[1], 2 + 122);
    }
}
