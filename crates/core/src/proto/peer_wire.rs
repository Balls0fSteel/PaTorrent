// Ported from libtorrent-rasterbar 2.0.11: src/bt_peer_connection.cpp (message framing,
// handshake, BEP 6 fast extension, BEP 10 extension handshake), src/identify_client.cpp (subset),
// src/generate_peer_id.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a sans-IO codec. `decode` borrows messages straight out of the receive buffer and
// `encode_*` append to a send buffer; the connection logic lives in `engine::peer`.

//! BitTorrent peer wire protocol (BEP 3, BEP 6, BEP 10).

use crate::bencode::{Entry, bdecode};
use crate::types::Sha1Hash;

pub const PROTOCOL: &[u8; 19] = b"BitTorrent protocol";
pub const HANDSHAKE_LEN: usize = 68;

/// Largest message we accept. A bitfield for libtorrent's 2M-piece limit is 256 KiB; piece
/// messages carry at most one 16 KiB block (libtorrent caps receive at about the same size).
pub const MAX_MESSAGE_LEN: usize = 512 * 1024;

pub mod msg {
    pub const CHOKE: u8 = 0;
    pub const UNCHOKE: u8 = 1;
    pub const INTERESTED: u8 = 2;
    pub const NOT_INTERESTED: u8 = 3;
    pub const HAVE: u8 = 4;
    pub const BITFIELD: u8 = 5;
    pub const REQUEST: u8 = 6;
    pub const PIECE: u8 = 7;
    pub const CANCEL: u8 = 8;
    pub const PORT: u8 = 9;
    // BEP 6
    pub const SUGGEST: u8 = 0x0d;
    pub const HAVE_ALL: u8 = 0x0e;
    pub const HAVE_NONE: u8 = 0x0f;
    pub const REJECT: u8 = 0x10;
    pub const ALLOWED_FAST: u8 = 0x11;
    // BEP 10
    pub const EXTENDED: u8 = 20;
}

/// Reserved-bit flags from the handshake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reserved(pub [u8; 8]);

impl Reserved {
    /// What we advertise: BEP 10 extensions and the BEP 6 fast extension.
    pub fn ours() -> Reserved {
        let mut r = [0u8; 8];
        r[5] |= 0x10;
        r[7] |= 0x04;
        Reserved(r)
    }

    pub fn supports_extensions(&self) -> bool {
        self.0[5] & 0x10 != 0
    }

    pub fn supports_fast(&self) -> bool {
        self.0[7] & 0x04 != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handshake {
    pub reserved: Reserved,
    pub info_hash: Sha1Hash,
    pub peer_id: [u8; 20],
}

pub fn encode_handshake(out: &mut Vec<u8>, info_hash: &Sha1Hash, peer_id: &[u8; 20]) {
    out.push(PROTOCOL.len() as u8);
    out.extend_from_slice(PROTOCOL);
    out.extend_from_slice(&Reserved::ours().0);
    out.extend_from_slice(&info_hash.0);
    out.extend_from_slice(peer_id);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeError {
    /// Fewer bytes than needed so far.
    Incomplete,
    /// Not the BitTorrent protocol (or encrypted, which Phase 4 adds).
    BadProtocol,
}

/// Parses the first 48 bytes (up to and including the info-hash) of a handshake. Incoming
/// connections need the info-hash before the peer id arrives, to pick the torrent.
pub fn decode_handshake_prefix(buf: &[u8]) -> Result<(Reserved, Sha1Hash), HandshakeError> {
    if !buf.is_empty() && buf[0] != 19 {
        return Err(HandshakeError::BadProtocol);
    }
    let n = buf.len().min(20);
    if n > 1 && buf[1..n] != PROTOCOL[..n - 1] {
        return Err(HandshakeError::BadProtocol);
    }
    if buf.len() < 48 {
        return Err(HandshakeError::Incomplete);
    }
    let reserved = Reserved(buf[20..28].try_into().expect("8 bytes"));
    Ok((reserved, Sha1Hash::from_slice(&buf[28..48]).expect("20 bytes")))
}

pub fn decode_handshake(buf: &[u8]) -> Result<Handshake, HandshakeError> {
    let (reserved, info_hash) = decode_handshake_prefix(buf)?;
    if buf.len() < HANDSHAKE_LEN {
        return Err(HandshakeError::Incomplete);
    }
    Ok(Handshake { reserved, info_hash, peer_id: buf[48..68].try_into().expect("20 bytes") })
}

/// One decoded message. Payload slices borrow from the receive buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message<'a> {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(&'a [u8]),
    Request {
        piece: u32,
        start: u32,
        length: u32,
    },
    Piece {
        piece: u32,
        start: u32,
        data: &'a [u8],
    },
    Cancel {
        piece: u32,
        start: u32,
        length: u32,
    },
    Port(u16),
    Suggest(u32),
    HaveAll,
    HaveNone,
    Reject {
        piece: u32,
        start: u32,
        length: u32,
    },
    AllowedFast(u32),
    Extended {
        id: u8,
        payload: &'a [u8],
    },
    /// A message id we do not know; libtorrent ignores these.
    Unknown {
        id: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// The length prefix exceeds [`MAX_MESSAGE_LEN`].
    TooLarge(u32),
    /// A known message with the wrong length.
    BadLength { id: u8, len: u32 },
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::TooLarge(n) => write!(f, "packet too large ({n} bytes)"),
            WireError::BadLength { id, len } => write!(f, "invalid size {len} for message {id}"),
        }
    }
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().expect("4 bytes"))
}

