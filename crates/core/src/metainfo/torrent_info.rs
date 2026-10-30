// Ported from libtorrent-rasterbar 2.0.11: src/torrent_info.cpp, include/libtorrent/torrent_info.hpp,
// is_valid_tracker_url from src/parse_url.cpp; fixes from 2.0.12 (invalid filenames become "_"),
// 2.0.13 (stricter checking of tracker URLs; v2 request sizes, aa59c7c583, 8b8242fe9b) and 2.1.0
// (max_directory_depth, remap_files removed)
// Copyright (c) 2003-2022, Arvid Norberg, Alden Torres, Andrei Kurushin, Pavel Pimenov,
// Steven Siloti and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: parsing returns `Result` instead of marking the object invalid; the info section is an
// `Arc<[u8]>` that file names borrow from; "similar"/"collections" are stored owned; there is no
// lazily cached info-dict node (call `info_dict()` to decode the section on demand).

//! A parsed `.torrent` file (libtorrent `torrent_info`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::error::MetainfoError;
use super::file_storage::{
    DEFAULT_BLOCK_SIZE, FileFlags, FileStorage, MAX_FILE_SIZE, MAX_NUM_PIECES, MAX_PIECE_SIZE,
    files_compatible,
};
use super::magnet::parse_magnet_uri;
use super::sanitize::{sanitize_append_forced_path_element, sanitize_append_path_element};
use crate::bencode::{Bdecoded, Node, bdecode, bdecode_with_limits};
use crate::crypto::crc32c::Crc32c;
use crate::crypto::{Sha1, Sha256};
use crate::flags::bit_flags;
use crate::types::{InfoHashes, Sha1Hash, Sha256Hash};
use crate::util::path::{combine_path, extension, remove_extension};
use crate::util::random::shuffle;
use crate::util::string::{
    ensure_trailing_slash, ltrim, maybe_url_encode, parse_url_components, string_begins_no_case,
    string_equal_no_case,
};
use crate::util::utf8::verify_encoding;

/// libtorrent `load_torrent_limits`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadLimits {
    /// Largest `.torrent` file accepted, in bytes.
    pub max_buffer_size: usize,
    pub max_pieces: u32,
    pub max_decode_depth: usize,
    pub max_decode_tokens: usize,
    /// Most elements in a file path or symlink target, and most levels of a v2 file tree
    /// (2.1.0). It protects against torrents with a deeply nested directory structure.
    pub max_directory_depth: usize,
}

impl Default for LoadLimits {
    fn default() -> LoadLimits {
        LoadLimits {
            max_buffer_size: 10_000_000,
            max_pieces: 0x200000,
            max_decode_depth: 100,
            max_decode_tokens: 3_000_000,
            max_directory_depth: 100,
        }
    }
}

/// Most memory the Merkle trees of one v2 torrent may need: 32 bytes per node of each file's
/// padded tree, about 64 bytes per 16 KiB block. Downloading a file expands its tree on the first
/// block (as in libtorrent), and the torrent author knows valid hashes for any size, so a torrent
/// whose trees would not fit is refused instead of an allocation of tens of gigabytes aborting the
/// client (SEC-09). 2 GiB holds the trees of 256 to 512 GiB of v2 files. libtorrent has no such
/// limit.
pub const MAX_MERKLE_TREE_BYTES: u64 = 2 << 30;

/// The memory the fully expanded Merkle trees of `files` would take (see
/// [`MAX_MERKLE_TREE_BYTES`]).
fn merkle_tree_bytes(files: &FileStorage) -> u64 {
    (0..files.num_files())
        .filter(|&f| !files.pad_file_at(f) && files.file_size(f) > 0)
        .map(|f| {
            // from the size, not file_num_blocks: that is a u32
            let leafs = files.file_size(f).div_ceil(DEFAULT_BLOCK_SIZE as u64).next_power_of_two();
            (2 * leafs - 1) * Sha256Hash::LEN as u64
        })
        .fold(0, u64::saturating_add)
}

bit_flags! {
    /// libtorrent `torrent_info_flags_t`.
    pub struct TorrentInfoFlags: u8 {
        const MULTIFILE = 0;
        const PRIVATE = 1;
        const I2P = 2;
        const SSL = 3;
        const V2_HAS_PIECE_HASHES = 4;
    }
}

/// Where a tracker URL came from (libtorrent `announce_entry::tracker_source`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackerSource {
    Torrent,
    Client,
    MagnetLink,
    TrackerExchange,
}

/// libtorrent `is_valid_tracker_url` (2.0.13): an http, https or udp URL without control
/// characters or spaces (RFC 3986 requires both to be percent-encoded, and control characters
/// enable header injection) whose host is a non-empty RFC 952/1123 LDH name or IP address. IPv6
/// literals may also hold ':', '%' (RFC 6874 zone separator), '_' and '~'.
pub fn is_valid_tracker_url(url: &str) -> bool {
    if url.is_empty() || url.bytes().any(|c| c <= 0x20 || c == 0x7f) {
        return false;
    }
    let u = url.as_bytes();
    if !string_begins_no_case(b"http://", u)
        && !string_begins_no_case(b"https://", u)
        && !string_begins_no_case(b"udp://", u)
    {
        return false;
    }
    let Ok(parts) = parse_url_components(url) else { return false };
    // libtorrent does this check inside parse_url_components; here it applies to tracker URLs
    // only. Whether the host was bracketed, found the way parse_url_components finds it.
    let rest = &url[url.find("://").expect("scheme checked") + 3..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let start = match (rest.find('@'), rest.find(':')) {
        (Some(at), Some(c)) if c < at && at < end => at + 1,
        _ => 0,
    };
    let ipv6_literal = rest[start..end].starts_with('[');
    !parts.host.is_empty()
        && parts.host.bytes().all(|c| {
            c.is_ascii_alphanumeric()
                || c == b'-'
                || c == b'.'
                || (ipv6_literal && matches!(c, b':' | b'%' | b'_' | b'~'))
        })
}

/// A tracker URL from the metadata (the static part of libtorrent `announce_entry`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnounceUrl {
    pub url: String,
    pub tier: u8,
    pub source: TrackerSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WebSeedKind {
    /// BEP 19 (GetRight style, "url-list").
    UrlSeed,
    /// BEP 17 (Hoffman style, "httpseeds").
    HttpSeed,
}

/// libtorrent `web_seed_entry`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSeed {
    pub url: String,
    pub kind: WebSeedKind,
    pub auth: String,
    pub extra_headers: Vec<(String, String)>,
}

impl WebSeed {
    pub fn new(url: impl Into<String>, kind: WebSeedKind) -> WebSeed {
        WebSeed { url: url.into(), kind, auth: String::new(), extra_headers: Vec::new() }
    }
}

/// A parsed torrent.
#[derive(Clone, Debug, Default)]
pub struct TorrentInfo {
    files: FileStorage,
    /// The original layout, kept once files are renamed (libtorrent `m_orig_files`).
    orig_files: Option<Box<FileStorage>>,
    trackers: Vec<AnnounceUrl>,
    web_seeds: Vec<WebSeed>,
    nodes: Vec<(String, u16)>,
    similar_torrents: Vec<Sha1Hash>,
    collections: Vec<String>,
    /// v2 piece layers, per file (empty where absent).
    piece_layers: Vec<Vec<u8>>,
    info_section: Option<Arc<[u8]>>,
    /// Offset of the v1 "pieces" string in the info section (0 = none).
    piece_hashes: u32,
    info_hashes: InfoHashes,
    comment: String,
    created_by: String,
    creation_date: Option<i64>,
    flags: TorrentInfoFlags,
}

impl TorrentInfo {
    /// Parses a `.torrent` file's contents with libtorrent's default limits.
    pub fn from_bytes(buf: &[u8]) -> Result<TorrentInfo, MetainfoError> {
        TorrentInfo::from_bytes_with_limits(buf, LoadLimits::default())
    }

    pub fn from_bytes_with_limits(buf: &[u8], limits: LoadLimits) -> Result<TorrentInfo, MetainfoError> {
        if buf.len() > limits.max_buffer_size {
            return Err(MetainfoError::MetadataTooLarge);
        }
        let tree = bdecode_with_limits(buf, limits.max_decode_depth, limits.max_decode_tokens)
            .map_err(|e| MetainfoError::Bdecode(e.kind))?;
        let root = tree.root().ok_or(MetainfoError::TorrentIsNoDict)?;
        TorrentInfo::parse_torrent_file(root, &limits)
    }

