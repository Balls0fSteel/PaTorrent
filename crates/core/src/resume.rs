// Ported from libtorrent-rasterbar 2.0.11: src/write_resume_data.cpp, src/read_resume_data.cpp;
// file-version 2 from 2.1.0 (bf6c5e546b)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a plain struct instead of `add_torrent_params`; only the fields this engine uses are
// read and written, under libtorrent's key names, so records stay readable by libtorrent tools.
// Extra `lt-` keys (queue position, total size, piece count and length) let a stopped
// torrent be listed without loading its metadata (PLAN.md D5). `comment`, `created by` and
// `creation date` are kept under the `.torrent` file's own key names, since the session store
// holds only the info dictionary and a restored torrent would lose them. Records are written
// as file-version 2 (`pieces` a packed bitfield); version 1 records (one byte per piece) still
// load. Neither version's verified-piece bits (seed mode) nor merkle trees are carried.

//! Fast-resume records.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::bencode::{Entry, Node, bdecode};
use crate::error::Error;
use crate::types::{InfoHashes, Sha1Hash, Sha256Hash};
use crate::util::bitfield::Bitfield;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ResumeData {
    pub info_hashes: InfoHashes,
    pub name: String,
    pub save_path: String,
    /// Pieces we have; `None` if unknown (the files must be checked). A file-version 2 record
    /// without `lt-num-pieces` gives whole bytes: up to 7 spare bits past the last piece.
    pub pieces: Option<Bitfield>,
    /// Partly downloaded pieces: piece index and its finished blocks (libtorrent `unfinished`).
    pub unfinished: Vec<(u32, Bitfield)>,
    pub total_uploaded: u64,
    pub total_downloaded: u64,
    pub active_time: u64,
    pub finished_time: u64,
    pub seeding_time: u64,
    pub added_time: i64,
    pub completed_time: i64,
    pub paused: bool,
    pub auto_managed: bool,
    pub sequential_download: bool,
    /// BEP 16 (libtorrent `super_seeding`).
    pub super_seeding: bool,
    /// Per-torrent switches (libtorrent `disable_dht`, `disable_lsd`, `disable_pex`).
    pub disable_dht: bool,
    pub disable_lsd: bool,
    pub disable_pex: bool,
    pub file_priority: Vec<u8>,
    /// Files stored under another name (libtorrent `mapped_files`).
    pub mapped_files: std::collections::BTreeMap<u32, String>,
    /// Tracker URLs per tier.
    pub trackers: Vec<Vec<String>>,
    /// The saved peers, compact (`peers` and `peers6`).
    pub peers: SavedPeers,
    /// BEP 19 and BEP 17 web seeds (libtorrent `url-list` / `httpseeds`): they live outside the
    /// info dictionary, so the metadata store does not keep them.
    pub url_seeds: Vec<String>,
    pub http_seeds: Vec<String>,
    pub upload_rate_limit: u32,
    pub download_rate_limit: u32,
    pub max_connections: Option<u32>,
    pub max_uploads: Option<u32>,
    pub queue_position: Option<u32>,
    pub total_size: u64,
    pub num_pieces: u32,
    pub piece_length: u32,
    /// The `.torrent` file's text outside the info dictionary (`comment`, `created by`,
    /// `creation date`).
    pub comment: String,
    pub created_by: String,
    pub creation_date: Option<i64>,
    /// The bencoded info dictionary, when embedded (libtorrent's `info` key).
    pub info: Option<Vec<u8>>,
}

