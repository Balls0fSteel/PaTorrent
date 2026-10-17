// Ported from libtorrent-rasterbar 2.0.11: src/part_file.cpp, include/libtorrent/part_file.hpp,
// src/posix_part_file.cpp, test/test_part_file.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the slot map is a separate, I/O-free type (fuzzed on its own); the header parser
// ignores a second piece claiming a slot that is already taken; free slots are reused lowest
// first and trailing free slots are cut off the file when the header is flushed; the open file
// handle is kept until `release`; live part files are shared between storage instances of the
// same torrent through `PartFiles`.

//! The part file: blocks of pieces that overlap files with priority 0 ("skipped" files), kept
//! in one `.<info-hash>.parts` file in the save path so the skipped files are never created.
//!
//! On-disk format (libtorrent's): a header of `round_up_1024((2 + num_pieces) * 4)` bytes holding
//! the big-endian `u32` piece count, the `u32` piece size, then one big-endian `i32` slot per
//! piece (-1: not in the file), zero-padded. Slot `s` holds its piece at
//! `header_size + s * piece_size`, at the piece-relative offset of each block.

use std::collections::{BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use super::file_pool::{read_at, write_at};

/// libtorrent `round_up`: header sizes are multiples of 1 KiB.
fn round_up(n: u64) -> u64 {
    (n + 1023) & !0x3ff
}

/// Which slot of the part file holds which piece (libtorrent `part_file::m_piece_map`,
/// `m_free_slots`, `m_num_allocated`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotMap {
    max_pieces: u32,
    piece_size: u32,
    piece_map: HashMap<u32, u32>,
    free_slots: BTreeSet<u32>,
    num_allocated: u32,
}

impl SlotMap {
    pub fn new(max_pieces: u32, piece_size: u32) -> SlotMap {
        SlotMap {
            max_pieces,
            piece_size,
            piece_map: HashMap::new(),
            free_slots: BTreeSet::new(),
            num_allocated: 0,
        }
    }

    pub fn header_size(max_pieces: u32) -> u64 {
        round_up((2 + max_pieces as u64) * 4)
    }

    /// Reads a header (libtorrent's constructor). A header that is short or written for another
    /// piece count or piece size yields an empty map, so the file's contents are overwritten.
    pub fn parse(header: &[u8], max_pieces: u32, piece_size: u32) -> SlotMap {
        let mut map = SlotMap::new(max_pieces, piece_size);
        let size = SlotMap::header_size(max_pieces);
        if (header.len() as u64) < size {
            return map;
        }
        let word = |i: usize| u32::from_be_bytes(header[i * 4..i * 4 + 4].try_into().expect("4 bytes"));
        if word(0) != max_pieces || word(1) != piece_size {
            return map;
        }
        let mut used = vec![false; max_pieces as usize];
        for piece in 0..max_pieces {
            let slot = word(2 + piece as usize) as i32;
            // Negative: not in the file. Out of range: an invalid part file (libtorrent skips it).
            // Taken: two pieces in one slot would hand one piece's data to the other.
            if slot < 0 || slot as u32 >= max_pieces || used[slot as usize] {
                continue;
            }
            let slot = slot as u32;
            used[slot as usize] = true;
            map.num_allocated = map.num_allocated.max(slot + 1);
            map.piece_map.insert(piece, slot);
        }
        // The holes below the highest slot are free.
        map.free_slots = (0..map.num_allocated).filter(|&s| !used[s as usize]).collect();
        map
    }

    /// The header for the current map (libtorrent `flush_metadata_impl`).
    pub fn serialize(&self) -> Vec<u8> {
        let mut header = Vec::with_capacity(SlotMap::header_size(self.max_pieces) as usize);
        header.extend_from_slice(&self.max_pieces.to_be_bytes());
        header.extend_from_slice(&self.piece_size.to_be_bytes());
        for piece in 0..self.max_pieces {
            let slot = self.piece_map.get(&piece).map_or(-1, |&s| s as i32);
            header.extend_from_slice(&slot.to_be_bytes());
        }
        header.resize(SlotMap::header_size(self.max_pieces) as usize, 0);
        header
    }