    /// Reads and parses a `.torrent` file.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<TorrentInfo, crate::Error> {
        let limits = LoadLimits::default();
        let len = std::fs::metadata(path.as_ref())?.len();
        if len > limits.max_buffer_size as u64 {
            return Err(MetainfoError::MetadataTooLarge.into());
        }
        let buf = std::fs::read(path)?;
        Ok(TorrentInfo::from_bytes_with_limits(&buf, limits)?)
    }

    /// A torrent known only by its info-hash (magnet links before metadata arrives).
    pub fn from_info_hashes(info_hashes: InfoHashes) -> TorrentInfo {
        TorrentInfo { info_hashes, ..TorrentInfo::default() }
    }

    /// Builds from a bare info dictionary (as received over ut_metadata).
    pub fn from_info_section(section: &[u8], limits: LoadLimits) -> Result<TorrentInfo, MetainfoError> {
        let tree = bdecode_with_limits(section, limits.max_decode_depth, limits.max_decode_tokens)
            .map_err(|e| MetainfoError::Bdecode(e.kind))?;
        let info = tree.root().ok_or(MetainfoError::TorrentInfoNoDict)?;
        let mut ti = TorrentInfo::default();
        ti.parse_info_section(info, &limits)?;
        ti.resolve_duplicate_filenames();
        Ok(ti)
    }

    fn parse_torrent_file(root: Node<'_>, limits: &LoadLimits) -> Result<TorrentInfo, MetainfoError> {
        if !root.is_dict() {
            return Err(MetainfoError::TorrentIsNoDict);
        }
        let mut ti = TorrentInfo::default();
        let Some(info) = root.dict_find_dict("info") else {
            // a "torrent" that is just a magnet link
            if let Some(uri) = root.dict_find_string_value("magnet-uri") {
                let p = parse_magnet_uri(&String::from_utf8_lossy(uri))?;
                ti.info_hashes = p.info_hashes;
                for (url, tier) in p.trackers.into_iter().zip(p.tracker_tiers) {
                    ti.trackers.push(AnnounceUrl {
                        url,
                        tier: tier.min(255) as u8,
                        source: TrackerSource::MagnetLink,
                    });
                }
                return Ok(ti);
            }
            return Err(MetainfoError::TorrentMissingInfo);
        };

        ti.parse_info_section(info, limits)?;
        ti.resolve_duplicate_filenames();

        if ti.info_hashes.has_v2() {
            // piece layers are optional (like a magnet link), but must be valid if present
            if let Some(layers) = root.dict_find_dict("piece layers") {
                ti.parse_piece_layers(layers)?;
            }
        }

        if let Some(similar) = root.dict_find_list("similar") {
            for s in similar.list_iter().filter(|n| n.is_string() && n.string_length() == 20) {
                ti.similar_torrents.push(Sha1Hash::from_slice(s.string_value()).expect("20 bytes"));
            }
        }
        if let Some(collections) = root.dict_find_list("collections") {
            for c in collections.list_iter().filter(Node::is_string) {
                ti.collections.push(String::from_utf8_lossy(c.string_value()).into_owned());
            }
        }

        // trackers: announce-list tiers (shuffled within each tier), else announce
        if let Some(tiers) = root.dict_find_list("announce-list") {
            for (j, tier) in tiers.list_iter().enumerate() {
                if !tier.is_list() {
                    continue;
                }
                for url in tier.list_iter() {
                    let url = String::from_utf8_lossy(url.string_value());
                    let url = ltrim(&url);
                    if !is_valid_tracker_url(url) {
                        continue;
                    }
                    ti.trackers.push(AnnounceUrl {
                        url: url.to_owned(),
                        tier: j.min(255) as u8,
                        source: TrackerSource::Torrent,
                    });
                }
            }
            shuffle(&mut ti.trackers);
            ti.trackers.sort_by_key(|t| t.tier); // stable: keeps the shuffle within tiers
        }
        if ti.trackers.is_empty() {
            let url = String::from_utf8_lossy(root.dict_find_string_value("announce").unwrap_or_default());
            let url = ltrim(&url);
            if is_valid_tracker_url(url) {
                ti.trackers.push(AnnounceUrl {
                    url: url.to_owned(),
                    tier: 0,
                    source: TrackerSource::Torrent,
                });
            }
        }

        if let Some(nodes) = root.dict_find_list("nodes") {
            for n in nodes.list_iter() {
                let (Some(host), Some(port)) = (n.list_at(0), n.list_at(1)) else { continue };
                if !n.is_list() || !host.is_string() || !port.is_int() {
                    continue;
                }
                ti.nodes.push((
                    String::from_utf8_lossy(host.string_value()).into_owned(),
                    port.int_value() as u16,
                ));
            }
        }

        let cd = root.dict_find_int_value("creation date").unwrap_or(-1);
        if cd >= 0 {
            ti.creation_date = Some(cd);
        }

        let multi = ti.flags.contains(TorrentInfoFlags::MULTIFILE) && ti.num_files() > 1;
        let url_seed = |s: &[u8]| {
            let mut url = maybe_url_encode(&String::from_utf8_lossy(s));
            if multi {
                ensure_trailing_slash(&mut url);
            }
            url
        };
        match root.dict_find("url-list") {
            Some(u) if u.is_string() && u.string_length() > 0 => {
                ti.web_seeds.push(WebSeed::new(url_seed(u.string_value()), WebSeedKind::UrlSeed));
            }
            Some(list) if list.is_list() => {
                let mut unique = HashSet::new();
                for u in list.list_iter().filter(|u| u.is_string() && u.string_length() > 0) {
                    let url = url_seed(u.string_value());
                    if unique.insert(url.clone()) {
                        ti.web_seeds.push(WebSeed::new(url, WebSeedKind::UrlSeed));
                    }
                }
            }
            _ => {}
        }
        match root.dict_find("httpseeds") {
            Some(u) if u.is_string() && u.string_length() > 0 => {
                let url = maybe_url_encode(&String::from_utf8_lossy(u.string_value()));
                ti.web_seeds.push(WebSeed::new(url, WebSeedKind::HttpSeed));
            }
            Some(list) if list.is_list() => {
                let mut unique = HashSet::new();
                for u in list.list_iter().filter(|u| u.is_string() && u.string_length() > 0) {
                    let url = maybe_url_encode(&String::from_utf8_lossy(u.string_value()));
                    if unique.insert(url.clone()) {
                        ti.web_seeds.push(WebSeed::new(url, WebSeedKind::HttpSeed));
                    }
                }
            }
            _ => {}
        }

        let text = |key_utf8: &str, key: &str| {
            let v = root.dict_find_string_value(key_utf8).filter(|v| !v.is_empty());
            verify_encoding(v.or_else(|| root.dict_find_string_value(key)).unwrap_or_default()).0
        };
        ti.comment = text("comment.utf-8", "comment");
        ti.created_by = text("created by.utf-8", "created by");
        Ok(ti)
    }

    /// libtorrent `parse_info_section`.
    fn parse_info_section(&mut self, info: Node<'_>, limits: &LoadLimits) -> Result<(), MetainfoError> {
        let max_pieces = limits.max_pieces;
        if !info.is_dict() {
            return Err(MetainfoError::TorrentInfoNoDict);
        }
        let section = info.data_section();
        let version = info.dict_find_int_value("meta version").unwrap_or(-1);
        let mut v1 = Some(Sha1::digest(section));
        // libtorrent always computes the v2 hash and clears it for v1 torrents; skip the work
        let v2 = (version >= 2).then(|| Sha256::digest(section));
        if section.len() >= i32::MAX as usize {
            return Err(MetainfoError::MetadataTooLarge);
        }
        if section.first() != Some(&b'd') || section.last() != Some(&b'e') {
            return Err(MetainfoError::InvalidBencoding);
        }
        let buffer: Arc<[u8]> = Arc::from(section);
        let info_offset = info.data_offset();
        // offset/len of a string node within our copy of the info section
        let span = |n: Node<'_>| ((n.string_offset() - info_offset) as u32, n.string_length() as u32);

        if version > 0 {
            if info.has_soft_error().is_some() {
                return Err(MetainfoError::InvalidBencoding);
            }
            if version > 2 {
                return Err(MetainfoError::TorrentUnknownVersion);
            }
        }

        let piece_length = info.dict_find_int_value("piece length").unwrap_or(-1);
        if piece_length <= 0 || piece_length > MAX_PIECE_SIZE as i64 {
            return Err(MetainfoError::TorrentMissingPieceLength);
        }
        // BEP 52: a power of two and at least 16 KiB
        if version > 1 && (piece_length < DEFAULT_BLOCK_SIZE as i64 || piece_length & (piece_length - 1) != 0)
        {
            return Err(MetainfoError::TorrentMissingPieceLength);
        }

        let mut files = FileStorage::with_buffer(buffer.clone());
        files.set_piece_length(piece_length as u32);

        let name_ent = info.dict_find_string("name.utf-8").or_else(|| info.dict_find_string("name"));
        let Some(name_ent) = name_ent else { return Err(MetainfoError::TorrentMissingName) };
        let mut name = String::new();
        sanitize_append_path_element(&mut name, name_ent.string_value());
        if name.is_empty() {
            // nothing usable left: name it after the info-hash
            name = match (v1, v2) {
                (Some(h), _) => h.to_hex(),
                (None, Some(h)) => h.to_hex(),
                _ => unreachable!(),
            };
        }

        // hybrid torrents: parse the v1 list separately and compare
        let mut v1_files = if version >= 2 { Some(files.clone()) } else { None };

        let files_node = info.dict_find_list("files");
        let file_tree = info.dict_find_dict("file tree");
        let ctx = ExtractCtx { span: &span, max_directory_depth: limits.max_directory_depth };
        let mut multifile = self.flags.contains(TorrentInfoFlags::MULTIFILE);
        if version >= 2 {
            let Some(tree) = file_tree else { return Err(MetainfoError::TorrentMissingFileTree) };
            extract_files2(tree, &mut files, &name, files_node.is_some(), 0, &ctx)?;
            files.sanitize_symlinks();
            multifile = files.num_files() > 1;
        } else if file_tree.is_some() {
            return Err(MetainfoError::TorrentMissingMetaVersion);
        }

        match files_node {
            None => {
                // v2 torrents may lack "length": then they are v2-only
                if version < 2 || info.dict_find("length").is_some() {
                    let target = v1_files.as_mut().unwrap_or(&mut files);
                    extract_single_file(info, target, "", true, &ctx)?;
                    multifile = false;
                } else {
                    v1 = None;
                }
            }
            Some(list) => {
                let target = v1_files.as_mut().unwrap_or(&mut files);
                extract_files(list, target, &name, &ctx)?;
                multifile = true;
            }
        }
        self.flags.set(TorrentInfoFlags::MULTIFILE, multifile);

        if files.num_files() == 0 {
            return Err(MetainfoError::NoFilesInTorrent);
        }
        if files.name().is_empty() {
            return Err(MetainfoError::TorrentMissingName);
        }

        // hybrid torrents must describe the same files in v1 and v2
        if let Some(v1f) = &v1_files {
            if v1f.num_files() > 0 {
                // libtorrent 2.0.0-2.0.7 made hybrids without tail padding; accept both
                if files.num_files() == v1f.num_files() + 1 {
                    files.remove_tail_padding();
                }
                if !files_compatible(&files, v1f) {
                    return Err(MetainfoError::TorrentInconsistentFiles);
                }
            }
        }

        let pl = files.piece_length() as u64;
        if files.total_size() / pl > MAX_NUM_PIECES as u64 {
            return Err(MetainfoError::TooManyPiecesInTorrent);
        }
        // the blocks are the pieces of the v2 trees (SEC-09)
        if files.v2() && merkle_tree_bytes(&files) > MAX_MERKLE_TREE_BYTES {
            return Err(MetainfoError::TooManyPiecesInTorrent);
        }
        let num_pieces = files.total_size().div_ceil(pl);
        if num_pieces >= (i32::MAX / 20) as u64 || num_pieces > max_pieces as u64 {
            return Err(MetainfoError::TooManyPiecesInTorrent);
        }
        files.set_num_pieces(num_pieces as u32);

        match info.dict_find_string("pieces") {
            None if version < 2 => return Err(MetainfoError::TorrentMissingPieces),
            // A hybrid (v2 metadata that also has a v1 file list) needs v1 piece hashes too.
            // libtorrent accepts it and would later hash-check v1 pieces against garbage.
            None if v1.is_some() => return Err(MetainfoError::TorrentMissingPieces),
            None => {}
            Some(pieces) => {
                if pieces.string_length() as u64 != num_pieces * 20 {
                    return Err(MetainfoError::TorrentInvalidHashes);
                }
                self.piece_hashes = span(pieces).0;
            }
        }

        self.flags.set(TorrentInfoFlags::PRIVATE, info.dict_find_int_value("private").unwrap_or(0) != 0);

        if let Some(similar) = info.dict_find_list("similar") {
            for s in similar.list_iter().filter(|n| n.is_string() && n.string_length() == 20) {
                self.similar_torrents.push(Sha1Hash::from_slice(s.string_value()).expect("20 bytes"));
            }
        }
        if let Some(collections) = info.dict_find_list("collections") {
            for c in collections.list_iter().filter(Node::is_string) {
                self.collections.push(String::from_utf8_lossy(c.string_value()).into_owned());
            }
        }
        if info.dict_find_string("ssl-cert").is_some() {
            self.flags.insert(TorrentInfoFlags::SSL);
        }

        if files.total_size() == 0 {
            return Err(MetainfoError::TorrentInvalidLength);
        }

        self.files = files;
        self.info_section = Some(buffer);
        self.info_hashes = InfoHashes { v1, v2 };
        debug_assert_eq!(self.info_hashes.has_v2(), self.files.v2());
        Ok(())
    }

    /// libtorrent `parse_piece_layers`: structural checks only; hashes are verified against the
    /// merkle roots when the torrent is loaded (Phase 8).
    fn parse_piece_layers(&mut self, e: Node<'_>) -> Result<(), MetainfoError> {
        let fs = self.orig_files();
        let pl = fs.piece_length() as u64;
        let all_roots: HashSet<Sha256Hash> =
            (0..fs.num_files()).filter(|&i| fs.file_size(i) > pl).map(|i| fs.root(i)).collect();

        let mut layers: HashMap<Sha256Hash, &[u8]> = HashMap::new();
        for (key, value) in e.dict_iter() {
            if key.len() != 32 || !value.is_string() || value.string_length() % 32 != 0 {
                return Err(MetainfoError::TorrentInvalidPieceLayer);
            }
            let root = Sha256Hash::from_slice(key).expect("32 bytes");
            if !all_roots.contains(&root) {
                // refers to no file in this torrent
                return Err(MetainfoError::TorrentInvalidPieceLayer);
            }
            layers.insert(root, value.string_value());
        }

        let mut piece_layers = vec![Vec::new(); fs.num_files()];
        for (i, slot) in piece_layers.iter_mut().enumerate() {
            if fs.file_size(i) <= pl {
                continue;
            }
            let Some(hashes) = layers.get(&fs.root(i)) else { continue };
            if hashes.len() as u64 != fs.file_num_pieces(i) as u64 * 32 {
                return Err(MetainfoError::TorrentInvalidPieceLayer);
            }
            *slot = hashes.to_vec();
        }
        self.piece_layers = piece_layers;
        self.flags.insert(TorrentInfoFlags::V2_HAS_PIECE_HASHES);
        Ok(())
    }

    /// Renames files whose paths collide (case-insensitively) with other files or directories
    /// by appending ".1", ".2", ... before the extension.
    fn resolve_duplicate_filenames(&mut self) {
        let mut seen = HashSet::new();
        self.files.all_path_hashes(&mut seen);
        for i in 0..self.files.num_files() {
            if !seen.insert(self.files.file_path_hash(i, "")) {
                // a collision: redo it the slow way, comparing full names
                self.resolve_duplicate_filenames_slow();
                return;
            }
        }
    }

    fn resolve_duplicate_filenames_slow(&mut self) {
        #[derive(Clone, Copy)]
        enum Entry {
            /// A directory: `len` bytes of `paths[index]`.
            Dir {
                index: usize,
                len: usize,
            },
            File(usize),
        }
        let mut table: HashMap<u32, Vec<Entry>> = HashMap::new();
        let lower =
            |crc: &mut Crc32c, s: &str| s.bytes().for_each(|c| crc.update_byte(c.to_ascii_lowercase()));

        let mut base = Crc32c::new();
        lower(&mut base, self.files.name());
        let paths = self.files.paths().to_vec();
        for (index, path) in paths.iter().enumerate() {
            let mut crc = base;
            if !path.is_empty() {
                crc.update_byte(b'/');
            }
            for (count, c) in path.bytes().enumerate() {
                if c == b'/' {
                    table.entry(crc.checksum()).or_default().push(Entry::Dir { index, len: count });
                }
                crc.update_byte(c.to_ascii_lowercase());
            }
            table.entry(crc.checksum()).or_default().push(Entry::Dir { index, len: path.len() });
        }

        for i in 0..self.files.num_files() {
            let hash = self.files.file_path_hash(i, "");
            let my_path = self.files.file_path(i, "");
            let other_name = |e: &Entry, fs: &FileStorage| match *e {
                Entry::Dir { index, len } => combine_path(fs.name(), &paths[index][..len]),
                Entry::File(f) => fs.file_path(f, ""),
            };
            let matched = table
                .get(&hash)
                .and_then(|v| {
                    v.iter().find(|e| {
                        string_equal_no_case(other_name(e, &self.files).as_bytes(), my_path.as_bytes())
                    })
                })
                .copied();
            let Some(m) = matched else {
                table.entry(hash).or_default().push(Entry::File(i));
                continue;
            };
            // pad files may collide with each other, if they have the same size
            if let Entry::File(other) = m {
                if self.files.pad_file_at(i)
                    && self.files.pad_file_at(other)
                    && self.files.file_size(i) == self.files.file_size(other)
                {
                    continue;
                }
            }
            let base_name = remove_extension(&my_path).to_owned();
            let ext = extension(&my_path).to_owned();
            let mut cnt = 0;
            let new_name = loop {
                cnt += 1;
                let candidate = format!("{base_name}.{cnt}{ext}");
                let mut crc = Crc32c::new();
                lower(&mut crc, &candidate);
                let h = crc.checksum();
                if !table.contains_key(&h) {
                    table.entry(h).or_default().push(Entry::File(i));
                    break candidate;
                }
            };
            self.copy_on_write();
            self.files.rename_file(i, &new_name);
        }
    }

    fn copy_on_write(&mut self) {
        if self.orig_files.is_none() {
            self.orig_files = Some(Box::new(self.files.clone()));
        }
    }

    // --- accessors (libtorrent names) ---

    pub fn files(&self) -> &FileStorage {
        &self.files
    }

    /// The layout as described by the metadata, before any renames.
    pub fn orig_files(&self) -> &FileStorage {
        self.orig_files.as_deref().unwrap_or(&self.files)
    }

    pub fn rename_file(&mut self, index: usize, new_name: &str) {
        if self.files.file_path(index, "") == new_name {
            return;
        }
        self.copy_on_write();
        self.files.rename_file(index, new_name);
    }

    pub fn is_valid(&self) -> bool {
        self.files.is_valid()
    }

    pub fn name(&self) -> &str {
        self.files.name()
    }

    pub fn num_files(&self) -> usize {
        self.files.num_files()
    }

    pub fn total_size(&self) -> u64 {
        self.files.total_size()
    }

    pub fn piece_length(&self) -> u32 {
        self.files.piece_length()
    }

    pub fn num_pieces(&self) -> u32 {
        self.files.num_pieces()
    }

    pub fn piece_size(&self, index: u32) -> u32 {
        self.files.piece_size(index)
    }

    /// The piece size requests are sized by (libtorrent 2.0.13 `piece_size_for_req`): pieces
    /// at the end of files are shorter in v2-only torrents; v1 and hybrid torrents need full
    /// pieces (but the last) to compute the SHA-1 piece hash.
    pub fn piece_size_for_req(&self, index: u32) -> u32 {
        if self.v1() { self.files.piece_size(index) } else { self.files.piece_size2(index) }
    }

    /// SHA-1 of piece `index` (v1 torrents).
    pub fn hash_for_piece(&self, index: u32) -> Option<Sha1Hash> {
        if self.piece_hashes == 0 || index >= self.num_pieces() {
            return None;
        }
        let start = self.piece_hashes as usize + index as usize * 20;
        Sha1Hash::from_slice(&self.info_section.as_deref()?[start..start + 20])
    }

    pub fn info_hashes(&self) -> InfoHashes {
        self.info_hashes
    }

    /// The hash used on the wire (v1, or truncated v2 for v2-only torrents).
    pub fn info_hash(&self) -> Sha1Hash {
        self.info_hashes.best_v1_compatible()
    }

    pub fn v1(&self) -> bool {
        self.info_hashes.has_v1()
    }

    pub fn v2(&self) -> bool {
        self.info_hashes.has_v2()
    }

    pub fn trackers(&self) -> &[AnnounceUrl] {
        &self.trackers
    }

    pub fn add_tracker(&mut self, url: &str, tier: u8, source: TrackerSource) {
        if self.trackers.iter().any(|t| t.url == url) || !is_valid_tracker_url(url) {
            return;
        }
        self.trackers.push(AnnounceUrl { url: url.to_owned(), tier, source });
        self.trackers.sort_by_key(|t| t.tier);
    }

    pub fn clear_trackers(&mut self) {
        self.trackers.clear();
    }

    pub fn web_seeds(&self) -> &[WebSeed] {
        &self.web_seeds
    }

    pub fn add_url_seed(&mut self, url: &str) {
        self.web_seeds.push(WebSeed::new(url, WebSeedKind::UrlSeed));
    }

    pub fn add_http_seed(&mut self, url: &str) {
        self.web_seeds.push(WebSeed::new(url, WebSeedKind::HttpSeed));
    }

    pub fn set_web_seeds(&mut self, seeds: Vec<WebSeed>) {
        self.web_seeds = seeds;
    }

    /// DHT bootstrap nodes from the "nodes" key.
    pub fn nodes(&self) -> &[(String, u16)] {
        &self.nodes
    }

    pub fn similar_torrents(&self) -> &[Sha1Hash] {
        &self.similar_torrents
    }

    pub fn collections(&self) -> &[String] {
        &self.collections
    }

    pub fn comment(&self) -> &str {
        &self.comment
    }

    pub fn creator(&self) -> &str {
        &self.created_by
    }

    /// Unix seconds, if the torrent has a creation date.
    pub fn creation_date(&self) -> Option<i64> {
        self.creation_date
    }

    pub fn is_private(&self) -> bool {
        self.flags.contains(TorrentInfoFlags::PRIVATE)
    }

    pub fn flags(&self) -> TorrentInfoFlags {
        self.flags
    }

    /// The raw bencoded info dictionary (what ut_metadata transfers).
    pub fn info_section(&self) -> &[u8] {
        self.info_section.as_deref().unwrap_or_default()
    }

    /// The info section as a shared buffer, for keeping alongside other structures.
    pub fn info_section_arc(&self) -> Option<Arc<[u8]>> {
        self.info_section.clone()
    }

    /// Decodes the info dictionary on demand (libtorrent `info()` keeps it cached instead).
    pub fn info_dict(&self) -> Option<Bdecoded<Arc<[u8]>>> {
        crate::bencode::bdecode(self.info_section.clone()?).ok()
    }

    pub fn ssl_cert(&self) -> Option<Vec<u8>> {
        if !self.flags.contains(TorrentInfoFlags::SSL) {
            return None;
        }
        let d = self.info_dict()?;
        d.root()?.dict_find_string_value("ssl-cert").map(<[u8]>::to_vec)
    }

    /// v2 piece hashes of `file` (the root itself for single-piece files).
    pub fn piece_layer(&self, file: usize) -> &[u8] {
        let fs = self.orig_files();
        if file >= fs.num_files() || fs.pad_file_at(file) {
            return &[];
        }
        if fs.file_size(file) <= self.piece_length() as u64 {
            // a single-piece file: its root is its only piece hash
            return fs.root_bytes(file);
        }
        self.piece_layers.get(file).map_or(&[], Vec::as_slice)
    }

    /// v2: the expected hash of `piece` (the merkle root over its 16 KiB blocks, from its
    /// file's piece layer, or the file's root for a single-piece file) and the number of
    /// leaves that root spans (the blocks per piece, or for a file smaller than a piece its
    /// block count rounded up to a power of two). `None` without v2 metadata, for a piece that
    /// starts in a pad file, or when the file's piece layer is not known.
    pub fn v2_piece_hash(&self, piece: u32) -> Option<(Sha256Hash, usize)> {
        if !self.v2() || piece >= self.num_pieces() {
            return None;
        }
        let fs = self.orig_files();
        let pl = fs.piece_length();
        let file = fs.file_index_at_piece(piece);
        if fs.pad_file_at(file) || fs.file_size(file) == 0 {
            return None;
        }
        let index = (piece - (fs.file_offset(file) / pl as u64) as u32) as usize;
        let layer = self.piece_layer(file);
        let hash = Sha256Hash::from_slice(layer.get(index * 32..index * 32 + 32)?)?;
        let leafs = if fs.file_size(file) < pl as u64 {
            super::merkle::merkle_num_leafs(fs.file_num_blocks(file) as usize)
        } else {
            (pl / DEFAULT_BLOCK_SIZE) as usize
        };
        Some((hash, leafs))
    }

    /// v2: every file's piece hashes are known (from the `.torrent` or added since).
    pub fn v2_piece_layers_complete(&self) -> bool {
        let fs = self.orig_files();
        self.v2()
            && (0..fs.num_files()).all(|f| {
                fs.pad_file_at(f)
                    || fs.file_size(f) == 0
                    || self.piece_layer(f).len() == fs.file_num_pieces(f) as usize * 32
            })
    }

    /// Adds the piece layers of a bencoded "piece layers" dictionary (as the session store keeps
    /// them beside the info dictionary), with the same checks as a `.torrent`'s.
    pub fn set_piece_layers(&mut self, bencoded: &[u8]) -> Result<(), MetainfoError> {
        let doc = bdecode(bencoded).map_err(|_| MetainfoError::TorrentInvalidPieceLayer)?;
        let root = doc.root().filter(Node::is_dict).ok_or(MetainfoError::TorrentInvalidPieceLayer)?;
        self.parse_piece_layers(root)
    }

    /// The piece layers as a bencoded "piece layers" dictionary (files with more than one
    /// piece only, as in a `.torrent`); `None` if there are none.
    pub fn piece_layers_bencoded(&self) -> Option<Vec<u8>> {
        let fs = self.orig_files();
        let mut layers = crate::bencode::Entry::new_dict();
        let mut any = false;
        for f in 0..fs.num_files() {
            if fs.pad_file_at(f) || fs.file_size(f) <= self.piece_length() as u64 {
                continue;
            }
            let layer = self.piece_layer(f);
            if !layer.is_empty() {
                *layers.key(fs.root_bytes(f)) = crate::bencode::Entry::Bytes(layer.to_vec());
                any = true;
            }
        }
        any.then(|| layers.bencode())
    }

    pub fn v2_piece_hashes_verified(&self) -> bool {
        self.flags.contains(TorrentInfoFlags::V2_HAS_PIECE_HASHES)
    }

    pub fn free_piece_layers(&mut self) {
        self.piece_layers = Vec::new();
        self.flags.remove(TorrentInfoFlags::V2_HAS_PIECE_HASHES);
    }

    pub fn map_block(&self, piece: u32, offset: u64, size: u64) -> Vec<super::FileSlice> {
        self.files.map_block(piece, offset, size)
    }

    pub fn map_file(&self, file: usize, offset: u64, size: u32) -> super::PeerRequest {
        self.files.map_file(file, offset, size)
    }
}