/// Decodes one message from the front of `buf`. Returns `Ok(None)` when more bytes are
/// needed, otherwise the message and the number of bytes it used.
pub fn decode(buf: &[u8]) -> Result<Option<(Message<'_>, usize)>, WireError> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = be32(buf);
    if len as usize > MAX_MESSAGE_LEN {
        return Err(WireError::TooLarge(len));
    }
    let total = 4 + len as usize;
    if buf.len() < total {
        return Ok(None);
    }
    if len == 0 {
        return Ok(Some((Message::KeepAlive, 4)));
    }
    let id = buf[4];
    let p = &buf[5..total];
    let bad = || WireError::BadLength { id, len };
    let exact = |n: usize| if p.len() == n { Ok(()) } else { Err(bad()) };
    let triple = |p: &[u8]| (be32(p), be32(&p[4..]), be32(&p[8..]));
    let m = match id {
        msg::CHOKE => exact(0).map(|_| Message::Choke)?,
        msg::UNCHOKE => exact(0).map(|_| Message::Unchoke)?,
        msg::INTERESTED => exact(0).map(|_| Message::Interested)?,
        msg::NOT_INTERESTED => exact(0).map(|_| Message::NotInterested)?,
        msg::HAVE => exact(4).map(|_| Message::Have(be32(p)))?,
        msg::BITFIELD => Message::Bitfield(p),
        msg::REQUEST => {
            exact(12)?;
            let (piece, start, length) = triple(p);
            Message::Request { piece, start, length }
        }
        msg::PIECE => {
            if p.len() < 8 {
                return Err(bad());
            }
            Message::Piece { piece: be32(p), start: be32(&p[4..]), data: &p[8..] }
        }
        msg::CANCEL => {
            exact(12)?;
            let (piece, start, length) = triple(p);
            Message::Cancel { piece, start, length }
        }
        msg::PORT => exact(2).map(|_| Message::Port(u16::from_be_bytes([p[0], p[1]])))?,
        msg::SUGGEST => exact(4).map(|_| Message::Suggest(be32(p)))?,
        msg::HAVE_ALL => exact(0).map(|_| Message::HaveAll)?,
        msg::HAVE_NONE => exact(0).map(|_| Message::HaveNone)?,
        msg::REJECT => {
            exact(12)?;
            let (piece, start, length) = triple(p);
            Message::Reject { piece, start, length }
        }
        msg::ALLOWED_FAST => exact(4).map(|_| Message::AllowedFast(be32(p)))?,
        msg::EXTENDED => {
            if p.is_empty() {
                return Err(bad());
            }
            Message::Extended { id: p[0], payload: &p[1..] }
        }
        id => Message::Unknown { id },
    };
    Ok(Some((m, total)))
}

fn header(out: &mut Vec<u8>, len: u32, id: u8) {
    out.extend_from_slice(&len.to_be_bytes());
    out.push(id);
}

pub fn encode_simple(out: &mut Vec<u8>, id: u8) {
    header(out, 1, id);
}

pub fn encode_keepalive(out: &mut Vec<u8>) {
    out.extend_from_slice(&[0, 0, 0, 0]);
}

pub fn encode_u32(out: &mut Vec<u8>, id: u8, v: u32) {
    header(out, 5, id);
    out.extend_from_slice(&v.to_be_bytes());
}