impl ResumeData {
    pub fn to_entry(&self) -> Entry {
        let mut e = Entry::new_dict();
        let int = |v: i64| Entry::Int(v);
        let s = |v: &str| Entry::Bytes(v.as_bytes().to_vec());
        *e.key("file-format") = s("libtorrent resume file");
        *e.key("file-version") = int(2);
        *e.key("libtorrent-version") = s(crate::VERSION);
        if let Some(v1) = self.info_hashes.v1 {
            *e.key("info-hash") = Entry::Bytes(v1.0.to_vec());
        }
        if let Some(v2) = self.info_hashes.v2 {
            *e.key("info-hash2") = Entry::Bytes(v2.0.to_vec());
        }
        *e.key("name") = s(&self.name);
        *e.key("save_path") = s(&self.save_path);
        // libtorrent 2.1: the have bitfield's bytes, MSB first, instead of a byte per piece.
        if let Some(p) = self.pieces.as_ref().filter(|p| !p.is_empty()) {
            *e.key("pieces") = Entry::Bytes(p.as_bytes().to_vec());
        }
        if !self.unfinished.is_empty() {
            // libtorrent's layout: the bitmask is the block bitfield's bytes, MSB first.
            *e.key("unfinished") = Entry::List(
                self.unfinished
                    .iter()
                    .map(|(piece, blocks)| {
                        let mut d = Entry::new_dict();
                        *d.key("piece") = int(*piece as i64);
                        *d.key("bitmask") = Entry::Bytes(blocks.as_bytes().to_vec());
                        d
                    })
                    .collect(),
            );
        }
        *e.key("total_uploaded") = int(self.total_uploaded as i64);
        *e.key("total_downloaded") = int(self.total_downloaded as i64);
        *e.key("active_time") = int(self.active_time as i64);
        *e.key("finished_time") = int(self.finished_time as i64);
        *e.key("seeding_time") = int(self.seeding_time as i64);
        *e.key("added_time") = int(self.added_time);
        *e.key("completed_time") = int(self.completed_time);
        *e.key("paused") = int(self.paused as i64);
        *e.key("auto_managed") = int(self.auto_managed as i64);
        *e.key("sequential_download") = int(self.sequential_download as i64);
        *e.key("super_seeding") = int(self.super_seeding as i64);
        *e.key("disable_dht") = int(self.disable_dht as i64);
        *e.key("disable_lsd") = int(self.disable_lsd as i64);
        *e.key("disable_pex") = int(self.disable_pex as i64);
        if !self.file_priority.is_empty() {
            *e.key("file_priority") =
                Entry::List(self.file_priority.iter().map(|&p| int(p as i64)).collect());
        }
        if let Some(&last) = self.mapped_files.keys().next_back() {
            // libtorrent `write_resume_data`: one string per file up to the last renamed one,
            // empty for files keeping their name.
            let list = (0..=last).map(|i| s(self.mapped_files.get(&i).map_or("", String::as_str))).collect();
            *e.key("mapped_files") = Entry::List(list);
        }
        *e.key("trackers") = Entry::List(
            self.trackers.iter().map(|tier| Entry::List(tier.iter().map(|u| s(u)).collect())).collect(),
        );
        if !self.peers.v4().is_empty() {
            *e.key("peers") = Entry::Bytes(self.peers.v4().to_vec());
        }
        if !self.peers.v6().is_empty() {
            *e.key("peers6") = Entry::Bytes(self.peers.v6().to_vec());
        }
        *e.key("url-list") = Entry::List(self.url_seeds.iter().map(|u| s(u)).collect());
        *e.key("httpseeds") = Entry::List(self.http_seeds.iter().map(|u| s(u)).collect());
        *e.key("upload_rate_limit") = int(self.upload_rate_limit as i64);
        *e.key("download_rate_limit") = int(self.download_rate_limit as i64);
        *e.key("max_connections") = int(self.max_connections.map_or(-1, |v| v as i64));
        *e.key("max_uploads") = int(self.max_uploads.map_or(-1, |v| v as i64));
        if let Some(q) = self.queue_position {
            *e.key("lt-queue-position") = int(q as i64);
        }
        *e.key("lt-total-size") = int(self.total_size as i64);
        *e.key("lt-num-pieces") = int(self.num_pieces as i64);
        *e.key("lt-piece-length") = int(self.piece_length as i64);
        if !self.comment.is_empty() {
            *e.key("comment") = s(&self.comment);
        }
        if !self.created_by.is_empty() {
            *e.key("created by") = s(&self.created_by);
        }
        if let Some(d) = self.creation_date {
            *e.key("creation date") = int(d);
        }
        if let Some(info) = &self.info {
            // Verbatim, so the info-hash of the embedded dictionary is preserved exactly.
            *e.key("info") = Entry::Preformatted(info.clone());
        }
        e
    }

    pub fn encode(&self) -> Vec<u8> {
        self.to_entry().bencode()
    }