struct ExtractCtx<'a> {
    /// Maps a string node to its `(offset, len)` within the info-section copy.
    span: &'a dyn Fn(Node<'_>) -> (u32, u32),
    /// [`LoadLimits::max_directory_depth`].
    max_directory_depth: usize,
}

fn file_attributes(dict: Node<'_>) -> FileFlags {
    let mut flags = FileFlags::empty();
    for &c in dict.dict_find_string_value("attr").unwrap_or_default() {
        match c {
            b'l' => flags |= FileFlags::SYMLINK,
            b'x' => flags |= FileFlags::EXECUTABLE,
            b'h' => flags |= FileFlags::HIDDEN,
            b'p' => flags |= FileFlags::PAD_FILE,
            _ => {}
        }
    }
    flags
}

/// Sum of the element lengths plus one separator each; errors on non-string elements.
fn path_length(p: Node<'_>) -> Result<usize, MetainfoError> {
    let mut len = 0;
    for e in p.list_iter() {
        if !e.is_string() {
            return Err(MetainfoError::TorrentInvalidName);
        }
        len += e.string_length() + 1;
    }
    Ok(len)
}

fn symlink_path(dict: Node<'_>, ctx: &ExtractCtx<'_>) -> Result<Option<String>, MetainfoError> {
    let Some(sp) = dict.dict_find_list("symlink path") else { return Ok(None) };
    if sp.list_size() > ctx.max_directory_depth {
        return Err(MetainfoError::TorrentDirectoryTooDeep);
    }
    let mut out = String::with_capacity(path_length(sp)?);
    for e in sp.list_iter() {
        sanitize_append_path_element(&mut out, e.string_value());
    }
    Ok(Some(out))
}