/// `request`, `cancel` and `reject`.
pub fn encode_block_msg(out: &mut Vec<u8>, id: u8, piece: u32, start: u32, length: u32) {
    header(out, 13, id);
    out.extend_from_slice(&piece.to_be_bytes());
    out.extend_from_slice(&start.to_be_bytes());
    out.extend_from_slice(&length.to_be_bytes());
}

pub fn encode_bitfield(out: &mut Vec<u8>, bits: &[u8]) {
    header(out, 1 + bits.len() as u32, msg::BITFIELD);
    out.extend_from_slice(bits);
}

pub fn encode_piece(out: &mut Vec<u8>, piece: u32, start: u32, data: &[u8]) {
    header(out, 9 + data.len() as u32, msg::PIECE);
    out.extend_from_slice(&piece.to_be_bytes());
    out.extend_from_slice(&start.to_be_bytes());
    out.extend_from_slice(data);
}

pub fn encode_extended(out: &mut Vec<u8>, ext_id: u8, payload: &[u8]) {
    header(out, 2 + payload.len() as u32, msg::EXTENDED);
    out.push(ext_id);
    out.extend_from_slice(payload);
}

/// What we learn from a BEP 10 extension handshake.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtHandshake {
    /// Extension name → the id the peer wants us to use for it.
    pub messages: Vec<(String, u8)>,
    /// Client name and version (`v`).
    pub client: Option<String>,
    /// The peer's listen port (`p`).
    pub listen_port: Option<u16>,
    /// Outstanding requests the peer accepts (`reqq`).
    pub reqq: Option<u32>,
    /// Size of the info dictionary, for ut_metadata (`metadata_size`).
    pub metadata_size: Option<u64>,
    /// BEP 21: the peer only uploads.
    pub upload_only: bool,
    /// Our address as the peer sees it (`yourip`).
    pub your_ip: Option<std::net::IpAddr>,
}

impl ExtHandshake {
    #[allow(dead_code)] // for ut_metadata / ut_pex (Phase 4)
    pub fn message_id(&self, name: &str) -> Option<u8> {
        self.messages.iter().find(|(n, _)| n == name).map(|&(_, id)| id).filter(|&id| id != 0)
    }
}

/// libtorrent `bt_peer_connection::write_extensions`.
pub fn encode_ext_handshake(
    messages: &[(&str, u8)],
    client: &str,
    listen_port: Option<u16>,
    reqq: u32,
    upload_only: bool,
    your_ip: Option<std::net::IpAddr>,
) -> Vec<u8> {
    let mut d = Entry::new_dict();
    let mut m = Entry::new_dict();
    for &(name, id) in messages {
        *m.key(name) = Entry::Int(id as i64);
    }
    *d.key("m") = m;
    *d.key("v") = Entry::Bytes(client.as_bytes().to_vec());
    *d.key("reqq") = Entry::Int(reqq as i64);
    if let Some(p) = listen_port {
        *d.key("p") = Entry::Int(p as i64);
    }
    if upload_only {
        *d.key("upload_only") = Entry::Int(1);
    }
    match your_ip {
        Some(std::net::IpAddr::V4(ip)) => *d.key("yourip") = Entry::Bytes(ip.octets().to_vec()),
        Some(std::net::IpAddr::V6(ip)) => *d.key("yourip") = Entry::Bytes(ip.octets().to_vec()),
        None => {}
    }
    d.bencode()
}

/// libtorrent `bt_peer_connection::on_extended_handshake`. Returns `None` for a payload that is
/// not a bencoded dictionary.
pub fn decode_ext_handshake(payload: &[u8]) -> Option<ExtHandshake> {
    let doc = bdecode(payload).ok()?;
    let root = doc.root()?;
    if !root.is_dict() {
        return None;
    }
    let mut h = ExtHandshake::default();
    if let Some(m) = root.dict_find_dict("m") {
        for (k, v) in m.dict_iter() {
            if let (Ok(name), Some(id)) = (std::str::from_utf8(k), v.try_int_value()) {
                if (0..=255).contains(&id) {
                    h.messages.push((name.to_owned(), id as u8));
                }
            }
        }
    }
    h.client = root.dict_find_string_value("v").map(|v| String::from_utf8_lossy(v).into_owned());
    h.listen_port = root.dict_find_int_value("p").filter(|&p| (1..=65535).contains(&p)).map(|p| p as u16);
    h.reqq = root.dict_find_int_value("reqq").filter(|&q| q > 0).map(|q| q.min(u32::MAX as i64) as u32);
    h.metadata_size = root.dict_find_int_value("metadata_size").filter(|&s| s > 0).map(|s| s as u64);
    h.upload_only = root.dict_find_int_value("upload_only").is_some_and(|v| v != 0);
    h.your_ip = root.dict_find_string_value("yourip").and_then(|b| match b.len() {
        4 => Some(std::net::IpAddr::from(<[u8; 4]>::try_from(b).ok()?)),
        16 => Some(std::net::IpAddr::from(<[u8; 16]>::try_from(b).ok()?)),
        _ => None,
    });
    Some(h)
}

