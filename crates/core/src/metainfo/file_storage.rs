// Ported from libtorrent-rasterbar 2.0.11: src/file_storage.cpp, include/libtorrent/file_storage.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes:
// - paths use '/' on every platform (see util::path);
// - borrowed names are offsets into a shared `Arc<[u8]>` info section instead of raw pointers,
//   owned names live in one arena string instead of one allocation each;
// - the directory table has a hash index (libtorrent searches it linearly, which is quadratic for
//   hostile torrents with many directories);
// - per-file SHA-1, mtime and v2 roots are side tables sized on first use, as in libtorrent.

//! The layout of a torrent's files in the piece space (libtorrent `file_storage`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::merkle::{merkle_num_leafs, merkle_num_nodes};
use crate::crypto::crc32c::Crc32c;
use crate::flags::bit_flags;
use crate::types::{Sha1Hash, Sha256Hash};
use crate::util::path::{
    append_path, combine_path, is_complete, lsplit_path, lsplit_path_at, path_compare, rsplit_path,
};

/// The BitTorrent request size.
pub const DEFAULT_BLOCK_SIZE: u32 = 0x4000;
/// Largest file the merkle-tree index math can handle.
pub const MAX_FILE_SIZE: u64 = {
    let a = (1u64 << 48) - 1;
    let b = (i32::MAX as u64 / 2) * DEFAULT_BLOCK_SIZE as u64;
    if a < b { a } else { b }
};
pub const MAX_FILE_OFFSET: u64 = (1 << 48) - 1;
pub const MAX_NUM_PIECES: u32 = (1 << 30) - 1;
pub const MAX_PIECE_SIZE: u32 = ((1 << 15) - 1) * 0x4000;

/// Longest single file name (libtorrent stores the length in 12 bits).
const MAX_NAME_LEN: usize = (1 << 12) - 1;
const NO_PATH: u32 = (1 << 30) - 1;
const PATH_IS_ABSOLUTE: u32 = (1 << 30) - 2;
const NOT_A_SYMLINK: u32 = u32::MAX;

bit_flags! {
    /// libtorrent `file_flags_t` (BEP 47 attributes).
    pub struct FileFlags: u8 {
        const PAD_FILE = 0;
        const HIDDEN = 1;
        const EXECUTABLE = 2;
        const SYMLINK = 3;
    }
}

/// A byte range of one file (libtorrent `file_slice`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSlice {
    pub file_index: usize,
    pub offset: u64,
    pub size: u64,
}

/// A byte range of one piece (libtorrent `peer_request`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerRequest {
    pub piece: u32,
    pub start: u32,
    pub length: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStorageError {
    /// errc::file_too_large
    FileTooLarge,
    /// errc::filename_too_long
    FilenameTooLong,
    /// errors::torrent_invalid_length: the total size would exceed 2^48 - 1.
    InvalidLength,
    /// errors::torrent_missing_pieces_root: a v1 file was added to v2 storage.
    MissingPiecesRoot,
    /// errors::torrent_inconsistent_files: a v2 file was added to v1 storage.
    InconsistentFiles,
}

/// Where a file's name bytes live.
#[derive(Clone, Copy, Debug)]
enum Name {
    Empty,
    /// In the shared info-section buffer.
    Borrowed {
        off: u32,
        len: u32,
    },
    /// In this storage's own arena.
    Owned {
        off: u32,
        len: u32,
    },
}

#[derive(Clone, Debug)]
struct Entry {
    offset: u64,
    size: u64,
    name: Name,
    path_index: u32,
    symlink_index: u32,
    flags: FileFlags,
    /// The directory path does not start with the torrent name.
    no_root_dir: bool,
}

impl Entry {
    fn new() -> Entry {
        Entry {
            offset: 0,
            size: 0,
            name: Name::Empty,
            path_index: NO_PATH,
            symlink_index: NOT_A_SYMLINK,
            flags: FileFlags::empty(),
            no_root_dir: false,
        }
    }
}

/// The files of a torrent and how they map onto pieces.
#[derive(Clone, Debug, Default)]
pub struct FileStorage {
    piece_length: u32,
    num_pieces: u32,
    v2: bool,
    files: Vec<Entry>,
    /// Sized on first use; zero hash = none.
    file_hashes: Vec<Sha1Hash>,
    /// v2 merkle roots; sized on first use; zero = none.
    roots: Vec<Sha256Hash>,
    symlinks: Vec<String>,
    /// Sized on first use.
    mtime: Vec<i64>,
    paths: Vec<String>,
    path_index: HashMap<String, u32>,
    name: String,
    total_size: u64,
    /// The info section that borrowed names point into.
    buffer: Option<Arc<[u8]>>,
    owned_names: String,
}

impl FileStorage {
    pub fn new() -> FileStorage {
        FileStorage::default()
    }

    /// Storage whose borrowed names point into `buffer` (the torrent's info section).
    pub fn with_buffer(buffer: Arc<[u8]>) -> FileStorage {
        FileStorage { buffer: Some(buffer), ..FileStorage::default() }
    }

    pub fn is_valid(&self) -> bool {
        self.piece_length > 0
    }

    pub fn reserve(&mut self, num_files: usize) {
        self.files.reserve(num_files);
    }

    pub fn num_files(&self) -> usize {
        self.files.len()
    }

    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    pub fn set_num_pieces(&mut self, n: u32) {
        self.num_pieces = n;
    }

    pub fn num_pieces(&self) -> u32 {
        self.num_pieces
    }

    pub fn set_piece_length(&mut self, l: u32) {
        self.piece_length = l;
    }

    pub fn piece_length(&self) -> u32 {
        self.piece_length
    }

    pub fn set_name(&mut self, n: impl Into<String>) {
        self.name = n.into();
    }

    /// The torrent name: the root directory for multi-file torrents.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// True once a file with a v2 merkle root has been added.
    pub fn v2(&self) -> bool {
        self.v2
    }

    /// Directory paths relative to the torrent root, indexed by entries.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// Size of piece `index`; only the last piece can be shorter.
    pub fn piece_size(&self, index: u32) -> u32 {
        debug_assert!(index < self.num_pieces);
        if index + 1 == self.num_pieces {
            let except_last = (self.num_pieces as u64 - 1) * self.piece_length as u64;
            (self.total_size - except_last) as u32
        } else {
            self.piece_length
        }
    }

    /// v2 piece size: pieces end at file boundaries (pad files are their own "files").
    pub fn piece_size2(&self, index: u32) -> u32 {
        let start = self.piece_length as u64 * index as u64;
        let next = self.files.partition_point(|f| f.offset <= start);
        match self.files.get(next) {
            None => self.piece_size(index),
            Some(f) => (self.piece_length as u64).min(f.offset - start) as u32,
        }
    }

    pub fn blocks_in_piece2(&self, index: u32) -> u32 {
        self.piece_size2(index).div_ceil(DEFAULT_BLOCK_SIZE)
    }

    pub fn blocks_per_piece(&self) -> u32 {
        self.piece_length.div_ceil(DEFAULT_BLOCK_SIZE)
    }

    /// Adds a file whose name is copied out of `path`.
    pub fn add_file(
        &mut self,
        path: &str,
        size: u64,
        flags: FileFlags,
        mtime: i64,
        symlink_path: &str,
        root: Option<Sha256Hash>,
    ) -> Result<(), FileStorageError> {
        self.add_file_borrow(None, path, size, flags, None, mtime, symlink_path, root)
    }