/// The borrowed name range, if the sanitized path still ends with the original bytes.
fn borrowable(filename: Node<'_>, path: &str, ctx: &ExtractCtx<'_>) -> Option<(u32, u32)> {
    let mut raw = filename.string_value();
    let (mut off, _) = (ctx.span)(filename);
    while raw.first() == Some(&b'/') {
        raw = &raw[1..];
        off += 1;
    }
    (!raw.is_empty() && path.as_bytes().ends_with(raw)).then_some((off, raw.len() as u32))
}

/// v1 file entry ("files" list item, or the info dict itself for single-file torrents).
fn extract_single_file(
    dict: Node<'_>,
    files: &mut FileStorage,
    root_dir: &str,
    top_level: bool,
    ctx: &ExtractCtx<'_>,
) -> Result<(), MetainfoError> {
    if !dict.is_dict() {
        return Err(MetainfoError::TorrentFileParseFailed);
    }
    let mut flags = file_attributes(dict);
    let size =
        if flags.contains(FileFlags::SYMLINK) { 0 } else { dict.dict_find_int_value("length").unwrap_or(-1) };
    if size < 0
        || (size as u64 / DEFAULT_BLOCK_SIZE as u64) >= (i32::MAX / 2) as u64
        || size as u64 > MAX_FILE_SIZE
    {
        return Err(MetainfoError::TorrentInvalidLength);
    }
    let size = size as u64;
    let mtime = dict.dict_find_int_value("mtime").unwrap_or(0);

    let mut path = root_dir.to_owned();
    let mut name = None;
    if top_level {
        // prefer name.utf-8: if present, it is more likely to be correctly encoded
        let p = dict.dict_find_string("name.utf-8").or_else(|| dict.dict_find_string("name"));
        let Some(p) = p.filter(|p| p.string_length() > 0) else {
            return Err(MetainfoError::TorrentMissingName);
        };
        sanitize_append_path_element(&mut path, p.string_value());
        if path.is_empty() {
            return Err(MetainfoError::TorrentMissingName);
        }
        name = borrowable(p, &path, ctx);
    } else {
        let p = dict.dict_find_list("path.utf-8").or_else(|| dict.dict_find_list("path"));
        match p {
            Some(p) if p.list_size() > 0 => {
                if p.list_size() > ctx.max_directory_depth {
                    return Err(MetainfoError::TorrentDirectoryTooDeep);
                }
                path.reserve(path_length(p)?);
                let mut last = None;
                for e in p.list_iter() {
                    // every element adds something: invalid names become "_"
                    sanitize_append_forced_path_element(&mut path, e.string_value());
                    last = Some(e);
                }
                name = last.and_then(|e| borrowable(e, &path, ctx));
            }
            _ if flags.contains(FileFlags::PAD_FILE) => {
                // pad files don't need a path; they live under ".pad"
                path = combine_path(".pad", &size.to_string());
            }
            _ => return Err(MetainfoError::TorrentMissingName),
        }
    }

    // BitComet pad files
    if path.contains("_____padding_file_") {
        flags |= FileFlags::PAD_FILE;
    }

    let filehash = dict
        .dict_find_string("sha1")
        .filter(|h| h.string_length() == 20)
        .and_then(|h| Sha1Hash::from_slice(h.string_value()));

    let mut link = String::new();
    if flags.contains(FileFlags::SYMLINK) {
        match symlink_path(dict, ctx)? {
            Some(l) => link = l,
            // technically invalid: a symlink must have a target
            None => flags.remove(FileFlags::SYMLINK),
        }
    }

    files.add_file_borrow(name, &path, size, flags, filehash, mtime, &link, None)?;
    Ok(())
}