    pub fn decode(buf: &[u8]) -> Result<ResumeData, Error> {
        let doc = bdecode(buf).map_err(|e| Error::InvalidResumeData(e.kind.message().into()))?;
        let root = doc.root().ok_or_else(|| Error::InvalidResumeData("empty".into()))?;
        ResumeData::from_node(root)
    }

    /// libtorrent `read_resume_data`.
    pub fn from_node(root: Node<'_>) -> Result<ResumeData, Error> {
        let bad = |m: &str| Error::InvalidResumeData(m.into());
        if !root.is_dict() {
            return Err(bad("not a dictionary"));
        }
        if root.dict_find_str("file-format") != Some("libtorrent resume file") {
            return Err(bad("invalid file format tag"));
        }
        let version = root.dict_find_int_value("file-version").unwrap_or(1);
        if version != 1 && version != 2 {
            return Err(bad(&format!("unsupported file-version {version}")));
        }
        let int = |k: &str| root.dict_find_int_value(k).unwrap_or(0);
        let uint = |k: &str| root.dict_find_int_value(k).filter(|&v| v >= 0).unwrap_or(0) as u64;
        let opt =
            |k: &str| root.dict_find_int_value(k).filter(|&v| v >= 0).map(|v| v.min(u32::MAX as i64) as u32);
        let mut r = ResumeData {
            info_hashes: InfoHashes {
                v1: root
                    .dict_find_string_value("info-hash")
                    .and_then(|b| (b.len() == 20).then(|| Sha1Hash::from_slice(b)).flatten()),
                v2: root
                    .dict_find_string_value("info-hash2")
                    .and_then(|b| (b.len() == 32).then(|| Sha256Hash::from_slice(b)).flatten()),
            },
            name: root.dict_find_str("name").unwrap_or_default().to_owned(),
            save_path: root.dict_find_str("save_path").unwrap_or_default().to_owned(),
            total_uploaded: uint("total_uploaded"),
            total_downloaded: uint("total_downloaded"),
            active_time: uint("active_time"),
            finished_time: uint("finished_time"),
            seeding_time: uint("seeding_time"),
            added_time: int("added_time"),
            completed_time: int("completed_time"),
            paused: int("paused") != 0,
            auto_managed: int("auto_managed") != 0,
            sequential_download: int("sequential_download") != 0,
            super_seeding: int("super_seeding") != 0,
            disable_dht: int("disable_dht") != 0,
            disable_lsd: int("disable_lsd") != 0,
            disable_pex: int("disable_pex") != 0,
            upload_rate_limit: opt("upload_rate_limit").unwrap_or(0),
            download_rate_limit: opt("download_rate_limit").unwrap_or(0),
            max_connections: opt("max_connections"),
            max_uploads: opt("max_uploads"),
            queue_position: opt("lt-queue-position"),
            total_size: uint("lt-total-size"),
            num_pieces: opt("lt-num-pieces").unwrap_or(0),
            piece_length: opt("lt-piece-length").unwrap_or(0),
            comment: root.dict_find_str("comment").unwrap_or_default().to_owned(),
            created_by: root.dict_find_str("created by").unwrap_or_default().to_owned(),
            creation_date: root.dict_find_int_value("creation date"),
            ..Default::default()
        };
        if let Some(p) = root.dict_find_string_value("pieces") {
            let bf = if version == 1 {
                // A byte per piece: bit 0 have, bit 1 verified (not carried).
                let mut bf = Bitfield::new(p.len(), false);
                for (i, &b) in p.iter().enumerate() {
                    bf.set(i, b & 1 != 0);
                }
                bf
            } else {
                let mut bf = Bitfield::from_bytes(p, p.len() * 8).expect("length matches");
                // Our own records know the piece count: drop the byte's spare bits.
                let n = r.num_pieces as usize;
                if n > 0 && n <= bf.len() && bf.len() - n < 8 {
                    bf.resize(n, false);
                }
                bf
            };
            r.pieces = Some(bf);
        }
        if let Some(l) = root.dict_find_list("unfinished") {
            for e in l.list_iter().filter(|e| e.is_dict()) {
                let Some(piece) =
                    e.dict_find_int_value("piece").filter(|&p| (0..=u32::MAX as i64).contains(&p))
                else {
                    continue;
                };
                let Some(mask) = e.dict_find_string_value("bitmask").filter(|m| !m.is_empty()) else {
                    continue;
                };
                let blocks = Bitfield::from_bytes(mask, mask.len() * 8).expect("length matches");
                r.unfinished.push((piece as u32, blocks));
            }
        }
        if let Some(l) = root.dict_find_list("file_priority") {
            r.file_priority =
                l.list_iter().map(|n| n.try_int_value().unwrap_or(4).clamp(0, 7) as u8).collect();
        }
        if let Some(l) = root.dict_find_list("mapped_files") {
            for (i, n) in l.list_iter().enumerate() {
                if let Some(name) = n.str_value().filter(|n| !n.is_empty()) {
                    r.mapped_files.insert(i as u32, name.to_owned());
                }
            }
        }
        if let Some(l) = root.dict_find_list("trackers") {
            for tier in l.list_iter().filter(|t| t.is_list()) {
                let urls: Vec<String> =
                    tier.list_iter().filter_map(|u| u.str_value().map(str::to_owned)).collect();
                if !urls.is_empty() {
                    r.trackers.push(urls);
                }
            }
        }
        let strings = |k: &str| -> Vec<String> {
            root.dict_find_list(k)
                .map(|l| l.list_iter().filter_map(|u| u.str_value().map(str::to_owned)).collect())
                .unwrap_or_default()
        };
        r.url_seeds = strings("url-list");
        r.http_seeds = strings("httpseeds");
        r.peers = SavedPeers::from_compact(
            root.dict_find_string_value("peers").unwrap_or_default(),
            root.dict_find_string_value("peers6").unwrap_or_default(),
        );
        if let Some(info) = root.dict_find_dict("info") {
            r.info = Some(info.data_section().to_vec());
        }
        if r.info_hashes.v1.is_none() && r.info_hashes.v2.is_none() {
            return Err(bad("missing info-hash"));
        }
        Ok(r)
    }
}