    pub fn slot(&self, piece: u32) -> Option<u32> {
        self.piece_map.get(&piece).copied()
    }

    /// Gives `piece` a slot: the lowest free one, else a new one at the end (libtorrent
    /// `allocate_slot`). Returns the piece's slot if it already has one.
    pub fn allocate(&mut self, piece: u32) -> u32 {
        if let Some(slot) = self.slot(piece) {
            return slot;
        }
        let slot = match self.free_slots.pop_first() {
            Some(s) => s,
            None => {
                self.num_allocated += 1;
                self.num_allocated - 1
            }
        };
        self.piece_map.insert(piece, slot);
        slot
    }

    /// Frees `piece`'s slot (libtorrent `free_piece`). Returns false if it had none.
    pub fn free(&mut self, piece: u32) -> bool {
        match self.piece_map.remove(&piece) {
            Some(slot) => {
                self.free_slots.insert(slot);
                true
            }
            None => false,
        }
    }

    /// Drops free slots at the end of the file. Returns the slot count left.
    pub fn trim(&mut self) -> u32 {
        while self.num_allocated > 0 && self.free_slots.remove(&(self.num_allocated - 1)) {
            self.num_allocated -= 1;
        }
        self.num_allocated
    }

    pub fn is_empty(&self) -> bool {
        self.piece_map.is_empty()
    }

    #[cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]
    pub fn num_allocated(&self) -> u32 {
        self.num_allocated
    }

    /// Pieces with a slot, in no particular order.
    pub fn pieces(&self) -> impl Iterator<Item = u32> + '_ {
        self.piece_map.keys().copied()
    }

    /// Checks the invariants: every slot below `num_allocated` is either free or held by exactly
    /// one piece, and nothing is at or above it.
    #[cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]
    pub fn is_consistent(&self) -> bool {
        let mut seen = vec![false; self.num_allocated as usize];
        for &slot in self.piece_map.values().chain(self.free_slots.iter()) {
            if slot >= self.num_allocated || std::mem::replace(&mut seen[slot as usize], true) {
                return false;
            }
        }
        seen.iter().all(|&s| s) && self.piece_map.keys().all(|&p| p < self.max_pieces)
    }
}

struct Inner {
    /// The directory holding the file (changed by `move_partfile`).
    dir: PathBuf,
    map: SlotMap,
    dirty: bool,
    /// The open file, and whether it was opened for writing.
    file: Option<(Arc<File>, bool)>,
}

/// libtorrent `part_file` / `posix_part_file`. Thread safe: the slot map is locked, but not
/// during the file I/O.
pub(crate) struct PartFile {
    name: String,
    piece_size: u32,
    header_size: u64,
    inner: Mutex<Inner>,
}