/// v2 file entry (a leaf of the "file tree").
fn extract_single_file2(
    dict: Node<'_>,
    files: &mut FileStorage,
    path: &str,
    name: Option<(u32, u32)>,
    ctx: &ExtractCtx<'_>,
) -> Result<(), MetainfoError> {
    if !dict.is_dict() {
        return Err(MetainfoError::TorrentFileParseFailed);
    }
    let flags = file_attributes(dict);
    if flags.contains(FileFlags::PAD_FILE) {
        return Err(MetainfoError::TorrentInvalidPadFile);
    }
    let size =
        if flags.contains(FileFlags::SYMLINK) { 0 } else { dict.dict_find_int_value("length").unwrap_or(-1) };
    if size < 0
        || (size as u64 / DEFAULT_BLOCK_SIZE as u64) >= MAX_NUM_PIECES as u64
        || size as u64 > MAX_FILE_SIZE
    {
        return Err(MetainfoError::TorrentInvalidLength);
    }
    let size = size as u64;
    let mtime = dict.dict_find_int_value("mtime").unwrap_or(0);

    let link = if flags.contains(FileFlags::SYMLINK) {
        symlink_path(dict, ctx)?.unwrap_or_default()
    } else {
        String::new()
    };

    let mut root = None;
    if link.is_empty() && size > 0 {
        let r = dict.dict_find_string("pieces root").filter(|r| r.string_length() == 32);
        let r = r
            .and_then(|r| Sha256Hash::from_slice(r.string_value()))
            .ok_or(MetainfoError::TorrentMissingPiecesRoot)?;
        if r.is_zero() {
            return Err(MetainfoError::TorrentMissingPiecesRoot);
        }
        root = Some(r);
    }
    files.add_file_borrow(name, path, size, flags, None, mtime, &link, root)?;
    Ok(())
}

/// Walks the v2 "file tree" recursively (at most `max_directory_depth` levels deep, which also
/// bounds the stack; the bdecode depth limit bounds it further).
fn extract_files2(
    tree: Node<'_>,
    target: &mut FileStorage,
    root_dir: &str,
    has_files: bool,
    depth: usize,
    ctx: &ExtractCtx<'_>,
) -> Result<(), MetainfoError> {
    if !tree.is_dict() {
        return Err(MetainfoError::TorrentFileParseFailed);
    }
    if depth > ctx.max_directory_depth {
        return Err(MetainfoError::TorrentDirectoryTooDeep);
    }
    let size = tree.dict_size();
    for (key, value) in tree.dict_iter().nodes() {
        if !value.is_dict() || key.string_length() == 0 {
            return Err(MetainfoError::TorrentFileParseFailed);
        }
        let leaf = value.dict_size() == 1 && value.dict_at(0).is_some_and(|(k, _)| k.is_empty());
        let single_file = leaf && !has_files && size == 1;
        let mut path = if single_file { String::new() } else { root_dir.to_owned() };
        sanitize_append_forced_path_element(
            &mut path,
            key.string_value().strip_prefix(b"/").unwrap_or(key.string_value()),
        );
        if leaf {
            let name = borrowable(key, &path, ctx);
            let (_, entry) = value.dict_at(0).expect("leaf has one entry");
            extract_single_file2(entry, target, &path, name, ctx)?;
            continue;
        }
        extract_files2(value, target, &path, true, depth + 1, ctx)?;
    }
    Ok(())
}

/// The v1 "files" list. `root_dir` is the torrent name.
fn extract_files(
    list: Node<'_>,
    target: &mut FileStorage,
    root_dir: &str,
    ctx: &ExtractCtx<'_>,
) -> Result<(), MetainfoError> {
    if !list.is_list() {
        return Err(MetainfoError::TorrentFileParseFailed);
    }
    target.reserve(list.list_size());
    for f in list.list_iter() {
        extract_single_file(f, target, root_dir, false, ctx)?;
    }
    // invalid symlinks are rewritten to point to themselves
    target.sanitize_symlinks();
    Ok(())
}

impl From<MetainfoError> for crate::Error {
    fn from(e: MetainfoError) -> crate::Error {
        crate::Error::Metainfo(e)
    }
}

#[cfg(test)]
mod tests {
    //! Ported from libtorrent-rasterbar 2.0.11 test/test_torrent_info.cpp, run against the same
    //! test_torrents fixtures (crates/core/tests/fixtures/torrents).
    use super::*;
    use crate::bencode::BdecodeErrorKind;
    use crate::bencode::Entry;

