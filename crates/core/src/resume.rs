// Ported from libtorrent-rasterbar 2.0.11: src/write_resume_data.cpp, src/read_resume_data.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a plain struct instead of `add_torrent_params`; only the fields this engine uses are
// read and written, under libtorrent's key names, so records stay readable by libtorrent tools.
// Extra `lt-` keys (queue position, total size, piece count and length) let a stopped
// torrent be listed without loading its metadata (PLAN.md D5).

//! Fast-resume records.

use std::net::SocketAddr;

use crate::bencode::{Entry, Node, bdecode};
use crate::error::Error;
use crate::proto::tracker::parse_compact_peers;
use crate::types::{InfoHashes, Sha1Hash, Sha256Hash};
use crate::util::bitfield::Bitfield;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ResumeData {
    pub info_hashes: InfoHashes,
    pub name: String,
    pub save_path: String,
    /// Pieces we have; `None` if unknown (the files must be checked).
    pub pieces: Option<Bitfield>,
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
    pub file_priority: Vec<u8>,
    /// Tracker URLs per tier.
    pub trackers: Vec<Vec<String>>,
    pub peers: Vec<SocketAddr>,
    pub upload_rate_limit: u32,
    pub download_rate_limit: u32,
    pub max_connections: Option<u32>,
    pub max_uploads: Option<u32>,
    pub queue_position: Option<u32>,
    pub total_size: u64,
    pub num_pieces: u32,
    pub piece_length: u32,
    /// The bencoded info dictionary, when embedded (libtorrent's `info` key).
    pub info: Option<Vec<u8>>,
}

impl ResumeData {
    pub fn to_entry(&self) -> Entry {
        let mut e = Entry::new_dict();
        let int = |v: i64| Entry::Int(v);
        let s = |v: &str| Entry::Bytes(v.as_bytes().to_vec());
        *e.key("file-format") = s("libtorrent resume file");
        *e.key("file-version") = int(1);
        *e.key("libtorrent-version") = s(crate::VERSION);
        if let Some(v1) = self.info_hashes.v1 {
            *e.key("info-hash") = Entry::Bytes(v1.0.to_vec());
        }
        if let Some(v2) = self.info_hashes.v2 {
            *e.key("info-hash2") = Entry::Bytes(v2.0.to_vec());
        }
        *e.key("name") = s(&self.name);
        *e.key("save_path") = s(&self.save_path);
        if let Some(p) = &self.pieces {
            *e.key("pieces") = Entry::Bytes(p.iter().map(u8::from).collect());
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
        if !self.file_priority.is_empty() {
            *e.key("file_priority") =
                Entry::List(self.file_priority.iter().map(|&p| int(p as i64)).collect());
        }
        *e.key("trackers") = Entry::List(
            self.trackers.iter().map(|tier| Entry::List(tier.iter().map(|u| s(u)).collect())).collect(),
        );
        let (mut v4, mut v6) = (Vec::new(), Vec::new());
        for p in &self.peers {
            match p {
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
        if !v4.is_empty() {
            *e.key("peers") = Entry::Bytes(v4);
        }
        if !v6.is_empty() {
            *e.key("peers6") = Entry::Bytes(v6);
        }
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
            upload_rate_limit: opt("upload_rate_limit").unwrap_or(0),
            download_rate_limit: opt("download_rate_limit").unwrap_or(0),
            max_connections: opt("max_connections"),
            max_uploads: opt("max_uploads"),
            queue_position: opt("lt-queue-position"),
            total_size: uint("lt-total-size"),
            num_pieces: opt("lt-num-pieces").unwrap_or(0),
            piece_length: opt("lt-piece-length").unwrap_or(0),
            ..Default::default()
        };
        if let Some(p) = root.dict_find_string_value("pieces") {
            let mut bf = Bitfield::new(p.len(), false);
            for (i, &b) in p.iter().enumerate() {
                bf.set(i, b & 1 != 0);
            }
            r.pieces = Some(bf);
        }
        if let Some(l) = root.dict_find_list("file_priority") {
            r.file_priority =
                l.list_iter().map(|n| n.try_int_value().unwrap_or(4).clamp(0, 7) as u8).collect();
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
        if let Some(p) = root.dict_find_string_value("peers") {
            parse_compact_peers(p, false, &mut r.peers);
        }
        if let Some(p) = root.dict_find_string_value("peers6") {
            parse_compact_peers(p, true, &mut r.peers);
        }
        if let Some(info) = root.dict_find_dict("info") {
            r.info = Some(info.data_section().to_vec());
        }
        if r.info_hashes.v1.is_none() && r.info_hashes.v2.is_none() {
            return Err(bad("missing info-hash"));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            total_uploaded: 10,
            total_downloaded: 20,
            active_time: 30,
            added_time: 1_700_000_000,
            paused: true,
            auto_managed: true,
            file_priority: vec![4, 0, 7],
            trackers: vec![vec!["http://a/announce".into()], vec!["udp://b:1".into(), "udp://c:2".into()]],
            peers: vec!["1.2.3.4:5".parse().unwrap(), "[::1]:6".parse().unwrap()],
            upload_rate_limit: 1000,
            max_connections: Some(50),
            queue_position: Some(3),
            total_size: 12345,
            num_pieces: 10,
            piece_length: 16384,
            info: Some(b"d4:name1:xe".to_vec()),
            ..Default::default()
        };
        let bytes = r.encode();
        assert_eq!(ResumeData::decode(&bytes).unwrap(), r);
        let doc = bdecode(&bytes[..]).unwrap();
        let root = doc.root().unwrap();
        assert_eq!(root.dict_find_string_value("pieces").unwrap(), &[0, 0, 0, 1, 0, 0, 0, 0, 0, 1]);
        assert_eq!(root.dict_find_int_value("max_uploads"), Some(-1));

        assert!(ResumeData::decode(b"d11:file-format3:fooe").is_err());
        assert!(ResumeData::decode(b"d11:file-format22:libtorrent resume filee").is_err(), "no info-hash");
    }
}