/// Peers saved in resume data, in the compact form they are saved in (libtorrent's `peers`
/// and `peers6`: 6 bytes per IPv4 peer, 18 per IPv6 one): a stopped torrent keeps them so
/// until it starts (`PeerList::Saved`). Unusable addresses (port 0, unspecified, multicast)
/// and repeats are dropped, as adding them to a peer list would.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub(crate) struct SavedPeers {
    /// The IPv4 entries' byte length (a `u32` at the front would cost as much and read worse),
    /// then the IPv4 entries, then the IPv6 ones.
    bytes: Box<[u8]>,
}

const V4_LEN: usize = 6;
const V6_LEN: usize = 18;

impl SavedPeers {
    /// Whether a peer list takes the address (libtorrent `peer_list::add_peer` refuses the
    /// others).
    pub fn usable(addr: &SocketAddr) -> bool {
        !(addr.port() == 0 || addr.ip().is_unspecified() || addr.ip().is_multicast())
    }

    pub fn from_addrs(addrs: impl IntoIterator<Item = SocketAddr>) -> SavedPeers {
        let (mut v4, mut v6) = (Vec::new(), Vec::new());
        for a in addrs {
            match a {
                SocketAddr::V4(a) => {
                    v4.extend_from_slice(&a.ip().octets());
                    v4.extend_from_slice(&a.port().to_be_bytes());
                }
                SocketAddr::V6(a) => {
                    v6.extend_from_slice(&a.ip().octets());
                    v6.extend_from_slice(&a.port().to_be_bytes());
                }
            }
        }
        SavedPeers::from_compact(&v4, &v6)
    }

    /// From libtorrent's `peers` and `peers6` strings (a trailing partial entry is ignored).
    pub fn from_compact(v4: &[u8], v6: &[u8]) -> SavedPeers {
        let mut seen = std::collections::HashSet::new();
        let mut keep = |c: &[u8]| {
            let a = decode_entry(c);
            SavedPeers::usable(&a) && seen.insert(a)
        };
        let mut out = Vec::with_capacity(v4.len() + v6.len());
        for c in v4.chunks_exact(V4_LEN) {
            if keep(c) {
                out.extend_from_slice(c);
            }
        }
        let n4 = out.len();
        for c in v6.chunks_exact(V6_LEN) {
            if keep(c) {
                out.extend_from_slice(c);
            }
        }
        SavedPeers::from_parts(n4, out)
    }