/// libtorrent `generate_peer_id`: the Azureus-style prefix, then random printable characters.
pub fn generate_peer_id(prefix: &str) -> [u8; 20] {
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-_.!~*()";
    let mut id = [0u8; 20];
    let p = prefix.as_bytes();
    let n = p.len().min(20);
    id[..n].copy_from_slice(&p[..n]);
    for b in &mut id[n..] {
        *b = CHARS[crate::util::random::random_below(CHARS.len() as u64) as usize];
    }
    id
}

/// A readable client name from an Azureus-style peer id (a small part of libtorrent
/// `identify_client`). Unknown ids come back as the raw prefix.
pub fn identify_client(peer_id: &[u8; 20]) -> String {
    const NAMES: &[(&[u8; 2], &str)] = &[
        (b"AZ", "Azureus"),
        (b"BC", "BitComet"),
        (b"BI", "BiglyBT"),
        (b"DE", "Deluge"),
        (b"KT", "KTorrent"),
        (b"LR", "LibreTorrent"),
        (b"LT", "libtorrent"),
        (b"lt", "libTorrent"),
        (b"PA", "PaTorrent"),
        (b"qB", "qBittorrent"),
        (b"TR", "Transmission"),
        (b"UT", "\u{b5}Torrent"),
        (b"UM", "\u{b5}Torrent Mac"),
        (b"UW", "\u{b5}Torrent Web"),
        (b"BT", "BitTorrent"),
        (b"FD", "Free Download Manager"),
        (b"PI", "PicoTorrent"),
        (b"RT", "rTorrent"),
        (b"TX", "Tixati"),
        (b"WW", "WebTorrent"),
    ];
    if peer_id[0] == b'-' && peer_id[7] == b'-' && peer_id[3..7].iter().all(u8::is_ascii_alphanumeric) {
        let code: [u8; 2] = [peer_id[1], peer_id[2]];
        let ver = |c: u8| match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'A'..=b'Z' => (c - b'A') as u32 + 10,
            b'a'..=b'z' => (c - b'a') as u32 + 10,
            _ => 0,
        };
        let version = format!("{}.{}.{}", ver(peer_id[3]), ver(peer_id[4]), ver(peer_id[5]));
        if let Some((_, name)) = NAMES.iter().find(|(c, _)| **c == code) {
            return format!("{name} {version}");
        }
        return format!("{} {version}", String::from_utf8_lossy(&code));
    }
    if peer_id[0] == b'M' && peer_id[2] == b'-' {
        return "Mainline".into();
    }
    String::from_utf8_lossy(&peer_id[..8]).chars().filter(|c| !c.is_control()).collect()
}