    /// libtorrent `add_file_borrow`: `name` is a `(offset, len)` range in the shared buffer
    /// holding the file name, so it needn't be copied. `path` is the full (sanitized) path
    /// including the torrent name for multi-file torrents.
    #[allow(clippy::too_many_arguments)]
    pub fn add_file_borrow(
        &mut self,
        name: Option<(u32, u32)>,
        path: &str,
        size: u64,
        flags: FileFlags,
        filehash: Option<Sha1Hash>,
        mtime: i64,
        symlink_path: &str,
        root: Option<Sha256Hash>,
    ) -> Result<(), FileStorageError> {
        if size > MAX_FILE_SIZE {
            return Err(FileStorageError::FileTooLarge);
        }
        if MAX_FILE_OFFSET - self.total_size < size {
            return Err(FileStorageError::InvalidLength);
        }
        let name_len = match name {
            Some((_, len)) => len as usize,
            None => rsplit_path(path).1.len(),
        };
        if name_len > MAX_NAME_LEN {
            return Err(FileStorageError::FilenameTooLong);
        }

        if !crate::util::path::has_parent_path(path) {
            // a single-file torrent: the file name is the torrent name
            debug_assert!(self.files.is_empty());
            self.name = path.to_owned();
        } else if self.files.is_empty() {
            self.name = lsplit_path(path).0.to_owned();
        }

        // Files without a root are v1, except symlinks and empty files, which can be either.
        if symlink_path.is_empty() && size > 0 {
            let v2 = root.is_some();
            if self.files.len() == self.symlinks.len() || self.total_size == 0 {
                self.v2 = v2;
            } else if self.v2 != v2 {
                return Err(if self.v2 {
                    FileStorageError::MissingPiecesRoot
                } else {
                    FileStorageError::InconsistentFiles
                });
            }
        }

        let mut e = Entry::new();
        self.update_path_index(&mut e, path, name.is_none());
        if let Some((off, len)) = name {
            e.name = Name::Borrowed { off, len };
        }
        e.size = size;
        e.offset = self.total_size;
        e.flags = flags & !FileFlags::SYMLINK;
        self.files.push(e);
        let index = self.files.len() - 1;

        if let Some(h) = filehash {
            if self.file_hashes.len() < self.files.len() {
                self.file_hashes.resize(self.files.len(), Sha1Hash::default());
            }
            self.file_hashes[index] = h;
        }
        if let Some(r) = root {
            if self.roots.len() < self.files.len() {
                self.roots.resize(self.files.len(), Sha256Hash::default());
            }
            self.roots[index] = r;
        }
        if !symlink_path.is_empty() && self.symlinks.len() < (NOT_A_SYMLINK - 1) as usize {
            self.files[index].symlink_index = self.symlinks.len() as u32;
            self.files[index].flags.set(FileFlags::SYMLINK, flags.contains(FileFlags::SYMLINK));
            self.symlinks.push(symlink_path.to_owned());
        }
        if mtime != 0 {
            if self.mtime.len() < self.files.len() {
                self.mtime.resize(self.files.len(), 0);
            }
            self.mtime[index] = mtime;
        }

        self.total_size += size;

        // v2: pad every file to a piece boundary (tail padding, as the BEP 52 reference does)
        if self.v2 && self.piece_length > 0 && self.total_size % self.piece_length as u64 != 0 {
            let pad_size = self.piece_length as u64 - self.total_size % self.piece_length as u64;
            if self.total_size > MAX_FILE_OFFSET - pad_size {
                return Err(FileStorageError::InvalidLength);
            }
            self.push_pad_file(self.total_size, pad_size);
            self.total_size += pad_size;
        }
        Ok(())
    }

    fn push_pad_file(&mut self, offset: u64, size: u64) {
        let mut pad = Entry::new();
        pad.offset = offset;
        pad.size = size;
        pad.path_index = self.get_or_add_path(".pad");
        pad.name = self.own_name(&size.to_string());
        pad.flags = FileFlags::PAD_FILE;
        self.files.push(pad);
    }

    fn own_name(&mut self, n: &str) -> Name {
        if n.is_empty() {
            return Name::Empty;
        }
        let off = self.owned_names.len() as u32;
        self.owned_names.push_str(n);
        Name::Owned { off, len: n.len() as u32 }
    }

    fn name_of(&self, e: &Entry) -> &str {
        match e.name {
            Name::Empty => "",
            Name::Owned { off, len } => &self.owned_names[off as usize..(off + len) as usize],
            Name::Borrowed { off, len } => {
                let buf = self.buffer.as_deref().expect("borrowed name without a buffer");
                std::str::from_utf8(&buf[off as usize..(off + len) as usize]).unwrap_or("")
            }
        }
    }

    /// `path` includes the torrent name (or is absolute).
    fn update_path_index(&mut self, e: &mut Entry, path: &str, set_name: bool) {
        if is_complete(path) {
            e.name = self.own_name(path);
            e.path_index = PATH_IS_ABSOLUTE;
            return;
        }
        let (mut branch, leaf) = rsplit_path(path);
        if branch.is_empty() {
            if set_name {
                e.name = self.own_name(leaf);
            }
            e.path_index = NO_PATH;
            return;
        }
        // strip the torrent name from the stored directory, if present
        let (first, rest) = lsplit_path(branch);
        if first == self.name {
            branch = rest.trim_start_matches('/');
            e.no_root_dir = false;
        } else {
            e.no_root_dir = true;
        }
        e.path_index = self.get_or_add_path(branch);
        if set_name {
            e.name = self.own_name(leaf);
        }
    }

    fn get_or_add_path(&mut self, path: &str) -> u32 {
        // files are usually grouped by directory: check the most recent one first
        if let Some(last) = self.paths.last() {
            if last == path {
                return (self.paths.len() - 1) as u32;
            }
        }
        if let Some(&i) = self.path_index.get(path) {
            return i;
        }
        let i = self.paths.len() as u32;
        self.paths.push(path.to_owned());
        self.path_index.insert(path.to_owned(), i);
        i
    }

    pub fn rename_file(&mut self, index: usize, new_filename: &str) {
        let mut e = self.files[index].clone();
        self.update_path_index(&mut e, new_filename, true);
        self.files[index] = e;
    }

    /// Removes a trailing pad file (hybrid torrents made by libtorrent 2.0.0–2.0.7 lack one).
    pub fn remove_tail_padding(&mut self) {
        let mut f = self.files.len();
        while f > 0 {
            f -= 1;
            if self.files[f].size == 0 {
                continue; // empty files and symlinks
            }
            if self.files[f].flags.contains(FileFlags::PAD_FILE) {
                self.total_size -= self.files[f].size;
                self.files.remove(f);
                for later in &mut self.files[f..] {
                    later.offset = self.total_size;
                }
            }
            return;
        }
    }

    pub fn hash(&self, index: usize) -> Sha1Hash {
        self.file_hashes.get(index).copied().unwrap_or_default()
    }

    /// v2 merkle root of the file (zero if none).
    pub fn root(&self, index: usize) -> Sha256Hash {
        self.roots.get(index).copied().unwrap_or_default()
    }

    /// The v2 merkle root as bytes, or empty if the file has none (libtorrent `root_ptr`).
    pub fn root_bytes(&self, index: usize) -> &[u8] {
        match self.roots.get(index) {
            Some(r) if !r.is_zero() => &r.0,
            _ => &[],
        }
    }

    /// Symlink target relative to the save path (empty if not a symlink).
    pub fn symlink(&self, index: usize) -> String {
        let e = &self.files[index];
        if e.symlink_index == NOT_A_SYMLINK {
            return String::new();
        }
        combine_path(&self.name, &self.symlinks[e.symlink_index as usize])
    }