    fn from_parts(n4: usize, entries: Vec<u8>) -> SavedPeers {
        if entries.is_empty() {
            return SavedPeers::default();
        }
        let mut bytes = Vec::with_capacity(4 + entries.len());
        bytes.extend_from_slice(&(n4 as u32).to_le_bytes());
        bytes.extend_from_slice(&entries);
        SavedPeers { bytes: bytes.into_boxed_slice() }
    }

    fn n4(&self) -> usize {
        match self.bytes.get(..4) {
            Some(b) => u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize,
            None => 0,
        }
    }

    /// The IPv4 entries, as libtorrent's `peers`.
    pub fn v4(&self) -> &[u8] {
        self.bytes.get(4..4 + self.n4()).unwrap_or_default()
    }

    /// The IPv6 entries, as libtorrent's `peers6`.
    pub fn v6(&self) -> &[u8] {
        self.bytes.get(4 + self.n4()..).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.v4().len() / V4_LEN + self.v6().len() / V6_LEN
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = SocketAddr> + '_ {
        self.v4().chunks_exact(V4_LEN).chain(self.v6().chunks_exact(V6_LEN)).map(decode_entry)
    }

    /// The first `n` peers.
    pub fn first(&self, n: usize) -> SavedPeers {
        if n >= self.len() {
            return self.clone();
        }
        let n4 = n.min(self.n4() / V4_LEN);
        let mut entries = self.v4()[..n4 * V4_LEN].to_vec();
        entries.extend_from_slice(&self.v6()[..(n - n4) * V6_LEN]);
        SavedPeers::from_parts(n4 * V4_LEN, entries)
    }

    /// Keeps the peers `f` says yes to; returns how many went.
    pub fn retain(&mut self, mut f: impl FnMut(SocketAddr) -> bool) -> usize {
        let before = self.len();
        let (mut entries, mut n4) = (Vec::new(), 0);
        for (i, a) in self.iter().enumerate() {
            if f(a) {
                let c = self.entry(i);
                if c.len() == V4_LEN {
                    n4 += V4_LEN;
                }
                entries.extend_from_slice(c);
            }
        }
        if entries.len() != self.bytes.len().saturating_sub(4) {
            *self = SavedPeers::from_parts(n4, entries);
        }
        before - self.len()
    }

    fn entry(&self, i: usize) -> &[u8] {
        let k4 = self.n4() / V4_LEN;
        if i < k4 { &self.v4()[i * V4_LEN..][..V4_LEN] } else { &self.v6()[(i - k4) * V6_LEN..][..V6_LEN] }
    }
}

