// Ported from libtorrent-rasterbar 2.0.11: src/create_torrent.cpp, include/libtorrent/create_torrent.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: v1 only (BEP 52 v2 / hybrid creation returns `V2Unsupported`); errors are `Result`s
// instead of exceptions; `set_piece_hashes` reads the files itself (one reader, a few hashing
// threads) instead of going through a disk_interface; directory entries are added in name order;
// an optional info["source"] (private-tracker convention) is supported.

//! Creating `.torrent` files (libtorrent `create_torrent`, `add_files`, `set_piece_hashes`).
//!
//! ```no_run
//! # use patorrent_core::metainfo::create_torrent::*;
//! # use patorrent_core::metainfo::FileStorage;
//! let path = std::path::Path::new("some/dir");
//! let mut fs = FileStorage::new();
//! add_files(&mut fs, path, &mut |_| true, CreateFlags::empty()).unwrap();
//! let mut ct = CreateTorrent::new(fs, 0, CreateFlags::empty()).unwrap();
//! ct.add_tracker("udp://tracker.example:1337/announce", 0);
//! set_piece_hashes(&mut ct, &parent_dir(path).unwrap(), &mut |_| true).unwrap();
//! let bytes = ct.generate_buf().unwrap();
//! ```

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use super::file_storage::{DEFAULT_BLOCK_SIZE, FileFlags, FileStorage, FileStorageError, calc_num_pieces};
use super::torrent_info::{TorrentInfo, WebSeedKind};
use crate::bencode::Entry;
use crate::crypto::sha1::Sha1;
use crate::flags::bit_flags;
use crate::types::Sha1Hash;
use crate::util::path::{combine_path, has_parent_path, lsplit_path};

bit_flags! {
    /// libtorrent `create_flags_t`. Torrents are always v1 (`v1_only`).
    pub struct CreateFlags: u8 {
        /// Include file modification times (`mtime`).
        const MODIFICATION_TIME = 0;
        /// Store symlinks as symlinks (BEP 47) instead of following them. Unix only.
        const SYMLINKS = 1;
        /// Request a v2-only torrent. Not supported yet: [`CreateTorrent::new`] fails.
        const V2_ONLY = 2;
        /// Sort files and pad each one to a piece boundary, including the last.
        const CANONICAL_FILES = 3;
        /// Do not record hidden / executable / symlink attributes.
        const NO_ATTRIBUTES = 4;
        /// Like `CANONICAL_FILES`, but without a pad file after the last file (and padding goes
        /// before each file, as in libtorrent before 2.0).
        const CANONICAL_FILES_NO_TAIL_PADDING = 5;
        /// Allow piece sizes that are neither a multiple of 16 kiB nor a power of two.
        const ALLOW_ODD_PIECE_SIZE = 6;
    }
}

/// libtorrent's `errors::` codes for torrent creation, plus I/O failures while hashing.
#[derive(Debug)]
pub enum CreateTorrentError {
    /// errors::invalid_piece_size
    InvalidPieceSize,
    /// errors::no_files_in_torrent
    NoFilesInTorrent,
    /// errors::torrent_invalid_length: the files are all empty.
    TorrentInvalidLength,
    /// errors::invalid_hash_entry: piece hashes are missing.
    InvalidHashEntry,
    /// errors::torrent_missing_file_tree: there are no files (or they are all empty).
    TorrentMissingFileTree,
    /// BEP 52 (v2 and hybrid) torrents cannot be created yet.
    V2Unsupported,
    /// The progress callback asked to stop.
    Aborted,
    /// A file name is not valid UTF-8.
    NonUtf8Path(PathBuf),
    /// A file could not be added to the file list (name too long, file too large).
    FileStorage(FileStorageError),
    /// Reading a file failed (or it is shorter than when it was added).
    Io(PathBuf, io::Error),
}

impl fmt::Display for CreateTorrentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use CreateTorrentError::*;
        match self {
            InvalidPieceSize => f.write_str("invalid piece size in torrent file"),
            NoFilesInTorrent => f.write_str("no files in torrent"),
            TorrentInvalidLength => f.write_str("invalid length of torrent"),
            InvalidHashEntry => f.write_str("invalid hash entry"),
            TorrentMissingFileTree => f.write_str("the v2 torrent file has no file tree"),
            V2Unsupported => f.write_str("creating v2 or hybrid torrents is not supported yet"),
            Aborted => f.write_str("operation aborted"),
            NonUtf8Path(p) => write!(f, "{}: file name is not valid UTF-8", p.display()),
            FileStorage(e) => match e {
                FileStorageError::FileTooLarge => f.write_str("File too large"),
                FileStorageError::FilenameTooLong => f.write_str("Filename too long"),
                FileStorageError::InvalidLength => f.write_str("invalid length of torrent"),
                FileStorageError::MissingPiecesRoot => f.write_str("a v2 file entry has no root hash"),
                FileStorageError::InconsistentFiles => {
                    f.write_str("the v1 and v2 file metadata does not match")
                }
            },
            Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for CreateTorrentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CreateTorrentError::Io(_, e) => Some(e),
            _ => None,
        }
    }
}

impl From<FileStorageError> for CreateTorrentError {
    fn from(e: FileStorageError) -> CreateTorrentError {
        CreateTorrentError::FileStorage(e)
    }
}

/// Largest piece size libtorrent writes; some clients don't support larger pieces.
pub const MAX_CREATE_PIECE_SIZE: u32 = 128 * 1024 * 1024;

/// libtorrent's automatic piece size: the smallest power of two from 16 kiB up such that the
/// piece list stays around `2 * sqrt(total_size)` bytes, capped at 16 MiB.
pub fn auto_piece_size(total_size: u64) -> u32 {
    // size_table is computed from the following:
    //   target_list_size = sqrt(total_size) * 2;
    //   target_piece_size = total_size / (target_list_size / hash_size);
    // Given hash_size = 20 bytes, target_piece_size = (16*1024 * pow(2, i))
    // we can determine size_table = (total_size = pow(2 * target_piece_size / hash_size, 2))
    const SIZE_TABLE: [u64; 10] = [
        2_684_355,       // ->  16 kiB
        10_737_418,      // ->  32 kiB
        42_949_673,      // ->  64 kiB
        171_798_692,     // -> 128 kiB
        687_194_767,     // -> 256 kiB
        2_748_779_069,   // -> 512 kiB
        10_995_116_278,  // ->   1 MiB
        43_980_465_111,  // ->   2 MiB
        175_921_860_444, // ->   4 MiB
        703_687_441_777, // ->   8 MiB
    ];
    let i = SIZE_TABLE.iter().take_while(|&&s| s < total_size).count();
    DEFAULT_BLOCK_SIZE << i
}

/// A torrent under construction (libtorrent `create_torrent`).
#[derive(Clone, Debug)]
pub struct CreateTorrent {
    files: FileStorage,
    /// One per piece; zero = not set yet.
    piece_hash: Vec<Sha1Hash>,
    /// (url, tier), kept sorted by tier.
    urls: Vec<(String, u32)>,
    url_seeds: Vec<String>,
    http_seeds: Vec<String>,
    nodes: Vec<(String, u16)>,
    similar: Vec<Sha1Hash>,
    collections: Vec<String>,
    /// The info dictionary verbatim, when built from an existing torrent.
    info_dict: Option<Vec<u8>>,
    comment: String,
    created_by: String,
    source: String,
    root_cert: Vec<u8>,
    /// 0 = don't write a creation date.
    creation_date: i64,
    multifile: bool,
    private: bool,
    include_mtime: bool,
    include_symlinks: bool,
}