    /// Symlink target relative to the torrent root.
    pub fn internal_symlink(&self, index: usize) -> &str {
        &self.symlinks[self.files[index].symlink_index as usize]
    }

    pub fn mtime(&self, index: usize) -> i64 {
        self.mtime.get(index).copied().unwrap_or(0)
    }

    /// The file's full path, joined onto `save_path` (ignored for absolute paths).
    pub fn file_path(&self, index: usize, save_path: &str) -> String {
        let e = &self.files[index];
        let name = self.name_of(e);
        match e.path_index {
            PATH_IS_ABSOLUTE => name.to_owned(),
            NO_PATH => combine_path(save_path, name),
            pi => {
                let p = &self.paths[pi as usize];
                let mut ret =
                    String::with_capacity(save_path.len() + self.name.len() + p.len() + name.len() + 3);
                ret.push_str(save_path);
                if !e.no_root_dir {
                    append_path(&mut ret, &self.name);
                }
                append_path(&mut ret, p);
                append_path(&mut ret, name);
                ret
            }
        }
    }

    /// Path relative to the torrent root (no torrent name).
    fn internal_file_path(&self, index: usize) -> String {
        let e = &self.files[index];
        let name = self.name_of(e);
        if e.path_index != PATH_IS_ABSOLUTE && e.path_index != NO_PATH {
            combine_path(&self.paths[e.path_index as usize], name)
        } else {
            name.to_owned()
        }
    }

    pub fn file_name(&self, index: usize) -> &str {
        self.name_of(&self.files[index])
    }

    /// True if the name points into the info-section buffer rather than being a copy.
    pub fn owns_name(&self, index: usize) -> bool {
        !matches!(self.files[index].name, Name::Borrowed { .. })
    }

    pub fn file_size(&self, index: usize) -> u64 {
        self.files[index].size
    }

    pub fn pad_file_at(&self, index: usize) -> bool {
        self.files[index].flags.contains(FileFlags::PAD_FILE)
    }

    pub fn file_offset(&self, index: usize) -> u64 {
        self.files[index].offset
    }

    pub fn file_flags(&self, index: usize) -> FileFlags {
        self.files[index].flags
    }

    pub fn file_absolute_path(&self, index: usize) -> bool {
        self.files[index].path_index == PATH_IS_ABSOLUTE
    }

    /// v2: pieces in the file (files are piece-aligned).
    pub fn file_num_pieces(&self, index: usize) -> u32 {
        let f = &self.files[index];
        if f.size == 0 {
            return 0;
        }
        debug_assert!(!f.flags.contains(FileFlags::PAD_FILE) && f.offset % self.piece_length as u64 == 0);
        f.size.div_ceil(self.piece_length as u64) as u32
    }

    /// v2: 16 KiB blocks in the file.
    pub fn file_num_blocks(&self, index: usize) -> u32 {
        let f = &self.files[index];
        if f.size == 0 {
            return 0;
        }
        f.size.div_ceil(DEFAULT_BLOCK_SIZE as u64) as u32
    }

    /// v2: index of the first piece-layer node in the file's merkle tree.
    pub fn file_first_piece_node(&self, index: usize) -> u32 {
        let leafs = merkle_num_leafs(self.file_num_pieces(index));
        merkle_num_nodes(leafs) - leafs
    }

    /// v2: index of the first block-layer node in the file's merkle tree.
    pub fn file_first_block_node(&self, index: usize) -> u32 {
        let leafs = merkle_num_leafs(self.file_num_blocks(index));
        merkle_num_nodes(leafs) - leafs
    }

    /// The file containing byte `offset` of the torrent.
    pub fn file_index_at_offset(&self, offset: u64) -> usize {
        debug_assert!(offset < self.total_size);
        self.files.partition_point(|f| f.offset <= offset) - 1
    }

    pub fn file_index_at_piece(&self, piece: u32) -> usize {
        self.file_index_at_offset(piece as u64 * self.piece_length as u64)
    }

    pub fn file_index_for_root(&self, root: &Sha256Hash) -> Option<usize> {
        (0..self.files.len()).find(|&i| self.root(i) == *root)
    }

    pub fn piece_index_at_file(&self, f: usize) -> u32 {
        (self.files[f].offset / self.piece_length as u64) as u32
    }

    /// The file ranges covered by `size` bytes at `offset` within `piece`.
    pub fn map_block(&self, piece: u32, offset: u64, mut size: u64) -> Vec<FileSlice> {
        let mut ret = Vec::new();
        if self.files.is_empty() {
            return ret;
        }
        let target = piece as u64 * self.piece_length as u64 + offset;
        // in case the range runs past the end, clamp it
        if target + size > self.total_size {
            size = self.total_size.saturating_sub(target);
        }
        let mut idx = self.files.partition_point(|f| f.offset <= target) - 1;
        let mut file_offset = target - self.files[idx].offset;
        while size > 0 && idx < self.files.len() {
            let fsize = self.files[idx].size;
            if file_offset < fsize {
                let n = (fsize - file_offset).min(size);
                ret.push(FileSlice { file_index: idx, offset: file_offset, size: n });
                size -= n;
                // the range continues at the start of the next file
                file_offset = 0;
            } else {
                // empty file (or one ending exactly at the target)
                file_offset -= fsize;
            }
            idx += 1;
        }
        ret
    }

    /// The piece range that holds `size` bytes at `offset` in `file`.
    pub fn map_file(&self, file: usize, offset: u64, size: u32) -> PeerRequest {
        let end = PeerRequest { piece: self.num_pieces, start: 0, length: 0 };
        if file >= self.files.len() {
            return end;
        }
        let off = offset + self.files[file].offset;
        if off >= self.total_size {
            return end;
        }
        let pl = self.piece_length as u64;
        let mut length = size;
        if off + size as u64 > self.total_size {
            length = (self.total_size - off) as u32;
        }
        PeerRequest { piece: (off / pl) as u32, start: (off % pl) as u32, length }
    }

    /// CRC32C of the lower-cased full path (libtorrent `file_path_hash`), for case-insensitive
    /// collision detection.
    pub fn file_path_hash(&self, index: usize, save_path: &str) -> u32 {
        let e = &self.files[index];
        let mut crc = Crc32c::new();
        let lower =
            |crc: &mut Crc32c, s: &str| s.bytes().for_each(|c| crc.update_byte(c.to_ascii_lowercase()));
        let with_sep = |crc: &mut Crc32c, s: &str| {
            if !s.is_empty() {
                lower(crc, s);
                crc.update_byte(b'/');
            }
        };
        let name = self.name_of(e);
        match e.path_index {
            PATH_IS_ABSOLUTE => lower(&mut crc, name),
            NO_PATH => {
                with_sep(&mut crc, save_path);
                lower(&mut crc, name);
            }
            pi => {
                with_sep(&mut crc, save_path);
                if !e.no_root_dir {
                    with_sep(&mut crc, &self.name);
                }
                with_sep(&mut crc, &self.paths[pi as usize]);
                lower(&mut crc, name);
            }
        }
        crc.checksum()
    }

    /// Hashes of every directory (and parent directory) in the torrent, in the same form as
    /// [`FileStorage::file_path_hash`] with an empty save path.
    pub fn all_path_hashes(&self, table: &mut HashSet<u32>) {
        let mut crc = Crc32c::new();
        if !self.name.is_empty() {
            self.name.bytes().for_each(|c| crc.update_byte(c.to_ascii_lowercase()));
            crc.update_byte(b'/');
        }
        for p in &self.paths {
            if p.is_empty() {
                continue;
            }
            let mut local = crc;
            for c in p.bytes() {
                if c == b'/' {
                    table.insert(local.checksum());
                }
                local.update_byte(c.to_ascii_lowercase());
            }
            table.insert(local.checksum());
        }
    }