/// One compact entry: 4 address bytes and the port, or 16 and the port (network order).
fn decode_entry(c: &[u8]) -> SocketAddr {
    let port = u16::from_be_bytes([c[c.len() - 2], c[c.len() - 1]]);
    if c.len() == V4_LEN {
        SocketAddr::new(Ipv4Addr::new(c[0], c[1], c[2], c[3]).into(), port)
    } else {
        SocketAddr::new(Ipv6Addr::from(<[u8; 16]>::try_from(&c[..16]).expect("16 bytes")).into(), port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_peers_stay_compact() {
        let v4 = [
            10, 0, 0, 1, 0, 1, 10, 0, 0, 2, 0, 2, 0, 0, 0, 0, 0, 3, 10, 0, 0, 1, 0, 1, 224, 0, 0, 1, 0, 4, 9,
        ];
        let mut v6 = vec![0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 5];
        v6.extend_from_slice(&[0; 18]);
        let s = SavedPeers::from_compact(&v4, &v6);
        // repeats, unspecified, multicast, port 0 and the partial entry are dropped
        let addrs: Vec<String> = s.iter().map(|a| a.to_string()).collect();
        assert_eq!(addrs, ["10.0.0.1:1", "10.0.0.2:2", "[2001:db8::1]:5"]);
        assert_eq!((s.len(), s.v4(), s.v6()), (3, &v4[..12], &v6[..18]));
        assert_eq!(SavedPeers::from_addrs(s.iter()), s);
        assert_eq!(s.first(1).iter().count(), 1);
        assert_eq!(s.first(2), SavedPeers::from_compact(&v4[..12], &[]));
        assert_eq!(s.first(3), s);
        assert!(s.first(0).is_empty() && SavedPeers::default().v6().is_empty());
        let mut t = s.clone();
        assert_eq!(t.retain(|a| a.port() != 2), 1);
        let ports: Vec<u16> = t.iter().map(|a| a.port()).collect();
        assert_eq!(ports, [1, 5]);
        assert_eq!(t.retain(|_| true), 0);
        assert_eq!(t.retain(|_| false), 2);
        assert!(t.is_empty() && t.iter().next().is_none());
    }

    #[test]
    fn round_trip() {
        let mut pieces = Bitfield::new(10, false);
        pieces.set(3, true);
        pieces.set(9, true);
        let r = ResumeData {
            info_hashes: InfoHashes::v1(Sha1Hash([5; 20])),
            name: "name".into(),
            save_path: "C:/dl".into(),
            pieces: Some(pieces),
            unfinished: vec![(4, Bitfield::from_bytes(&[0b1010_0000, 0x01], 16).unwrap())],
            total_uploaded: 10,
            total_downloaded: 20,
            active_time: 30,
            added_time: 1_700_000_000,
            paused: true,
            auto_managed: true,
            super_seeding: true,
            disable_dht: true,
            disable_pex: true,
            file_priority: vec![4, 0, 7],
            mapped_files: [(1, "x/renamed.bin".to_owned()), (3, "/abs/y".to_owned())].into(),
            trackers: vec![vec!["http://a/announce".into()], vec!["udp://b:1".into(), "udp://c:2".into()]],
            peers: SavedPeers::from_addrs(["1.2.3.4:5".parse().unwrap(), "[::1]:6".parse().unwrap()]),
            url_seeds: vec!["http://w/a/".into(), "https://w/b".into()],
            http_seeds: vec!["http://h/seed".into()],
            upload_rate_limit: 1000,
            max_connections: Some(50),
            queue_position: Some(3),
            total_size: 12345,
            num_pieces: 10,
            piece_length: 16384,
            comment: "a comment".into(),
            created_by: "mktorrent".into(),
            creation_date: Some(951_782_400),
            info: Some(b"d4:name1:xe".to_vec()),
            ..Default::default()
        };
        let bytes = r.encode();
        assert_eq!(ResumeData::decode(&bytes).unwrap(), r);
        let doc = bdecode(&bytes[..]).unwrap();
        let root = doc.root().unwrap();
        assert_eq!(root.dict_find_str("created by"), Some("mktorrent"));
        assert_eq!(root.dict_find_int_value("creation date"), Some(951_782_400));
        assert_eq!(root.dict_find_int_value("file-version"), Some(2));
        assert_eq!(root.dict_find_string_value("pieces").unwrap(), &[0b0001_0000, 0b0100_0000]);
        assert_eq!(root.dict_find_int_value("max_uploads"), Some(-1));
        // libtorrent's layout: one string per file up to the last renamed one.
        let mapped = root.dict_find_list("mapped_files").unwrap();
        let names: Vec<_> = mapped.list_iter().map(|n| n.str_value().unwrap()).collect();
        assert_eq!(names, ["", "x/renamed.bin", "", "/abs/y"]);
        assert_eq!(root.dict_find_int_value("disable_dht"), Some(1));
        assert_eq!(root.dict_find_int_value("disable_lsd"), Some(0));
        let u = root.dict_find_list("unfinished").unwrap().list_at(0).unwrap();
        assert_eq!(u.dict_find_int_value("piece"), Some(4));
        assert_eq!(u.dict_find_string_value("bitmask").unwrap(), &[0b1010_0000, 0x01]);

        // Malformed `unfinished` entries are skipped, as libtorrent does.
        let mut e = r.to_entry();
        let mut bad = Entry::new_dict();
        *bad.key("piece") = Entry::Int(-1);
        *bad.key("bitmask") = Entry::Bytes(vec![0xff]);
        let mut empty = Entry::new_dict();
        *empty.key("piece") = Entry::Int(2);
        *empty.key("bitmask") = Entry::Bytes(Vec::new());
        e.key("unfinished").list_mut().extend([bad, empty, Entry::Int(3)]);
        assert_eq!(ResumeData::decode(&e.bencode()).unwrap().unfinished, r.unfinished);

        assert!(ResumeData::decode(b"d11:file-format3:fooe").is_err());
        assert!(ResumeData::decode(b"d11:file-format22:libtorrent resume filee").is_err(), "no info-hash");
    }

    /// A record as 2.0.11 and our releases before file-version 2 wrote it.
    fn v1_record(pieces: &[u8], num_pieces: Option<i64>) -> Vec<u8> {
        let mut e = Entry::new_dict();
        *e.key("file-format") = Entry::Bytes(b"libtorrent resume file".to_vec());
        *e.key("file-version") = Entry::Int(1);
        *e.key("info-hash") = Entry::Bytes(vec![7; 20]);
        *e.key("pieces") = Entry::Bytes(pieces.to_vec());
        if let Some(n) = num_pieces {
            *e.key("lt-num-pieces") = Entry::Int(n);
        }
        e.bencode()
    }

    #[test]
    fn file_versions() {
        // Version 1: a byte per piece, bit 0 have, bit 1 verified (ignored).
        let r = ResumeData::decode(&v1_record(&[1, 0, 3, 2, 1], Some(5))).unwrap();
        let have: Vec<bool> = r.pieces.as_ref().unwrap().iter().collect();
        assert_eq!(have, [true, false, true, false, true]);
        // It saves as version 2 and loads back the same.
        let again = ResumeData::decode(&r.encode()).unwrap();
        assert_eq!(again, r);
        // No `file-version` means 1 (libtorrent's default).
        let entry = |b: Vec<u8>| Entry::from_node(bdecode(&b[..]).unwrap().root().unwrap());
        let mut e = entry(v1_record(&[1, 1], None));
        e.dict_mut().remove(b"file-version".as_slice());
        assert_eq!(ResumeData::decode(&e.bencode()).unwrap().pieces, Some(Bitfield::new(2, true)));

        // Version 2 from another client (no `lt-num-pieces`): whole bytes, MSB first; the
        // `verified` bitfield is not carried.
        let mut e = entry(v1_record(&[], None));
        *e.key("file-version") = Entry::Int(2);
        *e.key("pieces") = Entry::Bytes(vec![0b1010_0000, 0b0000_0001]);
        *e.key("verified") = Entry::Bytes(vec![0xff, 0xff]);
        let r = ResumeData::decode(&e.bencode()).unwrap();
        let p = r.pieces.unwrap();
        assert_eq!((p.len(), p.count(), p.get(0), p.get(2), p.get(15)), (16, 3, true, true, true));
        *e.key("lt-num-pieces") = Entry::Int(10);
        *e.key("pieces") = Entry::Bytes(vec![0xff, 0b1100_0000]);
        assert_eq!(ResumeData::decode(&e.bencode()).unwrap().pieces, Some(Bitfield::new(10, true)));

        // Other versions are refused.
        for v in [0, 3, -1] {
            *e.key("file-version") = Entry::Int(v);
            let err = ResumeData::decode(&e.bencode()).unwrap_err().to_string();
            assert!(err.contains(&format!("file-version {v}")), "{err}");
        }
    }

    #[test]
    fn version_2_is_smaller() {
        // A seed of 2,000 pieces, as the engine saves one.
        let r = ResumeData {
            info_hashes: InfoHashes::v1(Sha1Hash([5; 20])),
            name: "a seed".into(),
            save_path: "C:/dl".into(),
            pieces: Some(Bitfield::new(2000, true)),
            num_pieces: 2000,
            piece_length: 262_144,
            total_size: 2000 * 262_144,
            ..Default::default()
        };
        let v2 = r.encode();
        let mut e = r.to_entry();
        *e.key("file-version") = Entry::Int(1);
        *e.key("pieces") = Entry::Bytes(vec![1; 2000]);
        let v1 = e.bencode();
        assert_eq!(ResumeData::decode(&v1).unwrap(), r);
        assert_eq!(ResumeData::decode(&v2).unwrap(), r);
        eprintln!("2,000-piece seed record: version 1 {} bytes, version 2 {} bytes", v1.len(), v2.len());
        assert_eq!(v1.len() - v2.len(), 2000 - 250 + 1, "pieces: 2000:… -> 250:…");
    }
}