impl CreateTorrent {
    /// libtorrent `create_torrent(file_storage&, piece_size, flags)`. A `piece_size` of 0 picks
    /// one automatically ([`auto_piece_size`]). The files are laid out on pieces here: sorted and
    /// padded with the `CANONICAL_FILES*` flags, in the given order otherwise.
    pub fn new(
        mut fs: FileStorage,
        piece_size: u32,
        flags: CreateFlags,
    ) -> Result<CreateTorrent, CreateTorrentError> {
        if flags.contains(CreateFlags::V2_ONLY) {
            return Err(CreateTorrentError::V2Unsupported);
        }
        let mut ct = CreateTorrent {
            multifile: fs.num_files() > 1,
            files: FileStorage::new(),
            piece_hash: Vec::new(),
            urls: Vec::new(),
            url_seeds: Vec::new(),
            http_seeds: Vec::new(),
            nodes: Vec::new(),
            similar: Vec::new(),
            collections: Vec::new(),
            info_dict: None,
            comment: String::new(),
            created_by: String::new(),
            source: String::new(),
            root_cert: Vec::new(),
            creation_date: crate::util::stat::now_unix(),
            private: false,
            include_mtime: flags.contains(CreateFlags::MODIFICATION_TIME),
            include_symlinks: flags.contains(CreateFlags::SYMLINKS),
        };

        // return instead of failing; generate() reports the missing files
        if fs.num_files() == 0 || fs.total_size() == 0 {
            ct.files = fs;
            return Ok(ct);
        }

        if !ct.multifile && has_parent_path(&fs.file_path(0, "")) {
            ct.multifile = true;
        }

        let piece_size = if piece_size == 0 { auto_piece_size(fs.total_size()) } else { piece_size };

        // v1 torrents should have piece sizes divisible by 16 kiB
        if piece_size % (16 * 1024) != 0
            && !piece_size.is_power_of_two()
            && !flags.contains(CreateFlags::ALLOW_ODD_PIECE_SIZE)
        {
            return Err(CreateTorrentError::InvalidPieceSize);
        }
        // this is an unreasonably large piece size. Some clients don't support pieces this large.
        if piece_size > MAX_CREATE_PIECE_SIZE {
            return Err(CreateTorrentError::InvalidPieceSize);
        }

        fs.set_piece_length(piece_size);
        let canonical = flags.contains(CreateFlags::CANONICAL_FILES);
        let no_tail = flags.contains(CreateFlags::CANONICAL_FILES_NO_TAIL_PADDING);
        if canonical || no_tail {
            // pad files are whole blocks; libtorrent only canonicalizes v2-capable piece sizes
            if piece_size < 16 * 1024 {
                return Err(CreateTorrentError::InvalidPieceSize);
            }
            fs.canonicalize_impl(no_tail);
        }
        fs.set_num_pieces(calc_num_pieces(&fs));
        ct.files = fs;
        Ok(ct)
    }

    /// libtorrent `create_torrent(torrent_info const&)`: a copy of an existing v1 torrent whose
    /// info dictionary (and so its info-hash) is preserved verbatim.
    pub fn from_torrent_info(ti: &TorrentInfo) -> Result<CreateTorrent, CreateTorrentError> {
        if ti.v2() {
            return Err(CreateTorrentError::V2Unsupported);
        }
        if !ti.is_valid() || !ti.v1() {
            return Err(CreateTorrentError::InvalidHashEntry);
        }
        let mut ct = CreateTorrent::new(FileStorage::new(), 0, CreateFlags::empty())?;
        ct.files = ti.files().clone();
        ct.multifile = ti.num_files() > 1;
        ct.private = ti.is_private();
        if let Some(d) = ti.creation_date().filter(|&d| d > 0) {
            ct.creation_date = d;
        }
        ct.created_by = ti.creator().to_owned();
        ct.comment = ti.comment().to_owned();
        for n in ti.nodes() {
            ct.add_node(n.0.clone(), n.1);
        }
        for t in ti.trackers() {
            ct.add_tracker(&t.url, t.tier as u32);
        }
        for s in ti.web_seeds() {
            match s.kind {
                WebSeedKind::UrlSeed => ct.add_url_seed(&s.url),
                WebSeedKind::HttpSeed => ct.add_http_seed(&s.url),
            }
        }
        for i in 0..ti.num_pieces() {
            let h = ti.hash_for_piece(i).ok_or(CreateTorrentError::InvalidHashEntry)?;
            ct.set_hash(i, h);
        }
        ct.info_dict = Some(ti.info_section().to_vec());
        Ok(ct)
    }