    /// Sorts files by path, drops existing pad files and pads every file to a piece boundary
    /// (libtorrent `canonicalize`, used when creating torrents).
    pub fn canonicalize(&mut self) {
        self.canonicalize_impl(false);
    }

    pub fn canonicalize_impl(&mut self, backwards_compatible: bool) {
        assert!(self.piece_length >= 16 * 1024);
        let mut order: Vec<usize> = (0..self.files.len()).filter(|&i| !self.pad_file_at(i)).collect();
        order.sort_by(|&l, &r| {
            let (lf, rf) = (&self.files[l], &self.files[r]);
            if lf.path_index != rf.path_index {
                let lp = self.paths.get(lf.path_index as usize).map_or("", String::as_str);
                let rp = self.paths.get(rf.path_index as usize).map_or("", String::as_str);
                let c = path_compare(lp, self.name_of(lf), rp, self.name_of(rf));
                if c != std::cmp::Ordering::Equal {
                    return c;
                }
            }
            self.name_of(lf).cmp(self.name_of(rf))
        });

        let old_files = std::mem::take(&mut self.files);
        let old_hashes = std::mem::take(&mut self.file_hashes);
        let old_roots = std::mem::take(&mut self.roots);
        let old_mtime = std::mem::take(&mut self.mtime);
        let pl = self.piece_length as u64;
        let multi = order.len() > 1;
        let mut off = 0u64;

        let push_pad = |fs: &mut FileStorage, next_size: u64, off: &mut u64| {
            if *off % pl != 0 && next_size > 0 {
                let pad = pl - *off % pl;
                fs.push_pad_file(*off, pad);
                *off += pad;
                if !old_hashes.is_empty() {
                    fs.file_hashes.push(Sha1Hash::default());
                }
                if !old_roots.is_empty() {
                    fs.roots.push(Sha256Hash::default());
                }
                if !old_mtime.is_empty() {
                    fs.mtime.push(0);
                }
            }
        };

        for &i in &order {
            let mut e = old_files[i].clone();
            if backwards_compatible {
                push_pad(self, e.size, &mut off);
            }
            e.offset = off;
            off += e.size;
            let size = e.size;
            self.files.push(e);
            if !old_hashes.is_empty() {
                self.file_hashes.push(old_hashes.get(i).copied().unwrap_or_default());
            }
            if !old_roots.is_empty() {
                self.roots.push(old_roots.get(i).copied().unwrap_or_default());
            }
            if !old_mtime.is_empty() {
                self.mtime.push(old_mtime.get(i).copied().unwrap_or(0));
            }
            // single-file torrents are never padded (so they can be hybrid)
            if !backwards_compatible && multi {
                push_pad(self, size, &mut off);
            }
        }
        self.total_size = off;
    }

    /// Makes every symlink point inside the torrent; invalid ones point to themselves.
    pub fn sanitize_symlinks(&mut self) {
        let mut file_map: Option<HashMap<String, usize>> = None;
        let mut dir_map: Option<HashSet<String>> = None;
        let mut dir_links: HashMap<String, String> = HashMap::new();
        let mut to_validate: Vec<usize> = Vec::new();

        for i in 0..self.files.len() {
            if !self.file_flags(i).contains(FileFlags::SYMLINK) {
                continue;
            }
            let fm = file_map.get_or_insert_with(|| {
                (0..self.files.len()).map(|j| (self.internal_file_path(j), j)).collect()
            });
            let si = self.files[i].symlink_index as usize;
            let target = self.symlinks[si].clone();

            if is_complete(&target) {
                // absolute targets are never allowed
                self.symlinks[si] = self.internal_file_path(i);
                continue;
            }
            if let Some(&t) = fm.get(&target) {
                if self.file_flags(t).contains(FileFlags::SYMLINK) {
                    dir_links.insert(self.internal_file_path(i), target);
                }
                continue;
            }
            let dm = dir_map.get_or_insert_with(|| {
                let mut s = HashSet::new();
                for p in &self.paths {
                    let mut pv = p.as_str();
                    while !pv.is_empty() {
                        s.insert(pv.to_owned());
                        pv = rsplit_path(pv).0;
                    }
                }
                s
            });
            if dm.contains(&target) {
                dir_links.insert(self.internal_file_path(i), target);
                continue;
            }
            // backwards compatibility: targets relative to the link's own directory
            let pi = self.files[i].path_index;
            if pi < PATH_IS_ABSOLUTE {
                let rel = combine_path(&self.paths[pi as usize], &target);
                if self.paths.contains(&rel) || dm.contains(&rel) {
                    self.symlinks[si] = rel.clone();
                    dir_links.insert(self.internal_file_path(i), rel);
                    continue;
                }
                if let Some(&t) = fm.get(&rel) {
                    self.symlinks[si] = rel.clone();
                    if self.file_flags(t).contains(FileFlags::SYMLINK) {
                        dir_links.insert(self.internal_file_path(i), rel);
                    }
                    continue;
                }
            }
            // unknown: assume it might be a directory link and validate in a second pass
            dir_links.insert(self.internal_file_path(i), self.symlinks[si].clone());
            to_validate.push(i);
        }

        let empty_files = HashMap::new();
        let empty_dirs = HashSet::new();
        let fm = file_map.as_ref().unwrap_or(&empty_files);
        let dm = dir_map.as_ref().unwrap_or(&empty_dirs);
        for i in to_validate {
            let si = self.files[i].symlink_index as usize;
            if !resolve_symlink(&self.symlinks[si], fm, dm, &dir_links) {
                self.symlinks[si] = self.internal_file_path(i);
            }
        }
    }
}

/// Follows directory symlinks element by element; true if `target` ends at a real file or
/// directory. Each link may be traversed once (cycles fail).
fn resolve_symlink(
    target: &str,
    files: &HashMap<String, usize>,
    dirs: &HashSet<String>,
    dir_links: &HashMap<String, String>,
) -> bool {
    let mut target = target.to_owned();
    let mut traversed: HashSet<String> = HashSet::new();
    let mut branch_len = lsplit_path(&target).0.len();
    while branch_len < target.len() {
        let branch = target[..branch_len].to_owned();
        if !dirs.contains(&branch) {
            let Some(link) = dir_links.get(&branch) else { return false };
            if !traversed.insert(branch) {
                return false;
            }
            target = combine_path(link, &target[branch_len + 1..]);
            branch_len = lsplit_path(&target).0.len();
            continue;
        }
        branch_len = lsplit_path_at(&target, branch_len + 1).0.len();
    }
    files.contains_key(&target) || dirs.contains(&target)
}

/// libtorrent `aux::files_compatible`: hybrid torrents must describe the same non-empty,
/// non-pad files at the same offsets in their v1 and v2 file lists.
pub fn files_compatible(lhs: &FileStorage, rhs: &FileStorage) -> bool {
    if lhs.num_files() != rhs.num_files()
        || lhs.total_size() != rhs.total_size()
        || lhs.piece_length() != rhs.piece_length()
    {
        return false;
    }
    for i in 0..lhs.num_files() {
        let lr = !lhs.pad_file_at(i) && lhs.file_size(i) > 0;
        let rr = !rhs.pad_file_at(i) && rhs.file_size(i) > 0;
        if lr != rr {
            return false;
        }
        if !lr {
            continue;
        }
        if lhs.file_size(i) != rhs.file_size(i)
            || lhs.file_path(i, "") != rhs.file_path(i, "")
            || lhs.file_offset(i) != rhs.file_offset(i)
        {
            return false;
        }
        if lhs.file_flags(i).contains(FileFlags::SYMLINK) && lhs.symlink(i) != rhs.symlink(i) {
            return false;
        }
    }
    true
}