impl PartFile {
    /// Loads the header of `dir/name` if the file exists. Nothing is created until a write.
    pub fn new(dir: &Path, name: &str, num_pieces: u32, piece_size: u32) -> PartFile {
        debug_assert!(num_pieces > 0 && piece_size > 0);
        let header_size = SlotMap::header_size(num_pieces);
        let path = dir.join(name);
        let map = match File::open(&path) {
            Ok(f) => {
                let mut header = vec![0u8; header_size as usize];
                match read_at(&f, &mut header, 0) {
                    Ok(n) => SlotMap::parse(&header[..n], num_pieces, piece_size),
                    Err(_) => SlotMap::new(num_pieces, piece_size),
                }
            }
            Err(_) => SlotMap::new(num_pieces, piece_size),
        };
        PartFile {
            name: name.to_owned(),
            piece_size,
            header_size,
            inner: Mutex::new(Inner { dir: dir.to_path_buf(), map, dirty: false, file: None }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn slot_offset(&self, slot: u32) -> u64 {
        self.header_size + slot as u64 * self.piece_size as u64
    }

    /// Writes `buf` at `offset` in `piece`, giving the piece a slot first if needed.
    pub fn write(&self, buf: &[u8], piece: u32, offset: u32) -> io::Result<()> {
        debug_assert!(offset as u64 + buf.len() as u64 <= self.piece_size as u64);
        let mut inner = self.lock();
        let file = inner.open(self, true)?;
        let slot = match inner.map.slot(piece) {
            Some(s) => s,
            None => {
                inner.dirty = true;
                inner.map.allocate(piece)
            }
        };
        drop(inner);
        write_at(&file, buf, self.slot_offset(slot) + offset as u64)
    }

    /// Fills `buf` from `offset` in `piece`. A piece without a slot is `NotFound`.
    pub fn read(&self, buf: &mut [u8], piece: u32, offset: u32) -> io::Result<()> {
        debug_assert!(offset as u64 + buf.len() as u64 <= self.piece_size as u64);
        let mut inner = self.lock();
        let Some(slot) = inner.map.slot(piece) else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "piece not in the part file"));
        };
        let file = inner.open(self, false)?;
        drop(inner);
        if read_at(&file, buf, self.slot_offset(slot) + offset as u64)? != buf.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "part file too short"));
        }
        Ok(())
    }

    pub fn has_piece(&self, piece: u32) -> bool {
        self.lock().map.slot(piece).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().map.is_empty()
    }

    /// Frees `piece`'s slot; the data stays until the slot is reused (libtorrent `free_piece`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn free_piece(&self, piece: u32) {
        let mut inner = self.lock();
        if inner.map.free(piece) {
            inner.dirty = true;
        }
    }

    /// Frees every piece for which `keep` is false.
    pub fn retain(&self, mut keep: impl FnMut(u32) -> bool) {
        let mut inner = self.lock();
        let drop: Vec<u32> = inner.map.pieces().filter(|&p| !keep(p)).collect();
        for p in drop {
            inner.map.free(p);
            inner.dirty = true;
        }
    }

    /// Hands the stored bytes of the torrent range `offset..offset + size` (one file) to `f` as
    /// (offset in the range, data), piece by piece, then frees the pieces the range covers whole
    /// (libtorrent `export_file`). Pieces not in the part file are skipped.
    pub fn export_file(
        &self,
        offset: u64,
        size: u64,
        mut f: impl FnMut(u64, &[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut inner = self.lock();
        if inner.map.is_empty() || size == 0 {
            return Ok(());
        }
        let ps = self.piece_size as u64;
        let end = (offset + size).div_ceil(ps);
        let mut piece = offset / ps;
        let mut piece_offset = offset - piece * ps;
        let mut file_offset = 0u64;
        let mut left = size;
        let file = inner.open(self, false)?;
        let mut buf = Vec::new();
        while piece < end {
            let copy = (ps - piece_offset).min(left) as usize;
            if let Some(slot) = inner.map.slot(piece as u32) {
                drop(inner);
                buf.clear();
                buf.resize(copy, 0);
                // Never-written bytes past the end of the file read as zeros.
                read_at(&file, &mut buf, self.slot_offset(slot) + piece_offset)?;
                f(file_offset, &buf)?;
                inner = self.lock();
                if copy as u64 == ps && inner.map.slot(piece as u32) == Some(slot) {
                    inner.map.free(piece as u32);
                    inner.dirty = true;
                }
            }
            file_offset += copy as u64;
            left -= copy as u64;
            piece_offset = 0;
            piece += 1;
        }
        Ok(())
    }

    /// Writes the header if it changed; deletes the file once it holds no piece (libtorrent
    /// `flush_metadata`). Trailing free slots are cut off.
    pub fn flush_metadata(&self) -> io::Result<()> {
        let mut inner = self.lock();
        if !inner.dirty {
            return Ok(());
        }
        let path = inner.path(self);
        if inner.map.is_empty() {
            inner.file = None;
            inner.map.trim();
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            inner.dirty = false;
            return Ok(());
        }
        let file = inner.open(self, true)?;
        let slots = inner.map.trim();
        let len = self.slot_offset(slots);
        if file.metadata()?.len() > len {
            file.set_len(len)?;
        }
        write_at(&file, &inner.map.serialize(), 0)?;
        inner.dirty = false;
        Ok(())
    }

    /// Flushes the header and closes the file (libtorrent `release_files`).
    pub fn release(&self) -> io::Result<()> {
        let r = self.flush_metadata();
        self.lock().file = None;
        r
    }

    /// Forgets every piece and deletes the file (removing a torrent with its data).
    pub fn delete(&self) -> io::Result<()> {
        let mut inner = self.lock();
        inner.file = None;
        inner.map = SlotMap::new(inner.map.max_pieces, self.piece_size);
        inner.dirty = false;
        match std::fs::remove_file(inner.path(self)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Moves the file to `dir` (libtorrent `move_partfile`): a rename, or a copy and delete
    /// across file systems.
    pub fn move_partfile(&self, dir: &Path) -> io::Result<()> {
        self.flush_metadata()?;
        let mut inner = self.lock();
        inner.file = None;
        if !inner.map.is_empty() {
            let old = inner.path(self);
            let new = dir.join(&self.name);
            match std::fs::rename(&old, &new) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(_) => {
                    std::fs::copy(&old, &new)?;
                    std::fs::remove_file(&old)?;
                }
            }
        }
        inner.dir = dir.to_path_buf();
        Ok(())
    }
}

impl Inner {
    fn path(&self, pf: &PartFile) -> PathBuf {
        self.dir.join(&pf.name)
    }

    /// libtorrent `part_file::open_file`: opening for writing creates the file (hidden on
    /// Windows) and its directory.
    fn open(&mut self, pf: &PartFile, write: bool) -> io::Result<Arc<File>> {
        if let Some((f, writable)) = &self.file {
            if *writable || !write {
                return Ok(f.clone());
            }
        }
        let path = self.path(pf);
        let file = if write {
            let mut opts = OpenOptions::new();
            opts.read(true).write(true).create(true).truncate(false);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                // FILE_ATTRIBUTE_HIDDEN, as libtorrent's `open_mode::hidden`.
                opts.attributes(0x2);
            }
            match opts.open(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    opts.open(&path)?
                }
                r => r?,
            }
        } else {
            File::open(&path)?
        };
        let file = Arc::new(file);
        self.file = Some((file.clone(), write));
        Ok(file)
    }
}