    pub fn files(&self) -> &FileStorage {
        &self.files
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

    /// Adds a tracker in `tier` (0 = first). Duplicate URLs and empty strings are ignored.
    pub fn add_tracker(&mut self, url: &str, tier: u32) {
        if url.is_empty() || self.urls.iter().any(|(u, _)| u == url) {
            return;
        }
        self.urls.push((url.to_owned(), tier));
        self.urls.sort_by_key(|&(_, t)| t);
    }

    /// The trackers as (url, tier), sorted by tier.
    pub fn trackers(&self) -> &[(String, u32)] {
        &self.urls
    }

    /// A BEP 19 web seed ("url-list").
    pub fn add_url_seed(&mut self, url: &str) {
        self.url_seeds.push(url.to_owned());
    }

    /// A BEP 17 HTTP seed ("httpseeds").
    pub fn add_http_seed(&mut self, url: &str) {
        self.http_seeds.push(url.to_owned());
    }

    /// A DHT bootstrap node ("nodes").
    pub fn add_node(&mut self, host: impl Into<String>, port: u16) {
        self.nodes.push((host.into(), port));
    }

    pub fn add_similar_torrent(&mut self, ih: Sha1Hash) {
        self.similar.push(ih);
    }

    pub fn add_collection(&mut self, c: &str) {
        self.collections.push(c.to_owned());
    }

    pub fn set_comment(&mut self, s: &str) {
        self.comment = s.to_owned();
    }

    pub fn set_creator(&mut self, s: &str) {
        self.created_by = s.to_owned();
    }

    /// Seconds since the epoch; 0 leaves "creation date" out. Defaults to now.
    pub fn set_creation_date(&mut self, timestamp: i64) {
        self.creation_date = timestamp;
    }

    /// info["source"]: private trackers use it to give a cross-seeded torrent its own info-hash.
    pub fn set_source(&mut self, s: &str) {
        self.source = s.to_owned();
    }

    pub fn set_root_cert(&mut self, cert: &[u8]) {
        self.root_cert = cert.to_vec();
    }

    pub fn set_priv(&mut self, p: bool) {
        self.private = p;
    }

    pub fn priv_(&self) -> bool {
        self.private
    }

    /// Sets the SHA-1 hash of piece `index`. Panics if `index` is out of range.
    pub fn set_hash(&mut self, index: u32, h: Sha1Hash) {
        if self.piece_hash.is_empty() {
            self.piece_hash.resize(self.files.num_pieces() as usize, Sha1Hash::default());
        }
        self.piece_hash[index as usize] = h;
    }

    /// libtorrent `set_hash2`: v2 piece hashes. Always fails until v2 creation is ported.
    pub fn set_hash2(
        &mut self,
        _file: usize,
        _piece: u32,
        _h: crate::types::Sha256Hash,
    ) -> Result<(), CreateTorrentError> {
        Err(CreateTorrentError::InvalidHashEntry)
    }

    fn v1_hashes_valid(&self) -> bool {
        let pl = self.files.piece_length() as u64;
        pl > 0
            && self.piece_hash.len() as u64 == self.files.total_size().div_ceil(pl)
            && !self.piece_hash.iter().any(Sha1Hash::is_zero)
    }

    /// Builds the torrent (libtorrent `generate`).
    pub fn generate(&self) -> Result<Entry, CreateTorrentError> {
        if self.files.num_files() == 0 || self.files.total_size() == 0 {
            return Err(CreateTorrentError::TorrentMissingFileTree);
        }
        // if the v1 hashes were not all set, we can't create a torrent
        if !self.v1_hashes_valid() {
            return Err(CreateTorrentError::InvalidHashEntry);
        }

        let mut dict = Entry::new_dict();
        if let Some((url, _)) = self.urls.first() {
            *dict.key("announce") = url.as_str().into();
        }
        if !self.nodes.is_empty() {
            let nodes = self
                .nodes
                .iter()
                .map(|(h, p)| Entry::List(vec![h.as_str().into(), Entry::Int(*p as i64)]))
                .collect::<Vec<_>>();
            *dict.key("nodes") = Entry::List(nodes);
        }
        if self.urls.len() > 1 {
            let mut trackers: Vec<Entry> = Vec::new();
            let mut tier: Vec<Entry> = Vec::new();
            let mut current_tier = self.urls[0].1;
            for (url, t) in &self.urls {
                if *t != current_tier {
                    current_tier = *t;
                    trackers.push(Entry::List(std::mem::take(&mut tier)));
                }
                tier.push(url.as_str().into());
            }
            trackers.push(Entry::List(tier));
            *dict.key("announce-list") = Entry::List(trackers);
        }
        if !self.comment.is_empty() {
            *dict.key("comment") = self.comment.as_str().into();
        }
        if self.creation_date != 0 {
            *dict.key("creation date") = Entry::Int(self.creation_date);
        }
        if !self.created_by.is_empty() {
            *dict.key("created by") = self.created_by.as_str().into();
        }
        let seed_list = |seeds: &[String]| match seeds {
            [one] => Entry::from(one.as_str()),
            many => Entry::List(many.iter().map(|s| s.as_str().into()).collect()),
        };
        if !self.url_seeds.is_empty() {
            *dict.key("url-list") = seed_list(&self.url_seeds);
        }
        if !self.http_seeds.is_empty() {
            *dict.key("httpseeds") = seed_list(&self.http_seeds);
        }

        if let Some(info) = &self.info_dict {
            *dict.key("info") = Entry::Preformatted(info.clone());
            return Ok(dict);
        }

        let fs = &self.files;
        let mut info = Entry::new_dict();
        if !self.collections.is_empty() {
            *info.key("collections") =
                Entry::List(self.collections.iter().map(|c| c.as_str().into()).collect());
        }
        if !self.similar.is_empty() {
            *info.key("similar") = Entry::List(self.similar.iter().map(|h| h.0.as_slice().into()).collect());
        }
        *info.key("name") = fs.name().into();
        if !self.root_cert.is_empty() {
            *info.key("ssl-cert") = self.root_cert.as_slice().into();
        }
        if self.private {
            *info.key("private") = Entry::Int(1);
        }
        if !self.source.is_empty() {
            *info.key("source") = self.source.as_str().into();
        }

        if !self.multifile {
            if self.include_mtime {
                *info.key("mtime") = Entry::Int(fs.mtime(0));
            }
            *info.key("length") = Entry::Int(fs.file_size(0) as i64);
            let flags = self.file_flags(0);
            add_file_attrs(&mut info, flags, self.include_symlinks);
            if self.include_symlinks && flags.contains(FileFlags::SYMLINK) {
                add_symlink_path(&mut info, fs.internal_symlink(0));
            }
        } else {
            let mut files = Vec::with_capacity(fs.num_files());
            for i in 0..fs.num_files() {
                let mut file_e = Entry::new_dict();
                if self.include_mtime && fs.mtime(i) != 0 {
                    *file_e.key("mtime") = Entry::Int(fs.mtime(i));
                }
                *file_e.key("length") = Entry::Int(fs.file_size(i) as i64);

                let p = fs.file_path(i, "");
                debug_assert!(has_parent_path(&p));
                // deliberately skip the first path element, since that's the "name" of the
                // torrent already
                let mut path_e = Vec::new();
                let mut elems = lsplit_path(lsplit_path(&p).1);
                while !elems.0.is_empty() {
                    path_e.push(Entry::from(elems.0));
                    elems = lsplit_path(elems.1);
                }
                *file_e.key("path") = Entry::List(path_e);

                let flags = self.file_flags(i);
                add_file_attrs(&mut file_e, flags, self.include_symlinks);
                if self.include_symlinks && flags.contains(FileFlags::SYMLINK) {
                    add_symlink_path(&mut file_e, fs.internal_symlink(i));
                }
                files.push(file_e);
            }
            *info.key("files") = Entry::List(files);
        }

        *info.key("piece length") = Entry::Int(fs.piece_length() as i64);
        let mut pieces = Vec::with_capacity(self.piece_hash.len() * 20);
        for h in &self.piece_hash {
            pieces.extend_from_slice(&h.0);
        }
        *info.key("pieces") = Entry::Bytes(pieces);
        *dict.key("info") = info;
        Ok(dict)
    }

    /// The bencoded torrent (libtorrent `generate_buf`).
    pub fn generate_buf(&self) -> Result<Vec<u8>, CreateTorrentError> {
        self.generate().map(|e| e.bencode())
    }

    /// The file's flags. `FileStorage` keeps the symlink bit only for files that have a target.
    fn file_flags(&self, i: usize) -> FileFlags {
        self.files.file_flags(i)
    }
}

fn add_file_attrs(e: &mut Entry, flags: FileFlags, include_symlinks: bool) {
    if !flags.intersects(FileFlags::PAD_FILE | FileFlags::HIDDEN | FileFlags::EXECUTABLE | FileFlags::SYMLINK)
    {
        return;
    }
    let mut attr = String::new();
    if flags.contains(FileFlags::PAD_FILE) {
        attr.push('p');
    }
    if flags.contains(FileFlags::HIDDEN) {
        attr.push('h');
    }
    if flags.contains(FileFlags::EXECUTABLE) {
        attr.push('x');
    }
    if include_symlinks && flags.contains(FileFlags::SYMLINK) {
        attr.push('l');
    }
    *e.key("attr") = attr.into();
}

fn add_symlink_path(e: &mut Entry, symlink_path: &str) {
    // lexically_relative("", p) is p without a trailing separator
    let link = symlink_path.strip_suffix('/').unwrap_or(symlink_path);
    let mut list = Vec::new();
    let mut elems = lsplit_path(link);
    while !elems.0.is_empty() {
        list.push(Entry::from(elems.0));
        elems = lsplit_path(elems.1);
    }
    *e.key("symlink path") = Entry::List(list);
}

/// The directory to pass to [`set_piece_hashes`] for files added with [`add_files`]`(path)`:
/// the parent of `path` made absolute.
pub fn parent_dir(path: &Path) -> io::Result<PathBuf> {
    let full = complete(path)?;
    Ok(full.parent().map(Path::to_path_buf).unwrap_or(full))
}

fn complete(path: &Path) -> io::Result<PathBuf> {
    let full = std::path::absolute(path)?;
    // "." and ".." have no file name; resolve them
    if full.file_name().is_none() { std::fs::canonicalize(path) } else { Ok(full) }
}

/// libtorrent `add_files`: adds `path` (a file, or a directory recursively) to `fs`. The torrent
/// name is the last element of `path`. `pred` sees each full path first and can skip it (and,
/// for a directory, everything below it). Entries that can't be read are skipped, as in
/// libtorrent; directory entries are added in name order.
pub fn add_files(
    fs: &mut FileStorage,
    path: &Path,
    pred: &mut dyn FnMut(&Path) -> bool,
    flags: CreateFlags,
) -> Result<(), CreateTorrentError> {
    let full = complete(path).map_err(|e| CreateTorrentError::Io(path.to_path_buf(), e))?;
    let parent = full.parent().map(Path::to_path_buf).unwrap_or_default();
    let leaf = full.file_name().ok_or(CreateTorrentError::NoFilesInTorrent)?;
    let leaf = leaf.to_str().ok_or_else(|| CreateTorrentError::NonUtf8Path(full.clone()))?;
    add_files_impl(fs, &parent, leaf, pred, flags)
}

fn add_files_impl(
    fs: &mut FileStorage,
    p: &Path,
    l: &str,
    pred: &mut dyn FnMut(&Path) -> bool,
    flags: CreateFlags,
) -> Result<(), CreateTorrentError> {
    let f = p.join(l);
    if !pred(&f) {
        return Ok(());
    }
    let dont_follow = cfg!(unix) && flags.contains(CreateFlags::SYMLINKS);
    let meta = if dont_follow { std::fs::symlink_metadata(&f) } else { std::fs::metadata(&f) };
    let Ok(s) = meta else { return Ok(()) };

    // if the file is not a link or we're following links, and it's a directory, only then
    // should we recurse
    if s.is_dir() {
        let Ok(dir) = std::fs::read_dir(&f) else { return Ok(()) };
        let mut leaves: Vec<_> = dir.filter_map(Result::ok).map(|e| e.file_name()).collect();
        leaves.sort();
        for leaf in leaves {
            let leaf = leaf.to_str().ok_or_else(|| CreateTorrentError::NonUtf8Path(f.join(&leaf)))?;
            add_files_impl(fs, p, &combine_path(l, leaf), pred, flags)?;
        }
        return Ok(());
    }

    let mtime = s
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let file_flags =
        if flags.contains(CreateFlags::NO_ATTRIBUTES) { FileFlags::empty() } else { get_file_attributes(&f) };
    if file_flags.contains(FileFlags::SYMLINK) && flags.contains(CreateFlags::SYMLINKS) {
        let sym_path = get_symlink_path(&f);
        fs.add_file(l, 0, file_flags, mtime, &sym_path, None)?;
    } else {
        fs.add_file(l, s.len(), file_flags, mtime, "", None)?;
    }
    Ok(())
}

/// libtorrent `aux::get_file_attributes`: hidden on Windows; executable and symlink elsewhere.
pub fn get_file_attributes(p: &Path) -> FileFlags {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        match std::fs::metadata(p) {
            Ok(m) if m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 => FileFlags::HIDDEN,
            _ => FileFlags::empty(),
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(m) = std::fs::symlink_metadata(p) else { return FileFlags::empty() };
        let mut attr = FileFlags::empty();
        if m.permissions().mode() & 0o100 != 0 {
            attr |= FileFlags::EXECUTABLE;
        }
        if m.file_type().is_symlink() {
            attr |= FileFlags::SYMLINK;
        }
        attr
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = p;
        FileFlags::empty()
    }
}

/// libtorrent `aux::get_symlink_path`: the link target as stored (empty on Windows).
fn get_symlink_path(p: &Path) -> String {
    if cfg!(windows) {
        return String::new();
    }
    std::fs::read_link(p).ok().and_then(|t| t.to_str().map(str::to_owned)).unwrap_or_default()
}

/// libtorrent `set_piece_hashes`: reads the files from `base` (the directory that holds the
/// torrent's top-level file or folder) and sets every piece hash. `progress` is called with the
/// number of pieces done so far minus one (libtorrent passes the piece index of each completed
/// piece, in order) and returns false to cancel.
pub fn set_piece_hashes(
    t: &mut CreateTorrent,
    base: &Path,
    progress: &mut dyn FnMut(u32) -> bool,
) -> Result<(), CreateTorrentError> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 4);
    set_piece_hashes_threads(t, base, threads, progress)
}