/// Pieces entirely inside `file`: `[begin, end)`.
pub fn file_piece_range_exclusive(fs: &FileStorage, file: usize) -> (u32, u32) {
    let range = fs.map_file(file, 0, 1);
    let piece_size = fs.piece_length() as u64;
    let begin = if range.start == 0 { range.piece } else { range.piece + 1 };
    let end = if file == fs.num_files() - 1 {
        fs.num_pieces()
    } else {
        ((range.piece as u64 * piece_size + range.start as u64 + fs.file_size(file) + 1) / piece_size) as u32
    };
    (begin, end)
}

/// Pieces that overlap `file`: `[begin, end)`.
pub fn file_piece_range_inclusive(fs: &FileStorage, file: usize) -> (u32, u32) {
    let range = fs.map_file(file, 0, 1);
    let piece_size = fs.piece_length() as u64;
    let end = ((range.piece as u64 * piece_size + range.start as u64 + fs.file_size(file)).saturating_sub(1)
        / piece_size
        + 1) as u32;
    (range.piece, end)
}

pub fn calc_num_pieces(fs: &FileStorage) -> u32 {
    fs.total_size().div_ceil(fs.piece_length() as u64) as u32
}

/// Bytes that end up on disk (excludes pad files).
pub fn size_on_disk(fs: &FileStorage) -> u64 {
    (0..fs.num_files()).filter(|&i| !fs.pad_file_at(i)).map(|i| fs.file_size(i)).sum()
}

#[cfg(test)]
mod tests {
    //! Ported from libtorrent-rasterbar 2.0.11 test/test_file_storage.cpp.
    use super::*;

    const NONE: FileFlags = FileFlags::empty();

    fn add(fs: &mut FileStorage, path: &str, size: u64) {
        fs.add_file(path, size, NONE, 0, "", None).unwrap();
    }

    fn add_v2(fs: &mut FileStorage, path: &str, size: u64, root: &[u8; 32]) {
        fs.add_file(path, size, NONE, 0, "", Some(Sha256Hash(*root))).unwrap();
    }

    fn add_link(fs: &mut FileStorage, path: &str, target: &str) {
        fs.add_file(path, 0, FileFlags::SYMLINK, 0, target, None).unwrap();
    }

    const ROOT: &[u8; 32] = b"01234567890123456789012345678901";

    fn setup_test_storage() -> FileStorage {
        let mut st = FileStorage::new();
        add(&mut st, "test/a", 10000);
        add(&mut st, "test/b", 20000);
        add(&mut st, "test/c/a", 30000);
        add(&mut st, "test/c/b", 40000);
        st.set_piece_length(0x4000);
        st.set_num_pieces(calc_num_pieces(&st));
        assert_eq!(
            [st.file_name(0), st.file_name(1), st.file_name(2), st.file_name(3)],
            ["a", "b", "a", "b"]
        );
        assert_eq!(st.name(), "test");
        assert_eq!(st.file_path(0, ""), "test/a");
        assert_eq!(st.file_path(2, ""), "test/c/a");
        assert_eq!(st.file_path(3, ""), "test/c/b");
        assert_eq!((0..4).map(|i| st.file_offset(i)).collect::<Vec<_>>(), [0, 10000, 30000, 60000]);
        assert_eq!(st.total_size(), 100000);
        assert_eq!(st.num_pieces(), 100000u32.div_ceil(0x4000));
        st
    }

    #[test]
    fn coalesce_path() {
        let mut st = FileStorage::new();
        st.set_piece_length(0x4000);
        add(&mut st, "test/a", 10000);
        assert_eq!(st.paths(), [""]);
        add(&mut st, "test/b", 20000);
        assert_eq!(st.paths(), [""]);
        add(&mut st, "test/c/a", 30000);
        assert_eq!(st.paths(), ["", "c"]);
        add(&mut st, "test/c/b", 40000);
        assert_eq!(st.paths(), ["", "c"]);
        st.canonicalize();
        assert_eq!(st.paths(), ["", "c", ".pad"]);
    }

    #[test]
    fn rename_file() {
        let mut st = setup_test_storage();
        st.rename_file(0, "test/c/d");
        assert_eq!(st.file_path(0, "."), "test/c/d");
        assert_eq!(st.file_path(0, ""), "test/c/d");
        let abs = if cfg!(windows) { "c:\\tmp\\a" } else { "/tmp/a" };
        st.rename_file(0, abs);
        assert_eq!(st.file_path(0, "."), abs);
        st.rename_file(0, "test__/a");
        assert_eq!(st.file_path(0, "."), "test__/a");
    }

    #[test]
    fn set_name() {
        let mut st = setup_test_storage();
        st.set_name("test_2");
        assert_eq!(st.file_path(0, "."), "test_2/a");
    }

    #[test]
    fn rename_file2() {
        let mut st = FileStorage::new();
        add(&mut st, "a", 10000);
        assert_eq!(st.file_path(0, ""), "a");
        st.rename_file(0, "test/c/d");
        assert_eq!(st.file_path(0, "."), "test/c/d");
        assert_eq!(st.file_path(0, ""), "test/c/d");
        let abs = if cfg!(windows) { "c:\\tmp\\a" } else { "/tmp/a" };
        st.rename_file(0, abs);
        assert_eq!(st.file_path(0, "."), abs);
        assert_eq!(st.file_path(0, "/usr/local/temp"), abs);
        st.rename_file(0, "tmp/a");
        assert_eq!(st.file_path(0, "."), "tmp/a");
    }

    #[test]
    fn pointer_offset() {
        let buf: Arc<[u8]> = Arc::from(&b"test1fooba"[..]);
        let mut st = FileStorage::with_buffer(buf);
        st.set_piece_length(16 * 1024);
        let fh = Sha1Hash(*b"01234567890123456789");
        let root = Sha256Hash(*b"01234567890123456789012345678912");
        st.add_file_borrow(Some((0, 5)), "test-torrent-1/test1", 10, NONE, Some(fh), 0, "", Some(root))
            .unwrap();
        assert_eq!(st.file_name(0), "test1");
        assert!(!st.owns_name(0));
        assert_eq!(st.hash(0), fh);
        assert_eq!(st.root(0), root);
        assert_eq!(st.file_path(0, ""), "test-torrent-1/test1");
        assert_eq!(st.file_path(0, "tmp"), "tmp/test-torrent-1/test1");
    }

    #[test]
    fn invalid_paths() {
        let mut st = FileStorage::new();
        st.set_piece_length(16 * 1024);
        add(&mut st, "+///(", 10);
        assert_eq!(st.file_name(0), "(");
        assert_eq!(st.file_path(0, ""), "+/(");

        let mut st = FileStorage::new();
        st.set_piece_length(16 * 1024);
        add(&mut st, "+///+//(", 10);
        assert_eq!(st.file_name(0), "(");
        assert_eq!(st.file_path(0, ""), "+/+/(");
    }