    fn fixture(name: &str) -> Vec<u8> {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/torrents").join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn load(name: &str) -> TorrentInfo {
        TorrentInfo::from_bytes(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn sanity_check(ti: &TorrentInfo) {
        assert!(ti.piece_length() <= MAX_PIECE_SIZE);
        assert!(ti.num_pieces() > 0);
        assert_eq!(ti.v2(), ti.files().v2());
        // every file maps into the piece range
        let fs = ti.files();
        for i in 0..fs.num_files() {
            let last = fs.file_size(i).saturating_sub(1);
            assert!(ti.map_file(i, last, 0).piece <= ti.num_pieces());
        }
        if ti.v1() {
            assert!(ti.hash_for_piece(ti.num_pieces() - 1).is_some());
        }
    }

    const GOOD: &[&str] = &[
        "base.torrent",
        "empty_path.torrent",
        "parent_path.torrent",
        "hidden_parent_path.torrent",
        "single_multi_file.torrent",
        "slash_path.torrent",
        "slash_path2.torrent",
        "slash_path3.torrent",
        "backslash_path.torrent",
        "url_list.torrent",
        "url_list2.torrent",
        "url_list3.torrent",
        "httpseed.torrent",
        "empty_httpseed.torrent",
        "long_name.torrent",
        "whitespace_url.torrent",
        "duplicate_files.torrent",
        "pad_file.torrent",
        "creation_date.torrent",
        "no_creation_date.torrent",
        "url_seed.torrent",
        "url_seed_multi.torrent",
        "url_seed_multi_single_file.torrent",
        "url_seed_multi_space.torrent",
        "url_seed_multi_space_nolist.torrent",
        "empty_path_multi.torrent",
        "duplicate_web_seeds.torrent",
        "invalid_name2.torrent",
        "invalid_name3.torrent",
        "symlink1.torrent",
        "symlink2.torrent",
        "unordered.torrent",
        "symlink_zero_size.torrent",
        "pad_file_no_path.torrent",
        "large.torrent",
        "absolute_filename.torrent",
        "invalid_filename.torrent",
        "invalid_filename2.torrent",
        "overlapping_symlinks.torrent",
        "v2.torrent",
        "v2_multipiece_file.torrent",
        "v2_only.torrent",
        "v2_invalid_filename.torrent",
        "v2_invalid_filename2.torrent",
        "v2_multiple_files.torrent",
        "v2_symlinks.torrent",
        "v2_hybrid.torrent",
        "empty-files-1.torrent",
        "empty-files-2.torrent",
        "empty-files-3.torrent",
        "empty-files-4.torrent",
        "empty-files-5.torrent",
        "v2_no_piece_layers.torrent",
        "v2_incomplete_piece_layer.torrent",
        "similar.torrent",
        "similar2.torrent",
        "collection.torrent",
        "collection2.torrent",
        "dht_nodes.torrent",
        "large_piece_size.torrent",
    ];

    #[test]
    fn parse_good_torrents() {
        for name in GOOD {
            let ti = load(name);
            sanity_check(&ti);
        }
    }

    #[test]
    fn fixture_expectations() {
        let ti = load("slash_path.torrent");
        assert_eq!(ti.num_files(), 1);
        assert_eq!(ti.files().file_path(0, ""), "temp/_/_/bar");

        let ti = load("slash_path2.torrent");
        assert_eq!(ti.files().file_path(0, ""), "temp/abc....def/_/bar");

        let ti = load("slash_path3.torrent");
        assert_eq!(ti.files().file_path(0, ""), "temp....abc");

        let ti = load("whitespace_url.torrent");
        assert_eq!(ti.trackers()[0].url, "udp://test.com/announce");

        let ti = load("duplicate_files.torrent");
        assert_eq!(ti.num_files(), 2);
        assert_eq!(ti.files().file_path(0, ""), "temp/foo/bar.txt");
        assert_eq!(ti.files().file_path(1, ""), "temp/foo/bar.1.txt");

        let ti = load("pad_file.torrent");
        assert_eq!(ti.num_files(), 2);
        assert!(!ti.files().file_flags(0).contains(FileFlags::PAD_FILE));
        assert!(ti.files().file_flags(1).contains(FileFlags::PAD_FILE));

        assert_eq!(load("creation_date.torrent").creation_date(), Some(1234567));
        assert_eq!(load("no_creation_date.torrent").creation_date(), None);

        let ws = |n: &str| load(n).web_seeds().iter().map(|w| w.url.clone()).collect::<Vec<_>>();
        assert_eq!(ws("url_seed.torrent"), ["http://test.com/file"]);
        assert_eq!(ws("url_seed_multi.torrent"), ["http://test.com/file/"]);
        assert_eq!(ws("url_seed_multi_single_file.torrent"), ["http://test.com/file/temp/foo/bar.txt"]);
        assert_eq!(ws("url_seed_multi_space.torrent"), ["http://test.com/test%20file/foo%20bar/"]);
        assert_eq!(ws("url_seed_multi_space_nolist.torrent"), ["http://test.com/test%20file/foo%20bar/"]);
        assert_eq!(load("duplicate_web_seeds.torrent").web_seeds().len(), 3);

        // names that sanitize to nothing become the info-hash
        assert_eq!(load("invalid_name2.torrent").name(), "b61560c2918f463768cd122b6d2fdd47b77bdb35");
        assert_eq!(load("invalid_name3.torrent").name(), if cfg!(windows) { "foobar" } else { "foobar " });

        let ti = load("symlink1.torrent");
        assert_eq!(ti.num_files(), 2);
        assert_eq!(ti.files().symlink(1), "temp/a/b/bar");

        let ti = load("symlink2.torrent");
        assert_eq!(ti.num_files(), 5);
        assert_eq!(ti.files().symlink(0), "Some.framework/Versions/A/SDL2");
        assert_eq!(ti.files().symlink(4), "Some.framework/Versions/A");

        let ti = load("symlink_zero_size.torrent");
        assert_eq!(ti.files().symlink(1), "temp/a/b/bar");

        let ti = load("pad_file_no_path.torrent");
        assert_eq!(ti.files().file_path(1, ""), ".pad/2124");

        let ti = load("absolute_filename.torrent");
        assert_eq!(ti.files().file_path(0, ""), "temp/abcde");
        assert_eq!(ti.files().file_path(1, ""), "temp/foobar");

        assert_eq!(load("invalid_filename.torrent").num_files(), 2);
        assert_eq!(load("invalid_filename2.torrent").num_files(), 3);

        for name in ["overlapping_symlinks.torrent", "v2_symlinks.torrent"] {
            let ti = load(name);
            assert!(ti.num_files() > 3);
            assert_eq!(ti.files().symlink(0), "SDL2.framework/Versions/Current/Headers");
            assert_eq!(ti.files().symlink(1), "SDL2.framework/Versions/Current/Resources");
            assert_eq!(ti.files().symlink(2), "SDL2.framework/Versions/Current/SDL2");
        }

        assert_eq!(load("large_piece_size.torrent").piece_length(), 32767 * 0x4000);

        assert_eq!(load("similar.torrent").similar_torrents(), [Sha1Hash(*b"aaaaaaaaaaaaaaaaaaaa")]);
        assert_eq!(load("similar2.torrent").similar_torrents(), [Sha1Hash(*b"aaaaaaaaaaaaaaaaaaaa")]);
        assert_eq!(load("collection.torrent").collections(), ["bar", "foo"]);
        assert_eq!(load("collection2.torrent").collections(), ["bar", "foo"]);
        assert_eq!(
            load("dht_nodes.torrent").nodes(),
            [("127.0.0.1".to_owned(), 6881), ("192.168.1.1".to_owned(), 6881)]
        );
    }

    #[test]
    fn v2_fixtures() {
        let ti = load("v2.torrent");
        assert_eq!(ti.num_files(), 1);
        assert_eq!(ti.files().file_path(0, ""), "test64K");
        assert_eq!(ti.files().file_size(0), 65536);
        assert_eq!(
            ti.files().root(0).to_hex(),
            "60aae9c7b428f87e0713e88229e18f0adf12cd7b22a0dd8a92bb2485eb7af242"
        );
        assert!(ti.v1() && ti.v2());
        assert_eq!(
            ti.info_hashes().v2.unwrap().to_hex(),
            "597b180c1a170a585dfc5e85d834d69013ceda174b8f357d5bb1a0ca509faf0a"
        );

        let ti = load("v2_multipiece_file.torrent");
        assert_eq!(ti.files().file_path(0, ""), "test1MB");
        assert_eq!(ti.files().file_size(0), 1048576);
        assert_eq!(
            ti.files().root(0).to_hex(),
            "515ea9181744b817744ded9d2e8e9dc6a8450c0b0c52e24b5077f302ffbd9008"
        );
        assert!(ti.v1() && ti.v2());
        assert_eq!(
            ti.info_hashes().v2.unwrap().to_hex(),
            "108ac2c3718ce722e6896edc56c4afa98f1d711ecaace7aad74fca418ebd03de"
        );

        let ti = load("v2.torrent");
        assert_eq!(
            ti.piece_layer(0),
            ti.files().root(0).0.as_slice(),
            "single-piece file: layer is the root"
        );

        let ti = load("v2_only.torrent");
        assert_eq!(ti.files().file_path(0, ""), "test1MB");
        assert_eq!(ti.piece_layer(0).len(), ti.files().file_num_pieces(0) as usize * 32);
        assert!(!ti.v1() && ti.v2());
        assert_eq!(
            ti.info_hashes().v2.unwrap().to_hex(),
            "95e04d0c4bad94ab206efa884666fd89777dbe4f7bd9945af1829037a85c6192"
        );
        assert!(ti.v2_piece_hashes_verified());

        assert_eq!(load("v2_invalid_filename.torrent").files().file_path(0, ""), "_estMB");

        let ti = load("v2_invalid_filename2.torrent");
        assert_eq!(ti.num_files(), 3);
        assert_eq!(ti.files().file_path(0, ""), "test/_");
        assert_eq!(ti.files().file_path(1, ""), "test/_.1");
        assert_eq!(ti.files().file_path(2, ""), "test/stress_test2");

        let mut ti = load("v2_multiple_files.torrent");
        assert!(ti.v2_piece_hashes_verified());
        assert_eq!(ti.num_files(), 5);
        ti.free_piece_layers();
        assert!(ti.v2() && !ti.v2_piece_hashes_verified());

        for name in [
            "v2_hybrid.torrent",
            "empty-files-1.torrent",
            "empty-files-2.torrent",
            "empty-files-3.torrent",
            "empty-files-4.torrent",
            "empty-files-5.torrent",
        ] {
            let ti = load(name);
            assert!(ti.v1() && ti.v2(), "{name}");
        }
        for name in ["v2_no_piece_layers.torrent", "v2_incomplete_piece_layer.torrent"] {
            let ti = load(name);
            assert!(!ti.v1() && ti.v2(), "{name}");
        }
    }

    #[test]
    fn parse_invalid_torrents() {
        use MetainfoError::*;
        let cases: &[(&str, MetainfoError)] = &[
            ("missing_piece_len.torrent", TorrentMissingPieceLength),
            ("invalid_piece_len.torrent", TorrentMissingPieceLength),
            ("negative_piece_len.torrent", TorrentMissingPieceLength),
            ("no_name.torrent", TorrentMissingName),
            ("bad_name.torrent", TorrentMissingName),
            ("invalid_name.torrent", TorrentMissingName),
            ("invalid_info.torrent", TorrentMissingInfo),
            ("string.torrent", TorrentIsNoDict),
            ("negative_size.torrent", TorrentInvalidLength),
            ("negative_file_size.torrent", TorrentInvalidLength),
            ("invalid_path_list.torrent", TorrentInvalidName),
            ("missing_path_list.torrent", TorrentMissingName),
            ("invalid_pieces.torrent", TorrentMissingPieces),
            ("unaligned_pieces.torrent", TorrentInvalidHashes),
            ("invalid_file_size.torrent", TorrentInvalidLength),
            ("invalid_symlink.torrent", TorrentInvalidName),
            ("many_pieces.torrent", TooManyPiecesInTorrent),
            ("no_files.torrent", NoFilesInTorrent),
            ("zero.torrent", TorrentInvalidLength),
            ("zero2.torrent", TorrentInvalidLength),
            ("v2_mismatching_metadata.torrent", TorrentInconsistentFiles),
            ("v2_no_power2_piece.torrent", TorrentMissingPieceLength),
            ("v2_invalid_file.torrent", TorrentFileParseFailed),
            ("v2_deep_recursion.torrent", Bdecode(BdecodeErrorKind::DepthExceeded)),
            ("v2_non_multiple_piece_layer.torrent", TorrentInvalidPieceLayer),
            ("v2_piece_layer_invalid_file_hash.torrent", TorrentInvalidPieceLayer),
            ("v2_invalid_piece_layer.torrent", TorrentInvalidPieceLayer),
            ("v2_invalid_piece_layer_root.torrent", TorrentInvalidPieceLayer),
            ("v2_unknown_piece_layer_entry.torrent", TorrentInvalidPieceLayer),
            ("v2_invalid_piece_layer_size.torrent", TorrentInvalidPieceLayer),
            ("v2_bad_file_alignment.torrent", TorrentInconsistentFiles),
            ("v2_unordered_files.torrent", InvalidBencoding),
            ("v2_overlong_integer.torrent", InvalidBencoding),
            ("v2_missing_file_root_invalid_symlink.torrent", TorrentMissingPiecesRoot),
            ("v2_large_file.torrent", TorrentInvalidLength),
            ("v2_large_offset.torrent", TooManyPiecesInTorrent),
            ("v2_piece_size.torrent", TorrentMissingPieceLength),
            ("v2_invalid_pad_file.torrent", TorrentInvalidPadFile),
            ("v2_zero_root.torrent", TorrentMissingPiecesRoot),
            ("v2_zero_root_small.torrent", TorrentMissingPiecesRoot),
            ("v2_empty_filename.torrent", TorrentFileParseFailed),
        ];
        let mut failures = Vec::new();
        for &(name, expected) in cases {
            match TorrentInfo::from_bytes(&fixture(name)) {
                Err(e) if e == expected => {}
                // like libtorrent: some piece-layer problems only surface when hashes are checked
                Ok(_) if expected == TorrentInvalidPieceLayer => {}
                other => {
                    failures.push(format!("{name}: expected {expected:?}, got {:?}", other.map(|_| "Ok")))
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// The whole fixture directory loads without panicking, valid or not.
    #[test]
    fn all_fixtures_without_panics() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/torrents");
        let mut n = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let buf = std::fs::read(entry.unwrap().path()).unwrap();
            let _ = TorrentInfo::from_bytes(&buf);
            n += 1;
        }
        assert!(n >= 100);
    }

    fn single_file_torrent(extra: impl FnOnce(&mut Entry, &mut Entry)) -> Vec<u8> {
        let mut info = Entry::new_dict();
        *info.key("pieces") = "aaaaaaaaaaaaaaaaaaaa".into();
        *info.key("name.utf-8") = "test1".into();
        *info.key("name") = "test__".into();
        *info.key("piece length") = (16 * 1024).into();
        *info.key("length") = 3245.into();
        let mut torrent = Entry::new_dict();
        extra(&mut torrent, &mut info);
        *torrent.key("info") = info;
        torrent.bencode()
    }

    #[test]
    fn parse_torrents_names() {
        let ti = TorrentInfo::from_bytes(&single_file_torrent(|_, _| {})).unwrap();
        assert_eq!(ti.name(), "test1");

        let abs = if cfg!(windows) { "c:/test1/test2/test3" } else { "/test1/test2/test3" };
        let ti =
            TorrentInfo::from_bytes(&single_file_torrent(|_, i| *i.key("name.utf-8") = abs.into())).unwrap();
        assert_eq!(ti.name(), if cfg!(windows) { "c_test1test2test3" } else { "test1test2test3" });

        let ti = TorrentInfo::from_bytes(&single_file_torrent(|_, i| {
            *i.key("name.utf-8") = "test2/../test3/.././../../test4".into()
        }))
        .unwrap();
        assert_eq!(ti.name(), "test2..test3.......test4");
    }

    #[test]
    fn url_list_and_httpseeds() {
        let buf = single_file_torrent(|t, _| {
            let l = Entry::from(vec![
                Entry::from("http://foo.com/bar1"),
                Entry::from("http://foo.com/bar1"),
                Entry::from("http://foo.com/bar2"),
            ]);
            *t.key("url-list") = l.clone();
            *t.key("httpseeds") = l;
        });
        assert_eq!(TorrentInfo::from_bytes(&buf).unwrap().web_seeds().len(), 4);
    }

    #[test]
    fn trackers_and_web_seeds_api() {
        let mut ti = TorrentInfo::from_info_hashes(InfoHashes::v1(Sha1Hash(*b"                    ")));
        assert!(ti.trackers().is_empty());
        ti.add_tracker("http://test.com/announce", 0, TrackerSource::Client);
        ti.add_tracker("http://test.com/announce", 0, TrackerSource::Client);
        assert_eq!(ti.trackers().len(), 1);
        assert_eq!(ti.trackers()[0].url, "http://test.com/announce");
        ti.clear_trackers();
        assert!(ti.trackers().is_empty());

        ti.add_url_seed("http://test.com");
        assert_eq!(ti.web_seeds(), [WebSeed::new("http://test.com", WebSeedKind::UrlSeed)]);
        ti.add_http_seed("http://test.com");
        assert_eq!(ti.web_seeds()[1].kind, WebSeedKind::HttpSeed);
        let seeds = vec![
            WebSeed::new("http://test1.com", WebSeedKind::UrlSeed),
            WebSeed::new("http://test2com", WebSeedKind::HttpSeed),
        ];
        ti.set_web_seeds(seeds.clone());
        assert_eq!(ti.web_seeds(), seeds.as_slice());
    }

    /// Found by tests/robustness.rs: a hybrid needs v1 piece hashes (diverges from libtorrent,
    /// which accepts it and later fails every v1 hash check).
    #[test]
    fn hybrid_without_v1_pieces_is_rejected() {
        let build = |with_pieces: bool| {
            let mut leaf = Entry::new_dict();
            *leaf.key("length") = 100.into();
            *leaf.key("pieces root") = Entry::from(vec![7u8; 32]);
            let mut file = Entry::new_dict();
            *file.key("") = leaf;
            let mut tree = Entry::new_dict();
            *tree.key("x") = file;
            let mut info = Entry::new_dict();
            *info.key("file tree") = tree;
            *info.key("length") = 100.into();
            *info.key("meta version") = 2.into();
            *info.key("name") = "x".into();
            *info.key("piece length") = 16384.into();
            if with_pieces {
                *info.key("pieces") = Entry::from(vec![1u8; 20]);
            }
            let mut t = Entry::new_dict();
            *t.key("info") = info;
            TorrentInfo::from_bytes(&t.bencode())
        };
        assert_eq!(build(false).unwrap_err(), MetainfoError::TorrentMissingPieces);
        let ti = build(true).unwrap();
        assert!(ti.v1() && ti.v2());
        assert!(ti.hash_for_piece(0).is_some());
    }

    /// SEC-09: a v2 torrent whose Merkle trees would take more than MAX_MERKLE_TREE_BYTES once
    /// expanded (the first downloaded block of a file expands its tree) is refused.
    #[test]
    fn v2_trees_too_large_for_memory_are_refused() {
        const GIB: i64 = 1 << 30;
        let build = |sizes: &[i64]| {
            let mut tree = Entry::new_dict();
            for (i, size) in sizes.iter().enumerate() {
                let mut leaf = Entry::new_dict();
                *leaf.key("length") = (*size).into();
                *leaf.key("pieces root") = Entry::from(vec![7u8; 32]);
                let mut file = Entry::new_dict();
                *file.key("") = leaf;
                *tree.key(format!("f{i}")) = file;
            }
            let mut info = Entry::new_dict();
            *info.key("file tree") = tree;
            *info.key("meta version") = 2.into();
            *info.key("name") = "x".into();
            *info.key("piece length") = (256 << 20).into();
            TorrentInfo::from_info_section(&info.bencode(), LoadLimits::default())
        };
        // the finding: one file of about 16 TiB (the largest allowed), a 64 GiB tree
        assert_eq!(build(&[(16 << 40) - (1 << 20)]).unwrap_err(), MetainfoError::TooManyPiecesInTorrent);
        // 512 GiB is 2^25 blocks: 2^26 - 1 nodes, just under 2 GiB
        assert!(build(&[512 * GIB]).is_ok());
        assert_eq!(build(&[512 * GIB + 1]).unwrap_err(), MetainfoError::TooManyPiecesInTorrent);
        // the limit is for the whole torrent
        assert!(build(&[100 * GIB, 100 * GIB]).is_ok());
        assert_eq!(build(&[300 * GIB, 300 * GIB]).unwrap_err(), MetainfoError::TooManyPiecesInTorrent);
        assert_eq!(merkle_tree_bytes(build(&[16384]).unwrap().files()), 32);
        assert_eq!(merkle_tree_bytes(build(&[3 * 16384]).unwrap().files()), 7 * 32);
    }

    /// A v1 torrent with one file `depth` path elements deep (libtorrent 2.1.0
    /// make_v1_torrent_with_path_depth); `symlink` makes it a symlink with a target as deep.
    fn torrent_with_path_depth(depth: usize, symlink: bool) -> Vec<u8> {
        let mut path: Vec<Entry> = (1..depth).map(|_| Entry::from("d")).collect();
        path.push(Entry::from("file.txt"));
        let mut file = Entry::new_dict();
        *file.key("length") = 1.into();
        *file.key("path") = Entry::from(path.clone());
        let mut files = vec![file];
        if symlink {
            let mut link = Entry::new_dict();
            *link.key("attr") = "l".into();
            *link.key("path") = Entry::from(vec![Entry::from("link")]);
            *link.key("symlink path") = Entry::from(path);
            // the file itself is not too deep
            *files[0].key("path") = Entry::from(vec![Entry::from("file.txt")]);
            files.push(link);
        }
        let mut info = Entry::new_dict();
        *info.key("name") = "root".into();
        *info.key("piece length") = (16 * 1024).into();
        *info.key("pieces") = "aaaaaaaaaaaaaaaaaaaa".into();
        *info.key("files") = Entry::from(files);
        let mut t = Entry::new_dict();
        *t.key("info") = info;
        t.bencode()
    }

    /// libtorrent 2.1.0 load_torrent_directory_depth_v1_*.
    #[test]
    fn directory_depth_limit() {
        let small = LoadLimits { max_directory_depth: 5, ..LoadLimits::default() };
        for symlink in [false, true] {
            TorrentInfo::from_bytes(&torrent_with_path_depth(50, symlink)).unwrap();
            TorrentInfo::from_bytes(&torrent_with_path_depth(100, symlink)).unwrap();
            assert_eq!(
                TorrentInfo::from_bytes(&torrent_with_path_depth(200, symlink)).unwrap_err(),
                MetainfoError::TorrentDirectoryTooDeep
            );
            assert_eq!(
                TorrentInfo::from_bytes_with_limits(&torrent_with_path_depth(20, symlink), small)
                    .unwrap_err(),
                MetainfoError::TorrentDirectoryTooDeep
            );
            assert!(TorrentInfo::from_bytes_with_limits(&torrent_with_path_depth(5, symlink), small).is_ok());
        }

        // the v2 file tree
        let mut tree = Entry::new_dict();
        *tree.key("") = {
            let mut f = Entry::new_dict();
            *f.key("length") = 16384.into();
            *f.key("pieces root") = Entry::from(vec![1u8; 32]);
            f
        };
        for i in 0..10 {
            let mut parent = Entry::new_dict();
            *parent.key(format!("d{i}")) = tree;
            tree = parent;
        }
        let mut info = Entry::new_dict();
        *info.key("file tree") = tree;
        *info.key("meta version") = 2.into();
        *info.key("name") = "root".into();
        *info.key("piece length") = (16 * 1024).into();
        let info = info.bencode();
        assert!(TorrentInfo::from_info_section(&info, LoadLimits::default()).is_ok());
        assert_eq!(
            TorrentInfo::from_info_section(&info, small).unwrap_err(),
            MetainfoError::TorrentDirectoryTooDeep
        );
    }

    /// libtorrent 2.0.13 test_http_parser (is_valid_tracker_url, and the hostname cases
    /// parse_url_components rejects there).
    #[test]
    fn valid_tracker_urls() {
        for url in [
            "http://example.com/announce",
            "https://example.com/announce",
            "udp://example.com:1337",
            "http://192.168.0.1:8080/announce",
            "http://[2001:ff00::1]:42/announce",
            "HTTP://EXAMPLE.COM/announce",
            "http://[fe80::1%25eth0]:42/path",
            "http://user:pass@example.com/announce",
        ] {
            assert!(is_valid_tracker_url(url), "{url}");
        }
        for url in [
            "",
            "ftp://example.com/",
            "example.com/announce",
            "http://example.com/\r\nX-evil: 1",
            "http://example.com/\n",
            "http://example.com/\t",
            "http://exa mple.com/announce",
            "http://example.com/path with space",
            "http://example.com/announce key=value",
            " http://example.com/announce",
            "http://example.com/announce ",
            "http://",
            "https://",
            "udp://",
            "http:///path",
            "http://:8080/path",
            "http://[]/path",
            "http://under_score.example/announce",
            "http://test\r\n.com/path",
            "http://test.com:42abc",
            "http://[::1/announce",
        ] {
            assert!(!is_valid_tracker_url(url), "{url:?}");
        }
    }

    /// libtorrent 2.0.13 add_tracker_reject_invalid_url.
    #[test]
    fn add_tracker_reject_invalid_url() {
        let mut ti = TorrentInfo::from_info_hashes(InfoHashes::v1(Sha1Hash(*b"                    ")));
        assert_eq!(ti.trackers().len(), 0);
        let cases = [
            ("http://test.com/announce", 1),
            // random text is invalid
            ("invalid url", 1),
            ("https://test.com/announce", 2),
            // ftp scheme is invalid
            ("ftp://test.com", 2),
            ("udp://test.com/announce", 3),
            ("httpfoo://test.com/announce", 3),
            ("http://foo.com/announce", 4),
        ];
        for (url, n) in cases {
            ti.add_tracker(url, 0, TrackerSource::Client);
            assert_eq!(ti.trackers().len(), n, "{url}");
        }
        ti.clear_trackers();
        assert_eq!(ti.trackers().len(), 0);
    }

    /// Invalid announce and announce-list URLs in a .torrent are dropped.
    #[test]
    fn invalid_tracker_urls_in_metadata() {
        let ti = TorrentInfo::from_bytes(&single_file_torrent(|t, _| {
            *t.key("announce") = "ftp://tracker.example/announce".into();
        }))
        .unwrap();
        assert!(ti.trackers().is_empty());
        let ti = TorrentInfo::from_bytes(&single_file_torrent(|t, _| {
            let tier = vec![Entry::from("http://a_b/announce"), Entry::from("udp://tracker.example:80")];
            *t.key("announce-list") = Entry::from(vec![Entry::from(tier)]);
        }))
        .unwrap();
        assert_eq!(ti.trackers().len(), 1);
        assert_eq!(ti.trackers()[0].url, "udp://tracker.example:80");
    }

    #[test]
    fn empty_buffer_fails() {
        assert!(TorrentInfo::from_bytes(b"").is_err());
    }

    #[test]
    fn copy_keeps_borrowed_names() {
        let a = load("sample.torrent");
        let expected = ["sample/text_file2.txt", "sample/.____padding_file/0", "sample/text_file.txt"];
        let hashes = [Sha1Hash::default(), Sha1Hash::default(), Sha1Hash(*b"abababababababababab")];
        for i in 0..3 {
            assert_eq!(a.files().file_path(i, ""), expected[i]);
            assert_eq!(a.files().hash(i), hashes[i]);
        }
        let b = a.clone();
        drop(a);
        assert_eq!(b.num_files(), 3);
        for i in 0..3 {
            assert_eq!(b.files().file_path(i, ""), expected[i]);
            assert_eq!(b.files().hash(i), hashes[i]);
        }
    }

    /// libtorrent's resolve_duplicates cases. The torrents are built by hand here (libtorrent
    /// uses create_torrent, which is ported in Phase 5).
    #[test]
    fn resolve_duplicates() {
        type Case = &'static [(&'static str, u64, bool, &'static str)];
        let cases: &[Case] = &[
            &[
                ("test/temporary.txt", 0x4000, false, "test/temporary.txt"),
                ("test/Temporary.txt", 0x4000, false, "test/Temporary.1.txt"),
                ("test/TeMPorArY.txT", 0x4000, false, "test/TeMPorArY.2.txT"),
                ("test/test/TEMPORARY.TXT", 0x4000, false, "test/test/TEMPORARY.TXT"),
            ],
            &[
                ("test/b.exe", 0x4000, false, "test/b.exe"),
                ("test/B.ExE", 0x4000, false, "test/B.1.ExE"),
                ("test/B.exe", 0x4000, false, "test/B.2.exe"),
                ("test/filler", 0x4000, false, "test/filler"),
            ],
            &[
                ("test/a/b/c/d/e/f/g/h/i/j/k/l/m", 0x4000, false, "test/a/b/c/d/e/f/g/h/i/j/k/l/m"),
                ("test/a", 0x4000, false, "test/a.1"),
                ("test/a/b", 0x4000, false, "test/a/b.1"),
                ("test/a/b/c", 0x4000, false, "test/a/b/c.1"),
                ("test/a/b/c/d", 0x4000, false, "test/a/b/c/d.1"),
                ("test/a/b/c/d/e", 0x4000, false, "test/a/b/c/d/e.1"),
                ("test/a/b/c/d/e/f", 0x4000, false, "test/a/b/c/d/e/f.1"),
                ("test/a/b/c/d/e/f/g", 0x4000, false, "test/a/b/c/d/e/f/g.1"),
                ("test/a/b/c/d/e/f/g/h", 0x4000, false, "test/a/b/c/d/e/f/g/h.1"),
                ("test/a/b/c/d/e/f/g/h/i", 0x4000, false, "test/a/b/c/d/e/f/g/h/i.1"),
                ("test/a/b/c/d/e/f/g/h/i/j", 0x4000, false, "test/a/b/c/d/e/f/g/h/i/j.1"),
            ],
            &[("test/a", 0x4000, false, "test/a.1"), ("test/a/b", 0x4000, false, "test/a/b")],
            &[
                ("test/A/tmp", 0x4000, false, "test/A/tmp"),
                ("test/a", 0x4000, false, "test/a.1"),
                ("test/A", 0x4000, false, "test/A.2"),
                ("test/filler", 0x4000, false, "test/filler"),
            ],
            &[
                ("test/long/path/name/that/collides", 0x4000, false, "test/long/path/name/that/collides"),
                ("test/long/path", 0x4000, false, "test/long/path.1"),
                ("test/filler-1", 0x4000, false, "test/filler-1"),
                ("test/filler-2", 0x4000, false, "test/filler-2"),
            ],
            &[
                ("test/.pad/1234", 0x4000, true, "test/.pad/1234"),
                ("test/filler-1", 0x4000, false, "test/filler-1"),
                ("test/.pad/1234", 0x4000, true, "test/.pad/1234"),
                ("test/filler-2", 0x4000, false, "test/filler-2"),
            ],
            &[
                ("test/.pad/1234", 0x8000, true, "test/.pad/1234"),
                ("test/filler-1", 0x4000, false, "test/filler-1"),
                ("test/.pad/1234", 0x4000, true, "test/.pad/1234.1"),
                ("test/filler-2", 0x4000, false, "test/filler-2"),
            ],
            &[
                ("test/.pad/1234", 0x4000, false, "test/.pad/1234"),
                ("test/filler-1", 0x4000, false, "test/filler-1"),
                ("test/.pad/1234", 0x4000, true, "test/.pad/1234.1"),
                ("test/filler-2", 0x4000, false, "test/filler-2"),
            ],
            &[
                ("test/.pad/1234", 0x4000, true, "test/.pad/1234"),
                ("test/filler-1", 0x4000, false, "test/filler-1"),
                ("test/.pad/1234", 0x4000, false, "test/.pad/1234.1"),
                ("test/filler-2", 0x4000, false, "test/filler-2"),
            ],
            &[
                ("test/.pad/1234", 0x4000, true, "test/.pad/1234.1"),
                ("test/filler-1", 0x4000, false, "test/filler-1"),
                ("test/.pad/1234/filler-2", 0x4000, false, "test/.pad/1234/filler-2"),
            ],
        ];
        for case in cases {
            let total: u64 = case.iter().map(|c| c.1).sum();
            let mut files = Vec::new();
            for &(path, size, pad, _) in case.iter() {
                let mut f = Entry::new_dict();
                *f.key("length") = (size as i64).into();
                *f.key("path") = Entry::from(path.split('/').skip(1).map(Entry::from).collect::<Vec<_>>());
                if pad {
                    *f.key("attr") = "p".into();
                }
                files.push(f);
            }
            let mut info = Entry::new_dict();
            *info.key("files") = Entry::from(files);
            *info.key("name") = "test".into();
            *info.key("piece length") = 0x4000.into();
            *info.key("pieces") = Entry::from(vec![0xffu8; (total / 0x4000) as usize * 20]);
            let mut t = Entry::new_dict();
            *t.key("info") = info;
            let ti = TorrentInfo::from_bytes(&t.bencode()).unwrap();
            for (i, &(_, _, _, expected)) in case.iter().enumerate() {
                assert_eq!(ti.files().file_path(i, ""), expected, "case {:?}", case[0].0);
            }
        }
    }
}