/// BEP 6 allowed-fast set (libtorrent `generate_allowed_fast`), for IPv4 peers.
pub fn allowed_fast_set(ip: std::net::Ipv4Addr, info_hash: &Sha1Hash, num_pieces: u32, k: usize) -> Vec<u32> {
    let mut out = Vec::new();
    if num_pieces == 0 {
        return out;
    }
    let mut x = Vec::with_capacity(24);
    x.extend_from_slice(&(u32::from(ip) & 0xffff_ff00).to_be_bytes());
    x.extend_from_slice(&info_hash.0);
    let k = k.min(num_pieces as usize);
    while out.len() < k {
        let h = crate::crypto::Sha1::digest(&x).0;
        for chunk in h.chunks(4) {
            if out.len() >= k {
                break;
            }
            let index = be32(chunk) % num_pieces;
            if !out.contains(&index) {
                out.push(index);
            }
        }
        x = h.to_vec();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_round_trip() {
        let ih = Sha1Hash([7; 20]);
        let mut out = Vec::new();
        encode_handshake(&mut out, &ih, &[b'x'; 20]);
        assert_eq!(out.len(), HANDSHAKE_LEN);
        assert_eq!(decode_handshake(&out[..30]), Err(HandshakeError::Incomplete));
        assert_eq!(decode_handshake_prefix(&out[..48]).unwrap().1, ih);
        let h = decode_handshake(&out).unwrap();
        assert_eq!(h.info_hash, ih);
        assert!(h.reserved.supports_extensions() && h.reserved.supports_fast());
        assert_eq!(decode_handshake(b"\x13BitTorrent protocoX"), Err(HandshakeError::BadProtocol));
        assert_eq!(decode_handshake(b"\x14"), Err(HandshakeError::BadProtocol));
    }

    #[test]
    fn messages_round_trip() {
        let mut out = Vec::new();
        encode_keepalive(&mut out);
        encode_simple(&mut out, msg::UNCHOKE);
        encode_u32(&mut out, msg::HAVE, 513);
        encode_bitfield(&mut out, &[0xa0]);
        encode_block_msg(&mut out, msg::REQUEST, 1, 16384, 16384);
        encode_piece(&mut out, 2, 0, b"data");
        encode_block_msg(&mut out, msg::REJECT, 3, 0, 5);
        encode_simple(&mut out, msg::HAVE_ALL);
        encode_extended(&mut out, 0, b"de");
        out.extend_from_slice(&[0, 0, 0, 1, 99]);
        let expected = [
            Message::KeepAlive,
            Message::Unchoke,
            Message::Have(513),
            Message::Bitfield(&[0xa0]),
            Message::Request { piece: 1, start: 16384, length: 16384 },
            Message::Piece { piece: 2, start: 0, data: b"data" },
            Message::Reject { piece: 3, start: 0, length: 5 },
            Message::HaveAll,
            Message::Extended { id: 0, payload: b"de" },
            Message::Unknown { id: 99 },
        ];
        let mut buf = &out[..];
        for e in expected {
            let (m, n) = decode(buf).unwrap().unwrap();
            assert_eq!(m, e);
            buf = &buf[n..];
        }
        assert!(buf.is_empty());
        assert_eq!(decode(&[0, 0, 0, 5, 4, 0]).unwrap(), None, "partial message");
        assert_eq!(decode(&[0, 0, 0, 2, 4, 0]), Err(WireError::BadLength { id: 4, len: 2 }));
        assert_eq!(decode(&[0, 0x10, 0, 0]), Err(WireError::TooLarge(0x100000)));
    }

    #[test]
    fn extension_handshake() {
        let b = encode_ext_handshake(
            &[("ut_metadata", 2), ("ut_pex", 1)],
            "PaTorrent 0.1.0",
            Some(6881),
            500,
            false,
            Some("1.2.3.4".parse().unwrap()),
        );
        let h = decode_ext_handshake(&b).unwrap();
        assert_eq!(h.message_id("ut_metadata"), Some(2));
        assert_eq!(h.message_id("lt_donthave"), None);
        assert_eq!(h.client.as_deref(), Some("PaTorrent 0.1.0"));
        assert_eq!((h.listen_port, h.reqq), (Some(6881), Some(500)));
        assert_eq!(h.your_ip, Some("1.2.3.4".parse().unwrap()));
        assert!(decode_ext_handshake(b"li1ee").is_none());
    }

    #[test]
    fn peer_ids() {
        let id = generate_peer_id("-PA0100-");
        assert_eq!(&id[..8], b"-PA0100-");
        assert!(id[8..].iter().all(|c| c.is_ascii_graphic()));
        assert_eq!(identify_client(&id), "PaTorrent 0.1.0");
        assert_eq!(identify_client(b"-qB4650-abcdefghijkl"), "qBittorrent 4.6.5");
        assert_eq!(identify_client(b"-TR4060-abcdefghijkl"), "Transmission 4.0.6");
        assert_eq!(identify_client(b"M7-4-3--abcdefghijkl"), "Mainline");
    }

    // BEP 6 reference vectors (libtorrent test_fast_extension.cpp).
    #[test]
    fn allowed_fast_matches_bep6() {
        let ih = Sha1Hash([0xaa; 20]);
        let ip: std::net::Ipv4Addr = "80.4.4.200".parse().unwrap();
        assert_eq!(allowed_fast_set(ip, &ih, 1313, 7), [1059, 431, 808, 1217, 287, 376, 1188]);
        assert_eq!(allowed_fast_set(ip, &ih, 1313, 9), [1059, 431, 808, 1217, 287, 376, 1188, 353, 508]);
    }
}
