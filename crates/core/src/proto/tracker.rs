// Ported from libtorrent-rasterbar 2.0.11: src/http_tracker_connection.cpp (announce URL,
// parse_tracker_response, scrape URL), src/udp_tracker_connection.cpp (BEP 15 packets)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: pure encode/decode functions; the request state machines live in `engine::tracker`.

//! Tracker protocols: HTTP (BEP 3, 23, 24, 48) and UDP (BEP 15).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::bencode::bdecode;
use crate::types::Sha1Hash;
use crate::util::string::escape_string;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    None,
    Completed,
    Started,
    Stopped,
}

impl Event {
    fn http_name(self) -> Option<&'static str> {
        match self {
            Event::None => None,
            Event::Completed => Some("completed"),
            Event::Started => Some("started"),
            Event::Stopped => Some("stopped"),
        }
    }

    fn udp_code(self) -> u32 {
        match self {
            Event::None => 0,
            Event::Completed => 1,
            Event::Started => 2,
            Event::Stopped => 3,
        }
    }
}

/// libtorrent `tracker_request`, the parts we send.
#[derive(Clone, Debug)]
pub struct AnnounceRequest {
    pub info_hash: Sha1Hash,
    pub peer_id: [u8; 20],
    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
    pub corrupt: u64,
    pub event: Event,
    pub key: u32,
    pub num_want: u32,
    /// The `tracker id` an HTTP tracker gave us earlier.
    pub tracker_id: String,
}

/// libtorrent `http_tracker_connection::start` (announce URL construction).
pub fn announce_url(base: &str, r: &AnnounceRequest) -> String {
    let mut url = base.to_owned();
    url.push(if base.contains('?') { '&' } else { '?' });
    url.push_str("info_hash=");
    url.push_str(&escape_string(&r.info_hash.0));
    url.push_str("&peer_id=");
    url.push_str(&escape_string(&r.peer_id));
    url.push_str(&format!(
        "&port={}&uploaded={}&downloaded={}&left={}&corrupt={}&key={:08X}",
        r.port, r.uploaded, r.downloaded, r.left, r.corrupt, r.key
    ));
    if let Some(e) = r.event.http_name() {
        url.push_str("&event=");
        url.push_str(e);
    }
    url.push_str(&format!("&numwant={}&compact=1&no_peer_id=1", r.num_want));
    if !r.tracker_id.is_empty() {
        url.push_str("&trackerid=");
        url.push_str(&escape_string(r.tracker_id.as_bytes()));
    }
    url
}