    #[test]
    fn map_file() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(512);
        for (name, size) in
            [("test1", 17), ("test2", 612), ("test3", 0), ("test4", 0), ("test5", 3253), ("test6", 841)]
        {
            add(&mut fs, &format!("temp_storage/{name}.tmp"), size);
        }
        fs.set_num_pieces(calc_num_pieces(&fs));
        assert_eq!(fs.map_file(0, 0, 10), PeerRequest { piece: 0, start: 0, length: 10 });
        assert_eq!(fs.map_file(5, 0, 10), PeerRequest { piece: 7, start: 298, length: 10 });
        assert_eq!(fs.map_file(5, 0, 1000), PeerRequest { piece: 7, start: 298, length: 841 });
    }

    #[test]
    fn file_path_hash() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(512);
        add(&mut fs, "temp_storage/Foo", 17);
        add(&mut fs, "temp_storage/foo", 612);
        assert_eq!(fs.file_path_hash(0, "a"), fs.file_path_hash(1, "a"));
    }

    #[test]
    fn canonicalize_pad() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(0x4000);
        add(&mut fs, "s/2", 0x7000);
        add(&mut fs, "s/1", 1);
        add(&mut fs, "s/3", 0x7001);
        fs.canonicalize();
        assert_eq!(fs.num_files(), 6);
        let expect = [
            (1, false, Some("1")),
            (0x4000 - 1, true, None),
            (0x7000, false, Some("2")),
            (0x8000 - 0x7000, true, None),
            (0x7001, false, Some("3")),
            (0x8000 - 0x7001, true, None),
        ];
        for (i, (size, pad, name)) in expect.into_iter().enumerate() {
            assert_eq!(fs.file_size(i), size, "file {i}");
            assert_eq!(fs.pad_file_at(i), pad, "file {i}");
            if let Some(n) = name {
                assert_eq!(fs.file_name(i), n);
            }
        }
    }

    #[test]
    fn canonicalize_path() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(0x4000);
        add(&mut fs, "b/2/a", 0x4000);
        add(&mut fs, "b/1/a", 0x4000);
        add(&mut fs, "b/3/a", 0x4000);
        add(&mut fs, "b/11", 0x4000);
        fs.canonicalize();
        assert_eq!(fs.num_files(), 4);
        let paths: Vec<String> = (0..4).map(|i| fs.file_path(i, "")).collect();
        assert_eq!(paths, ["b/1/a", "b/11", "b/2/a", "b/3/a"]);
    }

    fn piece_range_storage() -> FileStorage {
        let ps = 16;
        let mut fs = FileStorage::new();
        fs.set_piece_length(ps);
        add(&mut fs, "temp_storage/0", ps as u64);
        add(&mut fs, "temp_storage/1", ps as u64 * 4 + 1);
        add(&mut fs, "temp_storage/2", ps as u64 * 4 - 1);
        fs.set_num_pieces(calc_num_pieces(&fs));
        fs
    }

    #[test]
    fn piece_ranges() {
        let fs = piece_range_storage();
        assert_eq!(file_piece_range_exclusive(&fs, 0), (0, 1));
        assert_eq!(file_piece_range_exclusive(&fs, 1), (1, 5));
        assert_eq!(file_piece_range_exclusive(&fs, 2), (6, 9));
        assert_eq!(file_piece_range_inclusive(&fs, 0), (0, 1));
        assert_eq!(file_piece_range_inclusive(&fs, 1), (1, 6));
        assert_eq!(file_piece_range_inclusive(&fs, 2), (5, 9));

        let ps = 0x4000u64;
        let mut fs = FileStorage::new();
        fs.set_piece_length(ps as u32);
        add(&mut fs, "temp_storage/0", ps * 3);
        add(&mut fs, "temp_storage/1", ps * 3 + 0x30);
        fs.set_num_pieces(calc_num_pieces(&fs));
        assert_eq!(file_piece_range_inclusive(&fs, 0), (0, 3));
        assert_eq!(file_piece_range_inclusive(&fs, 1), (3, 7));
        assert_eq!(file_piece_range_exclusive(&fs, 0), (0, 3));
        assert_eq!(file_piece_range_exclusive(&fs, 1), (3, 7));
    }

    #[test]
    fn piece_sizes() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(1024);
        add(&mut fs, "0", 100);
        fs.set_num_pieces(calc_num_pieces(&fs));
        assert_eq!(fs.piece_size(0), 100);

        let mut fs = FileStorage::new();
        fs.set_piece_length(1024);
        add(&mut fs, "0", 2000);
        fs.set_num_pieces(calc_num_pieces(&fs));
        assert_eq!(fs.piece_size(0), 1024);
        assert_eq!(fs.piece_size(1), 2000 - 1024);
    }

    fn five_small_files() -> FileStorage {
        let mut fs = FileStorage::new();
        fs.set_piece_length(1024);
        for i in 0..5 {
            add(&mut fs, &format!("test/{i}"), i + 1);
        }
        fs.set_num_pieces(calc_num_pieces(&fs));
        fs
    }

    #[test]
    fn file_index_at_offset() {
        let fs = five_small_files();
        let expect = [0, 1, 1, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 4];
        for (offset, f) in expect.into_iter().enumerate() {
            assert_eq!(fs.file_index_at_offset(offset as u64), f);
        }
    }

    #[test]
    fn map_block_start() {
        let fs = five_small_files();
        for (len, f) in [0, 1, 2, 2, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 5].into_iter().enumerate() {
            let map = fs.map_block(0, 0, len as u64);
            assert_eq!(map.len(), f, "len {len}");
            for (i, s) in map.iter().enumerate() {
                assert_eq!((s.file_index, s.offset), (i, 0));
            }
            assert_eq!(map.iter().map(|s| s.size).sum::<u64>(), len as u64);
        }
    }

    #[test]
    fn map_block_mid() {
        let fs = five_small_files();
        for (offset, f) in [0, 1, 1, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 4].into_iter().enumerate() {
            let map = fs.map_block(0, offset as u64, 1);
            assert_eq!(map.len(), 1);
            assert_eq!(map[0].file_index, f);
            assert!(map[0].offset <= offset as u64);
            assert_eq!(map[0].size, 1);
        }
    }

    #[test]
    fn sanitize_symlinks() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(1024);
        let invalid_abs = if cfg!(windows) { "C:\\invalid\\target\\path" } else { "/invalid/target/path" };
        add_link(&mut fs, "test/0", invalid_abs);
        add_link(&mut fs, "test/1", "ZZ"); // no such file
        add_link(&mut fs, "test/2", "B/B/ZZ"); // no such file
        add_link(&mut fs, "test/3", "0"); // ok
        add_link(&mut fs, "test/4", "A"); // ok
        add_link(&mut fs, "test/5", "4/B"); // advanced, ok
        add_link(&mut fs, "test/6", "5/C"); // advanced, ok
        add_link(&mut fs, "test/7", "4/B/C/ZZ"); // not ok
        add(&mut fs, "test/A/B/C", 10000);
        fs.set_num_pieces(calc_num_pieces(&fs));
        fs.sanitize_symlinks();
        let links: Vec<String> = (0..8).map(|i| fs.symlink(i)).collect();
        assert_eq!(
            links,
            ["test/0", "test/1", "test/2", "test/0", "test/A", "test/4/B", "test/5/C", "test/7"]
        );
    }

    #[test]
    fn sanitize_symlinks_single_file() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(1024);
        add(&mut fs, "test", 1);
        fs.set_num_pieces(calc_num_pieces(&fs));
        fs.sanitize_symlinks();
        assert_eq!(fs.file_path(0, ""), "test");
    }

    #[test]
    fn sanitize_symlinks_cascade() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(1024);
        add_link(&mut fs, "test/0", "1/ZZ");
        for (i, t) in ["2", "3", "4", "5", "6", "7", "A"].into_iter().enumerate() {
            add_link(&mut fs, &format!("test/{}", i + 1), t);
        }
        add_link(&mut fs, "test/no-exist", "1/ZZZ");
        add(&mut fs, "test/A/ZZ", 10000);
        fs.set_num_pieces(calc_num_pieces(&fs));
        fs.sanitize_symlinks();
        let links: Vec<String> = (0..9).map(|i| fs.symlink(i)).collect();
        assert_eq!(
            links,
            [
                "test/1/ZZ",
                "test/2",
                "test/3",
                "test/4",
                "test/5",
                "test/6",
                "test/7",
                "test/A",
                "test/no-exist"
            ]
        );
    }

    #[test]
    fn sanitize_symlinks_circular() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(1024);
        add_link(&mut fs, "test/0", "1");
        add_link(&mut fs, "test/1", "0");
        add_link(&mut fs, "test/2", "0/ZZ"); // resolving this loops forever
        add(&mut fs, "test/A/ZZ", 10000);
        fs.set_num_pieces(calc_num_pieces(&fs));
        fs.sanitize_symlinks();
        assert_eq!(fs.symlink(0), "test/1");
        assert_eq!(fs.symlink(1), "test/0");
        assert_eq!(fs.symlink(2), "test/2");
    }

    #[test]
    fn query_symlinks() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(1024);
        for i in 0..4 {
            add_link(&mut fs, &format!("test/{i}"), &i.to_string());
        }
        let l: HashSet<String> = (0..4).map(|i| fs.symlink(i)).collect();
        assert_eq!(l.len(), 4);

        let mut fs = FileStorage::new();
        fs.set_piece_length(1024);
        for i in 0..4 {
            add(&mut fs, &format!("test/{i}"), 10);
        }
        assert!((0..4).all(|i| fs.symlink(i).is_empty()));
    }

    fn two(a: (&str, u64, FileFlags), b: (&str, u64, FileFlags), piece_length: u32) -> FileStorage {
        let mut fs = FileStorage::new();
        fs.set_piece_length(piece_length);
        for (p, s, f) in [a, b] {
            fs.add_file(p, s, f, 0, "", None).unwrap();
        }
        fs
    }

    #[test]
    fn files_compatible_cases() {
        let base = two(("test/0", 1, NONE), ("test/1", 2, NONE), 0x4000);
        assert!(files_compatible(&base, &two(("test/0", 1, NONE), ("test/1", 2, NONE), 0x4000)));

        let mut one = FileStorage::new();
        one.set_piece_length(0x4000);
        add(&mut one, "test/0", 3);
        assert!(!files_compatible(&base, &one), "num files");
        assert!(!files_compatible(&two(("test/0", 2, NONE), ("test/1", 1, NONE), 0x4000), &base), "size");
        assert!(!files_compatible(&two(("test/1", 1, NONE), ("test/0", 2, NONE), 0x4000), &base), "name");
        assert!(
            files_compatible(&two(("test/0", 1, NONE), ("test/1", 2, FileFlags::HIDDEN), 0x4000), &base),
            "hidden"
        );
        assert!(
            !files_compatible(&two(("test/0", 1, NONE), ("test/1", 2, FileFlags::PAD_FILE), 0x4000), &base),
            "pad"
        );
        assert!(
            !files_compatible(&two(("test/0", 1, NONE), ("test/1", 2, NONE), 0x8000), &base),
            "piece size"
        );
    }

    #[test]
    fn files_compatible_empty_order_mtime_symlinks() {
        let mk = |names: [&str; 4]| {
            let mut fs = FileStorage::new();
            fs.set_piece_length(0x4000);
            add(&mut fs, names[0], 1);
            for n in &names[1..] {
                add(&mut fs, n, 0);
            }
            fs
        };
        assert!(files_compatible(
            &mk(["test/0", "test/1", "test/2", "test/3"]),
            &mk(["test/0", "test/3", "test/2", "test/1"])
        ));

        let mut a = FileStorage::new();
        a.set_piece_length(0x4000);
        a.add_file("test/0", 1, NONE, 1234, "", None).unwrap();
        a.add_file("test/1", 2, NONE, 1235, "", None).unwrap();
        let mut b = FileStorage::new();
        b.set_piece_length(0x4000);
        b.add_file("test/0", 1, NONE, 1234, "", None).unwrap();
        b.add_file("test/1", 2, NONE, 1234, "", None).unwrap();
        assert!(files_compatible(&a, &b), "mtime");

        let link = |target: &str| {
            let mut fs = FileStorage::new();
            fs.set_piece_length(0x4000);
            add(&mut fs, "test/0", 1);
            fs.add_file("test/1", 2, FileFlags::SYMLINK, 0, target, None).unwrap();
            fs
        };
        assert!(!files_compatible(&link("test/0"), &link("test/1")));
        assert!(files_compatible(&link("test/0"), &link("test/0")));
    }

    #[test]
    fn remove_tail_padding() {
        let mut fs1 = FileStorage::new();
        fs1.set_piece_length(0x4000);
        add(&mut fs1, "test/0", 1);
        fs1.add_file("test/1", 2, FileFlags::PAD_FILE, 0, "", None).unwrap();
        add(&mut fs1, "test/2", 0);
        add(&mut fs1, "test/3", 0);
        fs1.remove_tail_padding();
        let mut fs2 = FileStorage::new();
        fs2.set_piece_length(0x4000);
        add(&mut fs2, "test/0", 1);
        add(&mut fs2, "test/2", 0);
        add(&mut fs2, "test/3", 0);
        assert!(files_compatible(&fs1, &fs2));

        let mut fs1 = FileStorage::new();
        fs1.set_piece_length(0x4000);
        add(&mut fs1, "test/0", 1);
        fs1.add_file("test/1", 2, FileFlags::PAD_FILE, 0, "", None).unwrap();
        fs1.remove_tail_padding();
        let mut fs2 = FileStorage::new();
        fs2.set_piece_length(0x4000);
        add(&mut fs2, "test/0", 1);
        assert!(files_compatible(&fs1, &fs2));

        let mk = || {
            let mut fs = FileStorage::new();
            fs.set_piece_length(0x4000);
            add(&mut fs, "test/0", 1);
            for i in 1..4 {
                add(&mut fs, &format!("test/{i}"), 0);
            }
            fs
        };
        let mut fs1 = mk();
        fs1.remove_tail_padding();
        assert!(files_compatible(&fs1, &mk()), "no-op");
    }

    #[test]
    fn large_files() {
        let int_max = i32::MAX as u64;
        let mut fs = FileStorage::new();
        fs.set_piece_length(0x4000);
        let r = fs.add_file("test/0", int_max / 2 * DEFAULT_BLOCK_SIZE as u64 + 1, NONE, 0, "", None);
        assert_eq!(r, Err(FileStorageError::FileTooLarge));
        let r = fs.add_file("test/0", int_max * DEFAULT_BLOCK_SIZE as u64 + 1, NONE, 0, "", None);
        assert_eq!(r, Err(FileStorageError::FileTooLarge));
        assert!(fs.add_file("test/0", int_max / 2 * DEFAULT_BLOCK_SIZE as u64, NONE, 0, "", None).is_ok());
    }

    #[test]
    fn large_offset() {
        let int_max = i32::MAX as u64;
        let mut fs = FileStorage::new();
        fs.set_piece_length(0x4000);
        for i in 0..16 {
            add(&mut fs, &format!("test/{i}"), int_max / 2 * DEFAULT_BLOCK_SIZE as u64);
        }
        // exceeds 2^48 - 1
        assert_eq!(fs.add_file("test/16", 262144, NONE, 0, "", None), Err(FileStorageError::InvalidLength));
        assert!(fs.add_file("test/8", 262143, NONE, 0, "", None).is_ok());
    }

    #[test]
    fn large_filename() {
        let mut fs = FileStorage::with_buffer(Arc::from(vec![b'0'; 1 << 12]));
        fs.set_piece_length(0x4000);
        let r = fs.add_file_borrow(Some((0, 1 << 12)), "test/path/", 10, NONE, None, 0, "", None);
        assert_eq!(r, Err(FileStorageError::FilenameTooLong));
    }

    #[test]
    fn piece_size2() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(0x8000);
        add_v2(&mut fs, "test/0", 0x5000, ROOT);
        fs.set_num_pieces(calc_num_pieces(&fs));
        assert_eq!(fs.num_pieces(), 1);
        assert_eq!(fs.piece_size2(0), 0x5000);
        add_v2(&mut fs, "test/1", 0x2000, ROOT);
        add_v2(&mut fs, "test/2", 0x8000, ROOT);
        fs.set_num_pieces(calc_num_pieces(&fs));
        assert_eq!(fs.num_pieces(), 3);
        assert_eq!(fs.piece_size2(2), 0x8000);
        add_v2(&mut fs, "test/3", 8, ROOT);
        fs.set_num_pieces(calc_num_pieces(&fs));
        assert_eq!(fs.num_pieces(), 4);
        add_v2(&mut fs, "test/4", 0x8001, ROOT);
        fs.set_num_pieces(calc_num_pieces(&fs));
        assert_eq!(fs.num_pieces(), 6);
        let sizes: Vec<u32> = (0..6).map(|p| fs.piece_size2(p)).collect();
        assert_eq!(sizes, [0x5000, 0x2000, 0x8000, 8, 0x8000, 1]);
    }

    fn v2_canonical() -> FileStorage {
        let mut fs = FileStorage::new();
        fs.set_piece_length(0x8000);
        for (i, size) in [0x5000, 0x2000, 0x8000, 0x8001, 1, 0].into_iter().enumerate() {
            add_v2(&mut fs, &format!("test/{i}"), size, ROOT);
        }
        fs.canonicalize();
        fs
    }

    #[test]
    fn file_num_blocks_and_pieces() {
        let fs = v2_canonical();
        let pads = [1, 3, 6, 8];
        for p in pads {
            assert!(fs.pad_file_at(p), "pad at {p}");
        }
        let blocks: Vec<(usize, u32)> =
            [0, 2, 4, 5, 7, 9].into_iter().map(|i| (i, fs.file_num_blocks(i))).collect();
        assert_eq!(blocks, [(0, 2), (2, 1), (4, 2), (5, 3), (7, 1), (9, 0)]);
        let pieces: Vec<(usize, u32)> =
            [0, 2, 4, 5, 7, 9].into_iter().map(|i| (i, fs.file_num_pieces(i))).collect();
        assert_eq!(pieces, [(0, 1), (2, 1), (4, 1), (5, 2), (7, 1), (9, 0)]);
    }

    #[test]
    fn file_first_piece_node() {
        let node = |piece_size: u32, file_size: u64| {
            let mut fs = FileStorage::new();
            fs.set_piece_length(piece_size);
            add_v2(&mut fs, "test/0", file_size, ROOT);
            fs.set_num_pieces(calc_num_pieces(&fs));
            fs.file_first_piece_node(0)
        };
        assert_eq!(node(0x10000, 0x500000), 127);
        assert_eq!(node(0x8000, 0x500000), 255);
        assert_eq!(node(0x4000, 0x500000), 511);
        assert_eq!(node(0x2000, 0x500000), 1023);
        assert_eq!(node(0x1000, 0x500000), 2047);
        assert_eq!(node(0x1000, 0x7fffff), 2047);
        assert_eq!(node(0x1000, 0x800000), 2047);
        assert_eq!(node(0x1000, 0x800001), 4095);
        assert_eq!(node(0x1000, 0x7fff), 7);
        assert_eq!(node(0x1000, 0x8000), 7);
        assert_eq!(node(0x1000, 0x8001), 15);
        assert_eq!(node(0x1000, 0x1000), 0);
        assert_eq!(node(0x4000, 0x1000), 0);
    }

    #[test]
    fn file_first_block_node() {
        let node = |file_size: u64| {
            let mut fs = FileStorage::new();
            fs.set_piece_length(0x10000);
            add_v2(&mut fs, "test/0", file_size, ROOT);
            fs.set_num_pieces(calc_num_pieces(&fs));
            fs.file_first_block_node(0)
        };
        assert_eq!(node(0x500000), 511);
        assert_eq!(node(0x1000), 0);
        assert_eq!(node(0x7fffff), 511);
        assert_eq!(node(0x800000), 511);
        assert_eq!(node(0x800001), 1023);
    }

    #[test]
    fn mismatching_v1_v2_files() {
        let mut st = FileStorage::new();
        st.set_piece_length(0x4000);
        add(&mut st, "test/a", 10000);
        assert!(st.add_file("test/B", 10000, NONE, 0, "", Some(Sha256Hash([0xab; 32]))).is_err());

        let mut st = FileStorage::new();
        st.set_piece_length(0x4000);
        add_v2(&mut st, "test/B", 10000, &[0xab; 32]);
        assert!(st.add_file("test/a", 10000, NONE, 0, "", None).is_err());
    }

    #[test]
    fn v2_detection() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(0x8000);
        fs.add_file("test/0", 0x5000, NONE, 0, "symlink-test-1", None).unwrap();
        fs.add_file("test/1", 0x5000, NONE, 0, "symlink-test-2", None).unwrap();
        add_v2(&mut fs, "test/2", 0x2000, ROOT);
        assert!(fs.add_file("test/3", 0x2000, NONE, 0, "", None).is_err());

        let mut fs = FileStorage::new();
        fs.set_piece_length(0x8000);
        fs.add_file("test/0", 0x5000, NONE, 0, "symlink-test-1", None).unwrap();
        fs.add_file("test/1", 0x5000, NONE, 0, "symlink-test-2", None).unwrap();
        add(&mut fs, "test/2", 0x2000);
        assert!(fs.add_file("test/3", 0x2000, NONE, 0, "", Some(Sha256Hash(*ROOT))).is_err());
    }

    #[test]
    fn blocks_in_piece2() {
        for (size, blocks) in [(0x3fff, 1), (0x4000, 1), (0x4001, 2)] {
            let mut fs = FileStorage::new();
            fs.set_piece_length(0x8000);
            add_v2(&mut fs, "test/0", size, ROOT);
            fs.set_num_pieces(calc_num_pieces(&fs));
            assert_eq!(fs.blocks_in_piece2(0), blocks);
        }
    }

    #[test]
    fn file_index_for_root() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(0x8000);
        for d in *b"1234" {
            add_v2(&mut fs, &format!("test/{}", d - b'1'), 0x8000, &[d; 32]);
        }
        for (i, d) in (*b"1234").into_iter().enumerate() {
            assert_eq!(fs.file_index_for_root(&Sha256Hash([d; 32])), Some(i));
        }
        assert_eq!(fs.file_index_for_root(&Sha256Hash([b'5'; 32])), None);
    }

    #[test]
    fn size_on_disk_explicit_pads() {
        let mut fs = FileStorage::new();
        fs.set_piece_length(0x8000);
        assert_eq!(size_on_disk(&fs), 0);
        add_v2(&mut fs, "test/0", 100, &[b'1'; 32]);
        assert_eq!(size_on_disk(&fs), 100);
        fs.add_file("test/pad/0", 80, FileFlags::PAD_FILE, 0, "", Some(Sha256Hash([b'2'; 32]))).unwrap();
        assert_eq!(size_on_disk(&fs), 100);
        add_v2(&mut fs, "test/2", 333, &[b'3'; 32]);
        assert_eq!(size_on_disk(&fs), 433);
        assert!(size_on_disk(&fs) < fs.total_size());
    }
}