impl Drop for PartFile {
    fn drop(&mut self) {
        let _ = self.flush_metadata();
    }
}

/// The live part files, by path, so a restarted torrent whose old storage still has jobs in
/// flight shares one slot map instead of two diverging ones.
#[derive(Default)]
pub(crate) struct PartFiles {
    open: Mutex<HashMap<PathBuf, Weak<PartFile>>>,
}

impl PartFiles {
    pub fn get_or_open(&self, dir: &Path, name: &str, num_pieces: u32, piece_size: u32) -> Arc<PartFile> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let path = dir.join(name);
        if let Some(pf) = open.get(&path).and_then(Weak::upgrade) {
            return pf;
        }
        open.retain(|_, w| w.strong_count() > 0);
        let pf = Arc::new(PartFile::new(dir, name, num_pieces, piece_size));
        open.insert(path, Arc::downgrade(&pf));
        pf
    }

    /// The live part file at `path`, if any.
    pub fn get(&self, path: &Path) -> Option<Arc<PartFile>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).get(path).and_then(Weak::upgrade)
    }

    /// The part file at `old` was moved to `new`.
    pub fn rekey(&self, old: &Path, new: &Path) {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pf) = open.remove(old) {
            open.insert(new.to_path_buf(), pf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Sha1;

    const PIECE: u32 = 16 * 0x4000;

    fn pattern() -> Vec<u8> {
        (0..1024).map(|i| i as u8).collect()
    }

    /// test_part_file.cpp `part_file` (and `posix_part_file`, the same code here).
    #[test]
    fn part_file() {
        let root = crate::disk::tests::temp_dir("part_file");
        let (dir, dir2) = (root.join("partfile_test_dir"), root.join("partfile_test_dir2"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&dir2).unwrap();
        let buf = pattern();
        {
            let pf = PartFile::new(&dir, "partfile.parts", 100, PIECE);
            pf.flush_metadata().unwrap();
            // Nothing is in the part file, so it has not been created yet.
            assert!(!dir.join("partfile.parts").exists());

            pf.write(&buf, 10, 0).unwrap();
            pf.flush_metadata().unwrap();
            assert!(dir.join("partfile.parts").exists());

            pf.move_partfile(&dir2).unwrap();
            assert!(!dir.join("partfile.parts").exists());
            assert!(dir2.join("partfile.parts").exists());

            let mut back = vec![0u8; 1024];
            pf.read(&mut back, 10, 0).unwrap();
            assert_eq!(back, buf);
            assert_eq!(Sha1::digest(&back), Sha1::digest(&buf));
        }
        {
            // Load the part file back in.
            let pf = PartFile::new(&dir2, "partfile.parts", 100, PIECE);
            let mut back = vec![0u8; 1024];
            pf.read(&mut back, 10, 0).unwrap();
            assert_eq!(back, buf);

            let mut seen = 0u64;
            pf.export_file(10 * PIECE as u64, 1024, |file_offset, data| {
                for (i, &b) in data.iter().enumerate() {
                    assert_eq!(b, (file_offset + i as u64) as u8);
                }
                seen += data.len() as u64;
                Ok(())
            })
            .unwrap();
            assert_eq!(seen, 1024);

            pf.free_piece(10);
            pf.flush_metadata().unwrap();
            // That was the last piece: the part file deletes itself.
            assert!(!dir2.join("partfile.parts").exists());
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn header_format_matches_libtorrent() {
        let mut m = SlotMap::new(3, 0x4000);
        assert_eq!(SlotMap::header_size(3), 1024);
        assert_eq!(SlotMap::header_size(254), 1024);
        assert_eq!(SlotMap::header_size(255), 2048);
        assert_eq!(m.allocate(2), 0);
        assert_eq!(m.allocate(0), 1);
        let h = m.serialize();
        assert_eq!(h.len(), 1024);
        assert_eq!(&h[..20], &[0, 0, 0, 3, 0, 0, 0x40, 0, 0, 0, 0, 1, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0]);
        assert!(h[20..].iter().all(|&b| b == 0));
        assert_eq!(SlotMap::parse(&h, 3, 0x4000), m);
        // Another piece count or piece size, or a short header: start empty.
        assert!(SlotMap::parse(&h, 4, 0x4000).is_empty());
        assert!(SlotMap::parse(&h, 3, 0x8000).is_empty());
        assert!(SlotMap::parse(&h[..1000], 3, 0x4000).is_empty());
    }

    #[test]
    fn parse_skips_invalid_and_duplicate_slots_and_finds_holes() {
        let mut h = vec![0u8; 1024];
        h[..8].copy_from_slice(&[0, 0, 0, 4, 0, 0, 0x40, 0]);
        for (piece, slot) in [(0usize, 3i32), (1, 3), (2, 9), (3, -5)] {
            h[8 + piece * 4..12 + piece * 4].copy_from_slice(&slot.to_be_bytes());
        }
        let mut m = SlotMap::parse(&h, 4, 0x4000);
        assert!(m.is_consistent());
        assert_eq!((m.slot(0), m.slot(1), m.slot(2), m.slot(3)), (Some(3), None, None, None));
        assert_eq!(m.num_allocated(), 4);
        // Holes are reused lowest first.
        assert_eq!(m.allocate(1), 0);
        assert_eq!(m.allocate(2), 1);
        assert!(m.is_consistent());
    }

    #[test]
    fn freed_slots_are_reused_and_trailing_ones_cut_off() {
        let root = crate::disk::tests::temp_dir("part_file_trim");
        let pf = PartFile::new(&root, ".x.parts", 8, 0x4000);
        let block = vec![7u8; 0x4000];
        for piece in [5, 1, 6] {
            pf.write(&block, piece, 0).unwrap();
        }
        pf.flush_metadata().unwrap();
        let path = root.join(".x.parts");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 1024 + 3 * 0x4000);
        pf.free_piece(6);
        pf.flush_metadata().unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 1024 + 2 * 0x4000);
        pf.free_piece(5);
        pf.write(&block[..10], 3, 100).unwrap();
        pf.flush_metadata().unwrap();
        // Piece 3 took slot 0, freed by piece 5.
        let pf2 = PartFile::new(&root, ".x.parts", 8, 0x4000);
        assert!(pf2.has_piece(3) && pf2.has_piece(1) && !pf2.has_piece(5));
        let mut b = vec![0u8; 10];
        pf2.read(&mut b, 3, 100).unwrap();
        assert_eq!(b, [7; 10]);
        assert_eq!(pf2.read(&mut b, 5, 0).unwrap_err().kind(), io::ErrorKind::NotFound);
        pf2.retain(|_| false);
        pf2.flush_metadata().unwrap();
        assert!(!path.exists());
        drop((pf, pf2));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn export_frees_only_whole_pieces() {
        let root = crate::disk::tests::temp_dir("part_file_export");
        let ps = 0x4000u32;
        let pf = PartFile::new(&root, ".e.parts", 4, ps);
        let data: Vec<u8> = (0..4 * ps).map(|i| (i % 249) as u8).collect();
        // A file covering the end of piece 0, all of piece 1 and the start of piece 2.
        let (start, len) = (ps as u64 - 100, ps as u64 + 300);
        pf.write(&data[start as usize..ps as usize], 0, ps - 100).unwrap();
        pf.write(&data[ps as usize..2 * ps as usize], 1, 0).unwrap();
        pf.write(&data[2 * ps as usize..2 * ps as usize + 200], 2, 0).unwrap();
        let mut out = vec![0u8; len as usize];
        pf.export_file(start, len, |off, buf| {
            out[off as usize..off as usize + buf.len()].copy_from_slice(buf);
            Ok(())
        })
        .unwrap();
        assert_eq!(out, data[start as usize..(start + len) as usize]);
        assert!(pf.has_piece(0) && !pf.has_piece(1) && pf.has_piece(2));
        drop(pf);
        // Dropping flushes the header.
        let pf = PartFile::new(&root, ".e.parts", 4, ps);
        assert!(pf.has_piece(0) && !pf.has_piece(1) && pf.has_piece(2));
        pf.delete().unwrap();
        assert!(!root.join(".e.parts").exists());
        drop(pf);
        assert!(!root.join(".e.parts").exists(), "a deleted part file is not written back");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn registry_shares_live_part_files() {
        let root = crate::disk::tests::temp_dir("part_file_registry");
        let reg = PartFiles::default();
        let a = reg.get_or_open(&root, ".r.parts", 2, 0x4000);
        let b = reg.get_or_open(&root, ".r.parts", 2, 0x4000);
        assert!(Arc::ptr_eq(&a, &b));
        assert!(reg.get(&root.join(".r.parts")).is_some());
        drop((a, b));
        assert!(reg.get(&root.join(".r.parts")).is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