/// BEP 48 scrape convention: the last path component must start with "announce", which is
/// replaced by "scrape". Returns `None` for trackers that cannot be scraped.
pub fn scrape_url(announce: &str, info_hashes: &[Sha1Hash]) -> Option<String> {
    let (path, query) = match announce.find('?') {
        Some(q) => (&announce[..q], Some(&announce[q + 1..])),
        None => (announce, None),
    };
    let slash = path.rfind('/')?;
    let last = &path[slash + 1..];
    let rest = last.strip_prefix("announce")?;
    let mut url = format!("{}/scrape{rest}", &path[..slash]);
    let mut sep = '?';
    if let Some(q) = query {
        url.push('?');
        url.push_str(q);
        sep = '&';
    }
    for ih in info_hashes {
        url.push(sep);
        url.push_str("info_hash=");
        url.push_str(&escape_string(&ih.0));
        sep = '&';
    }
    Some(url)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnnounceResponse {
    pub peers: Vec<SocketAddr>,
    /// Seconds.
    pub interval: u32,
    pub min_interval: Option<u32>,
    pub complete: Option<u32>,
    pub incomplete: Option<u32>,
    pub downloaded: Option<u32>,
    pub warning: Option<String>,
    pub tracker_id: Option<String>,
    pub external_ip: Option<IpAddr>,
}

/// libtorrent's defaults when a tracker omits the interval.
pub const DEFAULT_INTERVAL: u32 = 1800;

/// Parses compact peers: 6 bytes each (IPv4) or 18 bytes each (IPv6).
pub fn parse_compact_peers(b: &[u8], v6: bool, out: &mut Vec<SocketAddr>) {
    let size = if v6 { 18 } else { 6 };
    for c in b.chunks_exact(size) {
        let ip: IpAddr = if v6 {
            Ipv6Addr::from(<[u8; 16]>::try_from(&c[..16]).expect("16 bytes")).into()
        } else {
            Ipv4Addr::new(c[0], c[1], c[2], c[3]).into()
        };
        let port = u16::from_be_bytes([c[size - 2], c[size - 1]]);
        out.push(SocketAddr::new(ip, port));
    }
}

fn int_u32(v: Option<i64>) -> Option<u32> {
    v.filter(|&x| x >= 0).map(|x| x.min(u32::MAX as i64) as u32)
}

/// libtorrent `parse_tracker_response` for announces. A `failure reason` is returned as `Err`.
pub fn parse_announce_response(body: &[u8]) -> Result<AnnounceResponse, String> {
    let doc = bdecode(body).map_err(|e| format!("invalid tracker response: {}", e.kind.message()))?;
    let root = doc.root().filter(|r| r.is_dict()).ok_or("invalid tracker response")?;
    if let Some(f) = root.dict_find_string_value("failure reason") {
        return Err(String::from_utf8_lossy(f).into_owned());
    }
    let mut r = AnnounceResponse {
        interval: int_u32(root.dict_find_int_value("interval")).unwrap_or(DEFAULT_INTERVAL),
        min_interval: int_u32(root.dict_find_int_value("min interval")),
        complete: int_u32(root.dict_find_int_value("complete")),
        incomplete: int_u32(root.dict_find_int_value("incomplete")),
        downloaded: int_u32(root.dict_find_int_value("downloaded")),
        warning: root
            .dict_find_string_value("warning message")
            .map(|w| String::from_utf8_lossy(w).into_owned()),
        tracker_id: root
            .dict_find_string_value("tracker id")
            .map(|w| String::from_utf8_lossy(w).into_owned()),
        ..Default::default()
    };
    if let Some(p) = root.dict_find("peers") {
        if p.is_string() {
            parse_compact_peers(p.string_value(), false, &mut r.peers);
        } else if p.is_list() {
            for e in p.list_iter() {
                let ip = e.dict_find_str("ip").and_then(|s| s.parse::<IpAddr>().ok());
                let port = e.dict_find_int_value("port").filter(|&p| (1..=65535).contains(&p));
                if let (Some(ip), Some(port)) = (ip, port) {
                    r.peers.push(SocketAddr::new(ip, port as u16));
                }
            }
        }
    }
    if let Some(p6) = root.dict_find_string_value("peers6") {
        parse_compact_peers(p6, true, &mut r.peers);
    }
    r.external_ip = root.dict_find_string_value("external ip").and_then(|b| match b.len() {
        4 => Some(IpAddr::from(<[u8; 4]>::try_from(b).ok()?)),
        16 => Some(IpAddr::from(<[u8; 16]>::try_from(b).ok()?)),
        _ => None,
    });
    Ok(r)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrapeEntry {
    pub complete: u32,
    pub incomplete: u32,
    pub downloaded: u32,
}

/// Parses an HTTP scrape response and returns the entry for `info_hash`.
pub fn parse_scrape_response(body: &[u8], info_hash: &Sha1Hash) -> Result<ScrapeEntry, String> {
    let doc = bdecode(body).map_err(|e| format!("invalid scrape response: {}", e.kind.message()))?;
    let root = doc.root().filter(|r| r.is_dict()).ok_or("invalid scrape response")?;
    if let Some(f) = root.dict_find_string_value("failure reason") {
        return Err(String::from_utf8_lossy(f).into_owned());
    }
    let files = root.dict_find_dict("files").ok_or("invalid scrape response")?;
    let e = files.dict_find_dict(info_hash.0).ok_or("scrape response does not contain the torrent")?;
    Ok(ScrapeEntry {
        complete: int_u32(e.dict_find_int_value("complete")).unwrap_or(0),
        incomplete: int_u32(e.dict_find_int_value("incomplete")).unwrap_or(0),
        downloaded: int_u32(e.dict_find_int_value("downloaded")).unwrap_or(0),
    })
}

// --- UDP (BEP 15) ---------------------------------------------------------------------------

pub const UDP_PROTOCOL_ID: u64 = 0x41727101980;
pub const ACTION_CONNECT: u32 = 0;
pub const ACTION_ANNOUNCE: u32 = 1;
pub const ACTION_SCRAPE: u32 = 2;
pub const ACTION_ERROR: u32 = 3;
/// libtorrent scrapes at most this many info-hashes per UDP packet.
pub const MAX_SCRAPE_BATCH: usize = 74;

pub fn udp_connect(txn: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(16);
    b.extend_from_slice(&UDP_PROTOCOL_ID.to_be_bytes());
    b.extend_from_slice(&ACTION_CONNECT.to_be_bytes());
    b.extend_from_slice(&txn.to_be_bytes());
    b
}

pub fn udp_announce(conn_id: u64, txn: u32, r: &AnnounceRequest) -> Vec<u8> {
    let mut b = Vec::with_capacity(98);
    b.extend_from_slice(&conn_id.to_be_bytes());
    b.extend_from_slice(&ACTION_ANNOUNCE.to_be_bytes());
    b.extend_from_slice(&txn.to_be_bytes());
    b.extend_from_slice(&r.info_hash.0);
    b.extend_from_slice(&r.peer_id);
    b.extend_from_slice(&r.downloaded.to_be_bytes());
    b.extend_from_slice(&r.left.to_be_bytes());
    b.extend_from_slice(&r.uploaded.to_be_bytes());
    b.extend_from_slice(&r.event.udp_code().to_be_bytes());
    b.extend_from_slice(&0u32.to_be_bytes()); // IP: let the tracker use the source address
    b.extend_from_slice(&r.key.to_be_bytes());
    b.extend_from_slice(&(r.num_want as i32).to_be_bytes());
    b.extend_from_slice(&r.port.to_be_bytes());
    b
}

pub fn udp_scrape(conn_id: u64, txn: u32, info_hashes: &[Sha1Hash]) -> Vec<u8> {
    let mut b = Vec::with_capacity(16 + 20 * info_hashes.len());
    b.extend_from_slice(&conn_id.to_be_bytes());
    b.extend_from_slice(&ACTION_SCRAPE.to_be_bytes());
    b.extend_from_slice(&txn.to_be_bytes());
    for ih in info_hashes.iter().take(MAX_SCRAPE_BATCH) {
        b.extend_from_slice(&ih.0);
    }
    b
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UdpResponse {
    Connect { txn: u32, connection_id: u64 },
    Announce { txn: u32, interval: u32, leechers: u32, seeders: u32, peers: Vec<SocketAddr> },
    Scrape { txn: u32, entries: Vec<ScrapeEntry> },
    Error { txn: u32, message: String },
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().expect("4 bytes"))
}

/// Decodes a tracker reply. `v6` selects the peer format (the tracker answers over the same
/// address family we asked on).
pub fn udp_decode(buf: &[u8], v6: bool) -> Option<UdpResponse> {
    if buf.len() < 8 {
        return None;
    }
    let (action, txn) = (be32(buf), be32(&buf[4..]));
    let p = &buf[8..];
    match action {
        ACTION_CONNECT if p.len() >= 8 => {
            Some(UdpResponse::Connect { txn, connection_id: u64::from_be_bytes(p[..8].try_into().ok()?) })
        }
        ACTION_ANNOUNCE if p.len() >= 12 => {
            let mut peers = Vec::new();
            parse_compact_peers(&p[12..], v6, &mut peers);
            Some(UdpResponse::Announce {
                txn,
                interval: be32(p),
                leechers: be32(&p[4..]),
                seeders: be32(&p[8..]),
                peers,
            })
        }
        ACTION_SCRAPE => {
            let entries = p
                .chunks_exact(12)
                .map(|c| ScrapeEntry {
                    complete: be32(c),
                    downloaded: be32(&c[4..]),
                    incomplete: be32(&c[8..]),
                })
                .collect();
            Some(UdpResponse::Scrape { txn, entries })
        }
        ACTION_ERROR => Some(UdpResponse::Error { txn, message: String::from_utf8_lossy(p).into_owned() }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> AnnounceRequest {
        AnnounceRequest {
            info_hash: Sha1Hash([0x12; 20]),
            peer_id: *b"-LB0100-abcdefghijkl",
            port: 6881,
            uploaded: 1,
            downloaded: 2,
            left: 3,
            corrupt: 0,
            event: Event::Started,
            key: 0xabcdef,
            num_want: 200,
            tracker_id: String::new(),
        }
    }

    #[test]
    fn http_announce_url() {
        let u = announce_url("http://t.org/announce", &req());
        assert_eq!(
            u,
            "http://t.org/announce?info_hash=%12%12%12%12%12%12%12%12%12%12%12%12%12%12%12%12%12%12%12%12\
             &peer_id=-LB0100-abcdefghijkl&port=6881&uploaded=1&downloaded=2&left=3&corrupt=0&key=00ABCDEF\
             &event=started&numwant=200&compact=1&no_peer_id=1"
        );
        assert!(
            announce_url("http://t.org/a?passkey=x", &req())
                .starts_with("http://t.org/a?passkey=x&info_hash=")
        );
    }

    // libtorrent test_tracker.cpp: scrape URL derivation.
    #[test]
    fn scrape_urls() {
        let ih = [Sha1Hash([0xff; 20])];
        assert_eq!(scrape_url("http://x.org/announce", &[]).unwrap(), "http://x.org/scrape");
        assert_eq!(scrape_url("http://x.org/announce.php?k=1", &[]).unwrap(), "http://x.org/scrape.php?k=1");
        assert!(scrape_url("http://x.org/a", &ih).is_none());
        assert!(scrape_url("http://x.org/announce/x", &ih).is_none());
        let s = scrape_url("http://x.org/x/announce", &ih).unwrap();
        assert!(
            s.ends_with("/x/scrape?info_hash=%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff%ff"),
            "{s}"
        );
    }

    // libtorrent test_tracker.cpp: parse_peers_* / parse_failure_reason / parse_interval.
    #[test]
    fn announce_responses() {
        let r = parse_announce_response(b"d8:intervali1800e5:peers6:\x01\x02\x03\x04\x00\x506:peers618:\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01\x00\x51e").unwrap();
        assert_eq!(r.interval, 1800);
        assert_eq!(r.peers, ["1.2.3.4:80".parse().unwrap(), "[::1]:81".parse().unwrap()]);

        let r = parse_announce_response(
            b"d5:peersld7:peer id20:aaaaaaaaaaaaaaaaaaaa2:ip15:123.123.123.1234:porti1001eed2:ip3:bad4:porti6881eee8:completei5e10:incompletei7e12:min intervali60e10:tracker id3:abc15:warning message4:warn11:external ip4:\x04\x03\x02\x01e",
        )
        .unwrap();
        assert_eq!(
            r.peers,
            ["123.123.123.123:1001".parse().unwrap()],
            "entries with a bad address are skipped"
        );
        assert_eq!((r.complete, r.incomplete, r.min_interval), (Some(5), Some(7), Some(60)));
        assert_eq!(r.interval, DEFAULT_INTERVAL);
        assert_eq!(r.tracker_id.as_deref(), Some("abc"));
        assert_eq!(r.warning.as_deref(), Some("warn"));
        assert_eq!(r.external_ip, Some("4.3.2.1".parse().unwrap()));

        assert_eq!(parse_announce_response(b"d14:failure reason5:test!e"), Err("test!".into()));
        assert!(parse_announce_response(b"<html>").is_err());

        let s = b"d5:filesd20:\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12\x12d8:completei1e10:downloadedi2e10:incompletei3eeee";
        assert_eq!(
            parse_scrape_response(s, &Sha1Hash([0x12; 20])),
            Ok(ScrapeEntry { complete: 1, incomplete: 3, downloaded: 2 })
        );
        assert!(parse_scrape_response(s, &Sha1Hash([0x13; 20])).is_err());
    }

    #[test]
    fn udp_packets() {
        let c = udp_connect(7);
        assert_eq!(&c[..8], &UDP_PROTOCOL_ID.to_be_bytes());
        assert_eq!(c.len(), 16);
        let a = udp_announce(99, 8, &req());
        assert_eq!(a.len(), 98);
        assert_eq!(&a[8..12], &[0, 0, 0, 1]);
        assert_eq!(&a[80..84], &[0, 0, 0, 2], "event started = 2");
        assert_eq!(&a[96..98], &6881u16.to_be_bytes());
        assert_eq!(udp_scrape(1, 2, &[Sha1Hash::default(); 100]).len(), 16 + 20 * MAX_SCRAPE_BATCH);

        let mut reply = vec![0, 0, 0, 0, 0, 0, 0, 7];
        reply.extend_from_slice(&42u64.to_be_bytes());
        assert_eq!(udp_decode(&reply, false), Some(UdpResponse::Connect { txn: 7, connection_id: 42 }));
        let mut reply = vec![0, 0, 0, 1, 0, 0, 0, 8, 0, 0, 7, 8, 0, 0, 0, 3, 0, 0, 0, 4];
        reply.extend_from_slice(&[10, 0, 0, 1, 0x1a, 0xe1]);
        assert_eq!(
            udp_decode(&reply, false),
            Some(UdpResponse::Announce {
                txn: 8,
                interval: 1800,
                leechers: 3,
                seeders: 4,
                peers: vec!["10.0.0.1:6881".parse().unwrap()]
            })
        );
        let reply = [0, 0, 0, 3, 0, 0, 0, 9, b'n', b'o'];
        assert_eq!(udp_decode(&reply, false), Some(UdpResponse::Error { txn: 9, message: "no".into() }));
        assert_eq!(udp_decode(&[0, 0, 0, 0, 0], false), None);
    }
}