/// [`set_piece_hashes`] with `threads` hashing threads (libtorrent `hashing_threads`). The
/// calling thread reads the files in order.
pub fn set_piece_hashes_threads(
    t: &mut CreateTorrent,
    base: &Path,
    threads: usize,
    progress: &mut dyn FnMut(u32) -> bool,
) -> Result<(), CreateTorrentError> {
    if t.files.num_files() == 0 {
        return Err(CreateTorrentError::NoFilesInTorrent);
    }
    if t.files.total_size() == 0 {
        return Err(CreateTorrentError::TorrentInvalidLength);
    }
    let num_pieces = t.files.num_pieces();
    let piece_len = t.files.piece_length() as usize;
    // have 4 outstanding hash requests per thread, and no less than 1 MiB, but cap the memory
    let threads = threads.max(1);
    let read_ahead = (threads * 4).max((1 << 20) / piece_len).min(((64 << 20) / piece_len).max(threads + 1));

    let files = &t.files;
    let mut hashes = vec![Sha1Hash::default(); num_pieces as usize];
    let result = std::thread::scope(|scope| {
        let (job_tx, job_rx) = mpsc::sync_channel::<(u32, Vec<u8>)>(read_ahead);
        let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
        let (done_tx, done_rx) = mpsc::channel::<(u32, Sha1Hash, Vec<u8>)>();
        for _ in 0..threads {
            let job_rx = job_rx.clone();
            let done_tx = done_tx.clone();
            scope.spawn(move || {
                loop {
                    let job = job_rx.lock().map(|rx| rx.recv());
                    let Ok(Ok((piece, buf))) = job else { break };
                    let h = Sha1::digest(&buf);
                    if done_tx.send((piece, h, buf)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(done_tx);

        let mut reader = PieceReader::new(files, base);
        let mut spare: Vec<Vec<u8>> = Vec::new();
        let mut completed = 0u32;
        let mut in_flight = 0usize;
        let mut result = Ok(());
        // records a finished piece; false when the caller cancelled
        let mut handle_done = |(piece, h, buf): (u32, Sha1Hash, Vec<u8>), spare: &mut Vec<Vec<u8>>| -> bool {
            hashes[piece as usize] = h;
            spare.push(buf);
            let keep_going = progress(completed);
            completed += 1;
            keep_going
        };

        'read: for piece in 0..num_pieces {
            // collect finished hashes without blocking, and block while too many are in flight
            loop {
                let next =
                    if in_flight >= read_ahead { done_rx.recv().ok() } else { done_rx.try_recv().ok() };
                let Some(done) = next else { break };
                in_flight -= 1;
                if !handle_done(done, &mut spare) {
                    result = Err(CreateTorrentError::Aborted);
                    break 'read;
                }
            }
            let mut buf = spare.pop().unwrap_or_default();
            if let Err(e) = reader.read_piece(piece, &mut buf) {
                result = Err(e);
                break;
            }
            if job_tx.send((piece, buf)).is_err() {
                break;
            }
            in_flight += 1;
        }
        drop(job_tx);
        // drain the hashers (also after an error, so the scope can end)
        for done in done_rx.iter() {
            if result.is_ok() && !handle_done(done, &mut spare) {
                result = Err(CreateTorrentError::Aborted);
            }
        }
        result
    });
    result?;
    for (i, h) in hashes.into_iter().enumerate() {
        t.set_hash(i as u32, h);
    }
    Ok(())
}

/// Reads pieces in order across file boundaries; pad files read as zeros.
struct PieceReader<'a> {
    fs: &'a FileStorage,
    base: &'a Path,
    /// The file being read and its handle.
    file: usize,
    handle: Option<File>,
}

impl<'a> PieceReader<'a> {
    fn new(fs: &'a FileStorage, base: &'a Path) -> PieceReader<'a> {
        PieceReader { fs, base, file: 0, handle: None }
    }

    fn read_piece(&mut self, piece: u32, buf: &mut Vec<u8>) -> Result<(), CreateTorrentError> {
        let size = self.fs.piece_size(piece) as usize;
        buf.clear();
        buf.resize(size, 0);
        let mut filled = 0usize;
        let mut offset = piece as u64 * self.fs.piece_length() as u64;
        while filled < size {
            // advance to the file containing `offset` (skipping empty files)
            while self.file < self.fs.num_files()
                && self.fs.file_offset(self.file) + self.fs.file_size(self.file) <= offset
            {
                self.file += 1;
                self.handle = None;
            }
            let i = self.file;
            let in_file = offset - self.fs.file_offset(i);
            let n = ((self.fs.file_size(i) - in_file) as usize).min(size - filled);
            let dst = &mut buf[filled..filled + n];
            if !self.fs.pad_file_at(i) {
                let path = self.base.join(self.fs.file_path(i, ""));
                if self.handle.is_none() {
                    self.handle =
                        Some(File::open(&path).map_err(|e| CreateTorrentError::Io(path.clone(), e))?);
                }
                // files are read front to back, so the position is already `in_file`
                if let Some(h) = self.handle.as_mut() {
                    h.read_exact(dst).map_err(|e| CreateTorrentError::Io(path, e))?;
                }
            }
            filled += n;
            offset += n as u64;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! Ported from libtorrent-rasterbar 2.0.11 test/test_create_torrent.cpp (the v1 parts), plus
    //! hashing tests against files on disk.
    use super::*;
    use crate::metainfo::TorrentInfo;

    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;

    fn fs_with(files: &[(&str, u64)]) -> FileStorage {
        let mut fs = FileStorage::new();
        for (p, s) in files {
            fs.add_file(p, *s, FileFlags::empty(), 0, "", None).unwrap();
        }
        fs
    }

    fn generate_buf(t: &CreateTorrent) -> Vec<u8> {
        let buf = t.generate().unwrap().bencode();
        assert_eq!(buf, t.generate_buf().unwrap());
        buf
    }

    // make sure creating a torrent from an existing handle preserves the info-dictionary
    // verbatim, so as to not alter the info-hash
    #[test]
    fn create_verbatim_torrent() {
        let test_torrent: &[u8] =
            b"d4:infod4:name6:foobar6:lengthi12345e12:piece lengthi65536e6:pieces20:ababababababababababee";
        let info = TorrentInfo::from_bytes(test_torrent).unwrap();
        let t = CreateTorrent::from_torrent_info(&info).unwrap();
        let buffer = generate_buf(&t);
        let at = buffer.windows(6).position(|w| w == b"4:info").unwrap();
        // strip the outermost dictionary from the source torrent, since create_torrent may have
        // added items next to the info dict
        let inner = &test_torrent[1..test_torrent.len() - 1];
        assert_eq!(&buffer[at..at + inner.len()], inner);
    }

    #[test]
    fn auto_piece_size() {
        let samples = [
            (100, 16 * KIB),
            (3 * MIB, 32 * KIB),
            (11 * MIB, 64 * KIB),
            (43 * MIB, 128 * KIB),
            (172 * MIB, 256 * KIB),
            (688 * MIB, 512 * KIB),
            (3 * GIB, MIB),
            (11 * GIB, 2 * MIB),
            (44 * GIB, 4 * MIB),
            (176 * GIB, 8 * MIB),
            (704 * GIB, 16 * MIB),
        ];
        for (size, expected) in samples {
            let ct = CreateTorrent::new(fs_with(&[("a", size)]), 0, CreateFlags::empty()).unwrap();
            assert_eq!(ct.piece_length() as u64, expected, "{size}");
        }
    }

    fn test_piece_size(piece_size: u32, f: CreateFlags) -> Result<u32, CreateTorrentError> {
        CreateTorrent::new(fs_with(&[("a", 100 * MIB)]), piece_size, f).map(|ct| ct.piece_length())
    }

    // libtorrent's v1_only cases (we are always v1)
    #[test]
    fn piece_size_restriction_16kb() {
        assert!(test_piece_size(15000, CreateFlags::empty()).is_err());
        assert!(test_piece_size(8000, CreateFlags::empty()).is_err());
        assert_eq!(test_piece_size(8192, CreateFlags::empty()).unwrap(), 8192);
        assert_eq!(test_piece_size(15000, CreateFlags::ALLOW_ODD_PIECE_SIZE).unwrap(), 15000);
    }

    #[test]
    fn piece_size_quanta() {
        assert_eq!(test_piece_size(32 * 1024, CreateFlags::empty()).unwrap(), 32 * 1024);
        assert_eq!(test_piece_size(48 * 1024, CreateFlags::empty()).unwrap(), 48 * 1024);
        assert!(test_piece_size(47 * 1024, CreateFlags::empty()).is_err());
    }

    #[test]
    fn piece_size_limit() {
        assert_eq!(test_piece_size(0x8000000, CreateFlags::empty()).unwrap(), 0x8000000);
        assert!(test_piece_size(0x8000001, CreateFlags::empty()).is_err());
        assert!(test_piece_size(0x10000000, CreateFlags::empty()).is_err());
    }

    #[test]
    fn v2_is_unsupported() {
        assert!(matches!(
            CreateTorrent::new(fs_with(&[("test/A", 0x8002)]), 0x4000, CreateFlags::V2_ONLY),
            Err(CreateTorrentError::V2Unsupported)
        ));
        let mut t = CreateTorrent::new(fs_with(&[("test/A", 0x8002)]), 0x4000, CreateFlags::empty()).unwrap();
        // v1_only_set_hash2
        assert!(t.set_hash2(0, 0, crate::types::Sha256Hash::max()).is_err());
    }

    #[test]
    fn create_torrent_round_trip() {
        let test_torrent: &[u8] = b"d8:announce26:udp://testurl.com/announce7:comment22:this is a test comment13:creation datei1337e4:infod6:lengthi12345e4:name6:foobar12:piece lengthi65536e6:pieces20:ababababababababababee";
        let info1 = TorrentInfo::from_bytes(test_torrent).unwrap();
        assert_eq!(info1.comment(), "this is a test comment");
        assert_eq!(info1.trackers().len(), 1);
        assert_eq!(info1.trackers()[0].url, "udp://testurl.com/announce");

        let t = CreateTorrent::from_torrent_info(&info1).unwrap();
        let buffer = generate_buf(&t);
        let info2 = TorrentInfo::from_bytes(&buffer).unwrap();
        assert_eq!(info2.comment(), "this is a test comment");
        assert_eq!(info2.trackers().len(), 1);
        assert_eq!(info2.trackers()[0].url, "udp://testurl.com/announce");
        assert_eq!(info1.info_hashes(), info2.info_hashes());
        assert_eq!(info2.hash_for_piece(0), info1.hash_for_piece(0));
        assert_eq!(info2.creation_date(), Some(1337));
    }

    #[test]
    fn from_v2_torrent_is_unsupported() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/torrents");
        let ti = TorrentInfo::from_file(dir.join("v2_only.torrent")).unwrap();
        assert!(matches!(CreateTorrent::from_torrent_info(&ti), Err(CreateTorrentError::V2Unsupported)));
    }

    // if we don't specify a v1-only flag, but only set v1 hashes, the created torrent is
    // implicitly v1-only (libtorrent pads here because its default is hybrid; we need the flag)
    #[test]
    fn implicit_v1_only() {
        let fs = fs_with(&[("test/A", 0x8002), ("test/B", 0x4002)]);
        let mut t = CreateTorrent::new(fs, 0x4000, CreateFlags::CANONICAL_FILES).unwrap();
        for i in 0..t.num_pieces() {
            t.set_hash(i, Sha1Hash::max());
        }
        let info = TorrentInfo::from_bytes(&generate_buf(&t)).unwrap();
        assert!(!info.v2());
        assert!(info.v1());
        assert_eq!(info.files().file_name(0), "A");
        assert!(info.files().pad_file_at(1));
        assert_eq!(info.files().file_name(2), "B");
        assert_eq!(info.name(), "test");
    }

    #[test]
    fn missing_hashes() {
        let mut t = CreateTorrent::new(fs_with(&[("A", 0x8000)]), 0x4000, CreateFlags::empty()).unwrap();
        assert!(matches!(t.generate(), Err(CreateTorrentError::InvalidHashEntry)));
        t.set_hash(0, Sha1Hash::max());
        assert!(matches!(t.generate(), Err(CreateTorrentError::InvalidHashEntry)));
        t.set_hash(1, Sha1Hash::max());
        assert!(t.generate().is_ok());
        let empty = CreateTorrent::new(fs_with(&[("A", 0)]), 0x4000, CreateFlags::empty()).unwrap();
        assert!(matches!(empty.generate(), Err(CreateTorrentError::TorrentMissingFileTree)));
    }

    fn test_field(f: impl FnOnce(&mut CreateTorrent)) -> TorrentInfo {
        let mut t = CreateTorrent::new(fs_with(&[("A", 0x4000)]), 0x4000, CreateFlags::empty()).unwrap();
        for i in 0..t.num_pieces() {
            t.set_hash(i, Sha1Hash::max());
        }
        f(&mut t);
        TorrentInfo::from_bytes(&generate_buf(&t)).unwrap()
    }

    #[test]
    fn no_creation_date() {
        let info = test_field(|t| t.set_creation_date(0));
        assert_eq!(info.creation_date(), None);
    }

    #[test]
    fn creation_date() {
        let info = test_field(|t| t.set_creation_date(1337));
        assert_eq!(info.creation_date(), Some(1337));
    }

    #[test]
    fn comment() {
        assert_eq!(test_field(|t| t.set_comment("foobar")).comment(), "foobar");
    }

    #[test]
    fn creator() {
        assert_eq!(test_field(|t| t.set_creator("foobar")).creator(), "foobar");
    }

    #[test]
    fn dht_nodes() {
        let info = test_field(|t| t.add_node("foobar", 1337));
        assert_eq!(info.nodes(), [("foobar".to_owned(), 1337)]);
    }

    #[test]
    fn ssl_cert() {
        let info = test_field(|t| t.set_root_cert(b"foobar"));
        assert_eq!(info.ssl_cert().as_deref(), Some(&b"foobar"[..]));
    }

    #[test]
    fn priv_() {
        assert!(test_field(|t| t.set_priv(true)).is_private());
        assert!(!test_field(|_| {}).is_private());
    }

    #[test]
    fn source_changes_info_hash() {
        let plain = test_field(|t| t.set_creation_date(0));
        let sourced = test_field(|t| {
            t.set_creation_date(0);
            t.set_source("TRACKER");
        });
        assert_ne!(plain.info_hash(), sourced.info_hash());
        let info = sourced.info_dict().unwrap();
        let source = info.root().and_then(|r| r.dict_find_string_value("source")).map(<[u8]>::to_vec);
        assert_eq!(source.as_deref(), Some(&b"TRACKER"[..]));
    }

    #[test]
    fn trackers_and_web_seeds() {
        let info = test_field(|t| {
            t.add_tracker("http://b/announce", 1);
            t.add_tracker("http://a/announce", 0);
            t.add_tracker("http://c/announce", 1);
            t.add_tracker("http://a/announce", 3); // duplicate, ignored
            t.add_tracker("", 0);
            t.add_url_seed("http://seed/one");
            t.add_url_seed("http://seed/two");
            t.add_http_seed("http://httpseed/");
        });
        // the parser shuffles trackers within a tier
        let mut trackers: Vec<_> = info.trackers().iter().map(|a| (a.url.as_str(), a.tier)).collect();
        trackers.sort();
        assert_eq!(trackers, [("http://a/announce", 0), ("http://b/announce", 1), ("http://c/announce", 1)]);
        let seeds: Vec<_> = info.web_seeds().iter().map(|w| (w.url.as_str(), w.kind)).collect();
        assert_eq!(
            seeds,
            [
                ("http://seed/one", WebSeedKind::UrlSeed),
                ("http://seed/two", WebSeedKind::UrlSeed),
                ("http://httpseed/", WebSeedKind::HttpSeed)
            ]
        );
        // a single tracker and a single web seed are plain strings
        let mut t = CreateTorrent::new(fs_with(&[("A", 0x4000)]), 0x4000, CreateFlags::empty()).unwrap();
        t.set_hash(0, Sha1Hash::max());
        t.set_creation_date(0);
        t.add_tracker("http://a/announce", 0);
        t.add_url_seed("http://seed/");
        assert_eq!(
            generate_buf(&t),
            b"d8:announce17:http://a/announce4:infod6:lengthi16384e4:name1:A12:piece lengthi16384e6:pieces20:\
              \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xffe\
              8:url-list12:http://seed/e"
                .as_slice()
        );
    }

    fn test_create_torrent(fs: FileStorage, piece_size: u32, flags: CreateFlags) -> Vec<u8> {
        let mut ct = CreateTorrent::new(fs, piece_size, flags).unwrap();
        ct.set_creation_date(1337);
        for i in 0..ct.num_pieces() {
            ct.set_hash(i, Sha1Hash::max());
        }
        generate_buf(&ct)
    }

    const FF40: &[u8] = &[0xff; 40];

    fn expected(files: &str) -> Vec<u8> {
        let mut v = format!(
            "d13:creation datei1337e4:infod5:filesl{files}e4:name4:test12:piece lengthi16384e6:pieces40:"
        )
        .into_bytes();
        v.extend_from_slice(FF40);
        v.extend_from_slice(b"ee");
        v
    }

    const SMALL1: &str = "d6:lengthi16383e4:pathl7:1-smallee";
    const SMALL2: &str = "d6:lengthi16383e4:pathl7:2-smallee";
    const SMALL3: &str = "d6:lengthi16383e4:pathl7:3-smallee";
    const EMPTY2: &str = "d6:lengthi0e4:pathl7:2-emptyee";
    const PAD1: &str = "d4:attr1:p6:lengthi1e4:pathl4:.pad1:1ee";

    #[test]
    fn v1_tail_padding() {
        let fs = fs_with(&[("test/1-small", 0x3fff), ("test/2-small", 0x3fff)]);
        assert_eq!(
            test_create_torrent(fs, 0x4000, CreateFlags::CANONICAL_FILES),
            expected(&format!("{SMALL1}{PAD1}{SMALL2}{PAD1}"))
        );
    }

    // padding goes before empty files, not after
    #[test]
    fn v1_empty_file_placement() {
        let fs = fs_with(&[("test/1-small", 0x3fff), ("test/2-empty", 0), ("test/3-small", 0x3fff)]);
        assert_eq!(
            test_create_torrent(fs, 0x4000, CreateFlags::CANONICAL_FILES),
            expected(&format!("{SMALL1}{PAD1}{EMPTY2}{SMALL3}{PAD1}"))
        );
    }

    // despite the files being added in one order, the torrent is still created with files in the
    // canonical order
    #[test]
    fn v1_file_sorting() {
        let fs = fs_with(&[("test/2-small", 0x3fff), ("test/1-small", 0x3fff)]);
        assert_eq!(
            test_create_torrent(fs, 0x4000, CreateFlags::CANONICAL_FILES),
            expected(&format!("{SMALL1}{PAD1}{SMALL2}{PAD1}"))
        );
    }

    // This is a backwards compatibility feature
    #[test]
    fn v1_no_tail_padding() {
        let fs = fs_with(&[("test/1-small", 0x3fff), ("test/2-small", 0x3fff)]);
        assert_eq!(
            test_create_torrent(fs, 0x4000, CreateFlags::CANONICAL_FILES_NO_TAIL_PADDING),
            expected(&format!("{SMALL1}{PAD1}{SMALL2}"))
        );
    }

    // padding goes after empty files in backwards compatibility mode
    #[test]
    fn v1_empty_file_placement_backwards_compatibility() {
        let fs = fs_with(&[("test/1-small", 0x3fff), ("test/2-empty", 0), ("test/3-small", 0x3fff)]);
        assert_eq!(
            test_create_torrent(fs, 0x4000, CreateFlags::CANONICAL_FILES_NO_TAIL_PADDING),
            expected(&format!("{SMALL1}{EMPTY2}{PAD1}{SMALL3}"))
        );
    }

    // the v1_only flag does not arrange files canonically (i.e. no ordering nor padding)
    #[test]
    fn v1_no_padding() {
        let fs = fs_with(&[("test/1-small", 0x3fff), ("test/2-small", 0x3fff)]);
        assert_eq!(
            test_create_torrent(fs, 0x4000, CreateFlags::empty()),
            expected(&format!("{SMALL1}{SMALL2}"))
        );
    }

    #[test]
    fn single_file() {
        let fs = fs_with(&[("1-small", 0x3fff)]);
        let mut want =
            b"d13:creation datei1337e4:infod6:lengthi16383e4:name7:1-small12:piece lengthi16384e6:pieces20:"
                .to_vec();
        want.extend_from_slice(&[0xff; 20]);
        want.extend_from_slice(b"ee");
        assert_eq!(test_create_torrent(fs, 0x4000, CreateFlags::empty()), want);
    }

    /// A scratch directory under the target dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let p = std::env::temp_dir().join(format!("patorrent-create-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pattern(len: usize, seed: u8) -> Vec<u8> {
        (0..len).map(|i| (i as u32).wrapping_mul(2_654_435_761).wrapping_add(seed as u32) as u8).collect()
    }

    /// The torrent built from a directory hashes the same bytes as the concatenated files.
    #[test]
    fn hash_directory() {
        let dir = TempDir::new("dir");
        let root = dir.0.join("content");
        std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
        std::fs::create_dir_all(root.join("emptydir")).unwrap();
        let files: [(&str, Vec<u8>); 5] = [
            ("b.bin", pattern(40_000, 1)),
            ("a.txt", pattern(100, 2)),
            ("sub/deeper/c", pattern(70_000, 3)),
            ("sub/empty", Vec::new()),
            ("z", pattern(16_384, 4)),
        ];
        for (p, data) in &files {
            std::fs::write(root.join(p), data).unwrap();
        }

        let mut fs = FileStorage::new();
        let mut seen = 0;
        add_files(
            &mut fs,
            &root,
            &mut |_| {
                seen += 1;
                true
            },
            CreateFlags::empty(),
        )
        .unwrap();
        assert_eq!(fs.num_files(), 5);
        assert!(seen >= 8);
        // name order, depth first
        let order: Vec<_> = (0..fs.num_files()).map(|i| fs.file_path(i, "")).collect();
        assert_eq!(
            order,
            ["content/a.txt", "content/b.bin", "content/sub/deeper/c", "content/sub/empty", "content/z"]
        );

        let mut t = CreateTorrent::new(fs, 0x8000, CreateFlags::empty()).unwrap();
        t.add_tracker("http://tracker/announce", 0);
        let mut calls = Vec::new();
        set_piece_hashes_threads(&mut t, &parent_dir(&root).unwrap(), 3, &mut |p| {
            calls.push(p);
            true
        })
        .unwrap();
        assert_eq!(calls, (0..t.num_pieces()).collect::<Vec<_>>());

        let mut all = Vec::new();
        for name in ["a.txt", "b.bin", "sub/deeper/c", "sub/empty", "z"] {
            all.extend_from_slice(&files.iter().find(|f| f.0 == name).unwrap().1);
        }
        let info = TorrentInfo::from_bytes(&t.generate_buf().unwrap()).unwrap();
        assert_eq!(info.name(), "content");
        assert_eq!(info.num_files(), 5);
        assert_eq!(info.total_size(), all.len() as u64);
        assert_eq!(info.num_pieces() as usize, all.len().div_ceil(0x8000));
        for (i, chunk) in all.chunks(0x8000).enumerate() {
            assert_eq!(info.hash_for_piece(i as u32), Some(Sha1::digest(chunk)), "piece {i}");
        }
        assert_eq!(info.files().file_path(2, ""), "content/sub/deeper/c");
    }

    /// Pad files hash as zeros; a predicate can skip entries.
    #[test]
    fn hash_canonical_with_predicate() {
        let dir = TempDir::new("canon");
        let root = dir.0.join("t");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("one"), pattern(20_000, 5)).unwrap();
        std::fs::write(root.join("two"), pattern(5_000, 6)).unwrap();
        std::fs::write(root.join("skip.me"), b"x").unwrap();

        let mut fs = FileStorage::new();
        add_files(&mut fs, &root, &mut |p| p.extension().is_none_or(|e| e != "me"), CreateFlags::empty())
            .unwrap();
        assert_eq!(fs.num_files(), 2);
        let mut t = CreateTorrent::new(fs, 0x4000, CreateFlags::CANONICAL_FILES).unwrap();
        set_piece_hashes(&mut t, &dir.0, &mut |_| true).unwrap();
        let info = TorrentInfo::from_bytes(&t.generate_buf().unwrap()).unwrap();
        // one, pad, two, pad
        assert_eq!(info.num_files(), 4);
        let one = pattern(20_000, 5);
        let mut second = one[0x4000..].to_vec();
        second.resize(0x4000, 0);
        let mut last = pattern(5_000, 6);
        last.resize(0x4000, 0);
        assert_eq!(info.num_pieces(), 3);
        assert_eq!(info.hash_for_piece(0), Some(Sha1::digest(&one[..0x4000])));
        assert_eq!(info.hash_for_piece(1), Some(Sha1::digest(&second)));
        assert_eq!(info.hash_for_piece(2), Some(Sha1::digest(&last)));
    }

    #[test]
    fn hash_single_file_cancel_and_errors() {
        let dir = TempDir::new("single");
        let file = dir.0.join("movie.bin");
        std::fs::write(&file, pattern(100_000, 7)).unwrap();

        let mut fs = FileStorage::new();
        add_files(&mut fs, &file, &mut |_| true, CreateFlags::MODIFICATION_TIME).unwrap();
        assert_eq!(fs.num_files(), 1);
        assert!(fs.mtime(0) > 0);
        let mut t = CreateTorrent::new(fs, 0x4000, CreateFlags::MODIFICATION_TIME).unwrap();

        // cancel after two pieces
        let mut n = 0;
        let r = set_piece_hashes(&mut t, &dir.0, &mut |_| {
            n += 1;
            n < 2
        });
        assert!(matches!(r, Err(CreateTorrentError::Aborted)));

        set_piece_hashes(&mut t, &dir.0, &mut |_| true).unwrap();
        let e = t.generate().unwrap();
        assert!(e.get("info").unwrap().get("mtime").is_some());
        let info = TorrentInfo::from_bytes(&e.bencode()).unwrap();
        assert_eq!(info.name(), "movie.bin");
        assert_eq!(info.files().file_path(0, ""), "movie.bin");

        // the file shrank since it was added
        std::fs::write(&file, pattern(1000, 7)).unwrap();
        assert!(matches!(set_piece_hashes(&mut t, &dir.0, &mut |_| true), Err(CreateTorrentError::Io(..))));
        std::fs::remove_file(&file).unwrap();
        assert!(matches!(set_piece_hashes(&mut t, &dir.0, &mut |_| true), Err(CreateTorrentError::Io(..))));

        let mut empty = CreateTorrent::new(FileStorage::new(), 0, CreateFlags::empty()).unwrap();
        assert!(matches!(
            set_piece_hashes(&mut empty, &dir.0, &mut |_| true),
            Err(CreateTorrentError::NoFilesInTorrent)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn hidden_attribute() {
        let dir = TempDir::new("hidden");
        let root = dir.0.join("h");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("visible"), b"abc").unwrap();
        let hidden = root.join("hidden");
        std::fs::write(&hidden, b"def").unwrap();
        let st = std::process::Command::new("attrib").arg("+h").arg(&hidden).status().unwrap();
        assert!(st.success());

        let mut fs = FileStorage::new();
        add_files(&mut fs, &root, &mut |_| true, CreateFlags::empty()).unwrap();
        let mut t = CreateTorrent::new(fs, 0x4000, CreateFlags::empty()).unwrap();
        set_piece_hashes(&mut t, &dir.0, &mut |_| true).unwrap();
        let info = TorrentInfo::from_bytes(&t.generate_buf().unwrap()).unwrap();
        assert_eq!(info.files().file_name(0), "hidden");
        assert!(info.files().file_flags(0).contains(FileFlags::HIDDEN));
        assert!(!info.files().file_flags(1).contains(FileFlags::HIDDEN));

        let mut fs = FileStorage::new();
        add_files(&mut fs, &root, &mut |_| true, CreateFlags::NO_ATTRIBUTES).unwrap();
        assert!(!fs.file_flags(0).contains(FileFlags::HIDDEN));
    }

    #[cfg(unix)]
    #[test]
    fn create_torrent_symlink() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new("symlink");
        let root = dir.0.join("test-torrent");
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("a/b/c/file-1"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("d/file-2"), vec![0u8; 1000]).unwrap();
        symlink("../a/b/c/file-1", root.join("d/test-link-1")).unwrap();
        symlink("a/b/c/file-1", root.join("test-link-2")).unwrap();
        symlink("a/b/c/file-1", root.join("a/b/c/test-link-3")).unwrap();
        symlink("../../../d/file-2", root.join("a/b/c/test-link-4")).unwrap();

        let mut fs = FileStorage::new();
        add_files(&mut fs, &root, &mut |_| true, CreateFlags::SYMLINKS).unwrap();
        let mut t = CreateTorrent::new(fs, 16 * 1024, CreateFlags::SYMLINKS).unwrap();
        set_piece_hashes(&mut t, &dir.0, &mut |_| true).unwrap();
        let ti = TorrentInfo::from_bytes(&generate_buf(&t)).unwrap();

        let mut found = 0;
        for i in 0..ti.num_files() {
            let filename = ti.files().file_path(i, "");
            if filename == "test-torrent/d/test-link-1"
                || filename == "test-torrent/test-link-2"
                || filename == "test-torrent/a/b/c/test-link-3"
            {
                assert_eq!(ti.files().symlink(i), "test-torrent/a/b/c/file-1");
                found += 1;
            } else if filename == "test-torrent/a/b/c/test-link-4" {
                assert_eq!(ti.files().symlink(i), "test-torrent/d/file-2");
                found += 1;
            }
        }
        assert_eq!(found, 4);
    }

    #[cfg(unix)]
    #[test]
    fn v1_attributes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("attr");
        let f = dir.0.join("file-1");
        std::fs::write(&f, vec![0u8; 1000]).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o700)).unwrap();

        let mut fs = FileStorage::new();
        add_files(&mut fs, &f, &mut |_| true, CreateFlags::empty()).unwrap();
        let mut t = CreateTorrent::new(fs, 16 * 1024, CreateFlags::empty()).unwrap();
        set_piece_hashes(&mut t, &dir.0, &mut |_| true).unwrap();
        let e = t.generate().unwrap();
        assert_eq!(e.get("info").unwrap().get("attr").unwrap().as_bytes(), Some(&b"x"[..]));
    }
}
