// Ported from libtorrent-rasterbar 2.0.11: src/mmap_disk_io.cpp (job, fence and thread-pool
// structure), src/posix_storage.cpp, src/storage_utils.cpp, src/disk_job_fence.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: positional read/write instead of mmap (PLAN.md D3). Jobs for a piece always run on
// the same thread (piece % threads), so per-piece hash state needs no locks; a fence is a job
// broadcast to every thread whose last finisher runs the fenced action. Blocks are hashed as
// they are written, in order, so a completed piece is never read back unless its blocks arrived
// out of order and did not fit in the small reorder cache. Blocks restored from resume data
// (`unfinished`) are on disk but not in any hash state, so they are read back as the cursor
// reaches them.

//! Disk I/O: a thread pool doing file reads, writes, piece hashing and file checking for the
//! network thread.

pub(crate) mod file_pool;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use crate::crypto::Sha1;
use crate::error::Error;
use crate::metainfo::TorrentInfo;
use crate::metainfo::file_storage::FileFlags;
use crate::util::bitfield::Bitfield;
use file_pool::{FilePool, read_at, write_at};

/// The BitTorrent block size (libtorrent `default_block_size`).
pub(crate) const BLOCK_SIZE: u32 = 16 * 1024;

/// Out-of-order blocks each thread may keep in memory while waiting to hash them in order.
const REORDER_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// One torrent's files on disk (libtorrent `storage_interface` / `posix_storage`).
pub(crate) struct Storage {
    /// Unique for the life of the process; a restarted torrent gets a new one, so completions
    /// for an older instance can be told apart.
    pub id: u64,
    pub save_path: String,
    pub info: Arc<TorrentInfo>,
    /// Set by the network thread to abandon queued and running work (checks, hashing).
    pub abort: AtomicBool,
}

impl Storage {
    pub fn new(id: u64, save_path: String, info: Arc<TorrentInfo>) -> Storage {
        Storage { id, save_path, info, abort: AtomicBool::new(false) }
    }

    fn file_path(&self, file: usize) -> PathBuf {
        PathBuf::from(self.info.files().file_path(file, &self.save_path))
    }

    fn skip_file(&self, file: usize) -> bool {
        let fs = self.info.files();
        fs.file_flags(file).intersects(FileFlags::PAD_FILE | FileFlags::SYMLINK)
    }
}

#[derive(Debug)]
pub(crate) enum CheckMode {
    /// Hash every piece (libtorrent `check_files` without resume data, or `force_recheck`).
    Full,
    /// Trust these pieces if every file holding them exists and is long enough; otherwise fall
    /// back to a full check (libtorrent `verify_resume_data`).
    Resume(Bitfield),
}

enum Job {
    Write { storage: Arc<Storage>, piece: u32, offset: u32, data: Vec<u8> },
    Read { storage: Arc<Storage>, piece: u32, offset: u32, length: u32, cookie: u64 },
    Restore { storage: Arc<Storage>, piece: u32, blocks: Vec<bool> },
    Check { storage: Arc<Storage>, mode: CheckMode },
    Fence { storage: Arc<Storage>, fence: Arc<Fence> },
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FenceAction {
    /// Close the files and drop all cached state (libtorrent `release_files`, `stop_torrent`).
    Release,
    /// Release, then delete the files and any directories left empty (libtorrent
    /// `delete_files`).
    Delete,
}

struct Fence {
    remaining: AtomicUsize,
    action: FenceAction,
}

#[derive(Debug)]
pub(crate) struct CheckResult {
    pub have: Bitfield,
    /// True when resume data did not match the files and a full check was done instead.
    pub resume_failed: bool,
}

/// Results sent back to the network thread.
#[derive(Debug)]
pub(crate) enum Completion {
    Write {
        storage: u64,
        piece: u32,
        offset: u32,
        error: Option<(String, Error)>,
    },
    /// Every block of the piece was written; `passed` tells whether its hash matched.
    Hashed {
        storage: u64,
        piece: u32,
        passed: bool,
    },
    Read {
        storage: u64,
        cookie: u64,
        piece: u32,
        offset: u32,
        result: Result<Vec<u8>, Error>,
    },
    CheckProgress {
        storage: u64,
        checked: u32,
    },
    Checked {
        storage: u64,
        result: Result<CheckResult, (String, Error)>,
    },
    Fenced {
        storage: u64,
        action: FenceAction,
        error: Option<Error>,
    },
}

pub(crate) struct DiskIo {
    threads: Vec<mpsc::Sender<Job>>,
    hasher: mpsc::Sender<Job>,
    handles: Vec<JoinHandle<()>>,
    pool: Arc<FilePool>,
    queued_write_bytes: Arc<AtomicU64>,
    next_storage_id: u64,
}

struct Ctx {
    pool: Arc<FilePool>,
    done: mpsc::Sender<Completion>,
    wake: Arc<dyn Fn() + Send + Sync>,
    queued_write_bytes: Arc<AtomicU64>,
}

impl Ctx {
    fn send(&self, c: Completion) {
        if self.done.send(c).is_ok() {
            (self.wake)();
        }
    }
}

impl DiskIo {
    /// Starts `threads` I/O threads plus one checking thread. Completions go to `done`, followed
    /// by a call to `wake`.
    pub fn new(
        threads: usize,
        file_pool_size: usize,
        done: mpsc::Sender<Completion>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> DiskIo {
        let pool = Arc::new(FilePool::new(file_pool_size));
        let queued_write_bytes = Arc::new(AtomicU64::new(0));
        let ctx = || Ctx {
            pool: pool.clone(),
            done: done.clone(),
            wake: wake.clone(),
            queued_write_bytes: queued_write_bytes.clone(),
        };
        let mut senders = Vec::new();
        let mut handles = Vec::new();
        let spawn = |name: String, rx: mpsc::Receiver<Job>, ctx: Ctx| {
            std::thread::Builder::new()
                .name(name)
                .spawn(move || disk_thread(rx, ctx))
                .expect("failed to spawn a disk thread")
        };
        for i in 0..threads.clamp(1, 64) {
            let (tx, rx) = mpsc::channel();
            handles.push(spawn(format!("lt-disk-{i}"), rx, ctx()));
            senders.push(tx);
        }
        let (hasher, rx) = mpsc::channel();
        handles.push(spawn("lt-hash".into(), rx, ctx()));
        DiskIo { threads: senders, hasher, handles, pool, queued_write_bytes, next_storage_id: 1 }
    }

    pub fn new_storage(&mut self, save_path: String, info: Arc<TorrentInfo>) -> Arc<Storage> {
        let id = self.next_storage_id;
        self.next_storage_id += 1;
        Arc::new(Storage::new(id, save_path, info))
    }

    pub fn set_file_pool_size(&self, n: usize) {
        self.pool.set_limit(n);
    }

    fn thread_for(&self, piece: u32) -> &mpsc::Sender<Job> {
        &self.threads[piece as usize % self.threads.len()]
    }

    pub fn async_write(&self, storage: &Arc<Storage>, piece: u32, offset: u32, data: Vec<u8>) {
        self.queued_write_bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        let _ = self.thread_for(piece).send(Job::Write { storage: storage.clone(), piece, offset, data });
    }

    pub fn async_read(&self, storage: &Arc<Storage>, piece: u32, offset: u32, length: u32, cookie: u64) {
        let _ = self.thread_for(piece).send(Job::Read {
            storage: storage.clone(),
            piece,
            offset,
            length,
            cookie,
        });
    }

    /// Tells the piece's hash state that `blocks` are already on disk (restored from resume
    /// data). Must be queued before any write to the piece. A piece that is complete this way is
    /// hashed at once and answered by [`Completion::Hashed`].
    pub fn async_restore_partial(&self, storage: &Arc<Storage>, piece: u32, blocks: Vec<bool>) {
        let _ = self.thread_for(piece).send(Job::Restore { storage: storage.clone(), piece, blocks });
    }

    pub fn async_check(&self, storage: &Arc<Storage>, mode: CheckMode) {
        let _ = self.hasher.send(Job::Check { storage: storage.clone(), mode });
    }

    /// Runs `action` after every job queued so far for `storage` has finished (libtorrent
    /// `disk_job_fence`). Answered by [`Completion::Fenced`].
    pub fn async_fence(&self, storage: &Arc<Storage>, action: FenceAction) {
        storage.abort.store(true, Ordering::Relaxed);
        let fence = Arc::new(Fence { remaining: AtomicUsize::new(self.threads.len() + 1), action });
        for tx in self.threads.iter().chain([&self.hasher]) {
            let _ = tx.send(Job::Fence { storage: storage.clone(), fence: fence.clone() });
        }
    }

    /// Bytes handed to `async_write` and not written yet. Peers stop reading from their sockets
    /// while this exceeds `max_queued_disk_bytes` (libtorrent's disk-queue back-pressure).
    pub fn queued_write_bytes(&self) -> u64 {
        self.queued_write_bytes.load(Ordering::Relaxed)
    }
}

impl Drop for DiskIo {
    fn drop(&mut self) {
        for tx in self.threads.iter().chain([&self.hasher]) {
            let _ = tx.send(Job::Shutdown);
        }
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

/// Incremental hash of one piece being downloaded.
struct PartialHash {
    hasher: Sha1,
    /// Next block to feed to the hasher.
    cursor: u32,
    received: Vec<bool>,
    /// Blocks written ahead of the cursor, kept to avoid reading them back.
    cached: HashMap<u32, Vec<u8>>,
    /// Blocks restored from resume data. A read error on one of them fails the piece (the data
    /// is downloaded again) instead of failing the torrent.
    restored: Vec<bool>,
}

impl PartialHash {
    fn new(num_blocks: u32) -> PartialHash {
        PartialHash {
            hasher: Sha1::new(),
            cursor: 0,
            received: vec![false; num_blocks as usize],
            cached: HashMap::new(),
            restored: vec![false; num_blocks as usize],
        }
    }
}

fn disk_thread(rx: mpsc::Receiver<Job>, ctx: Ctx) {
    let mut partial: HashMap<(u64, u32), PartialHash> = HashMap::new();
    let mut cached_bytes = 0usize;
    while let Ok(job) = rx.recv() {
        match job {
            Job::Write { storage, piece, offset, data } => {
                let length = data.len() as u32;
                let result =
                    write_block(&storage, &ctx, &mut partial, &mut cached_bytes, piece, offset, data);
                ctx.queued_write_bytes.fetch_sub(length as u64, Ordering::Relaxed);
                match result {
                    Ok(hashed) => {
                        ctx.send(Completion::Write { storage: storage.id, piece, offset, error: None });
                        if let Some(passed) = hashed {
                            ctx.send(Completion::Hashed { storage: storage.id, piece, passed });
                        }
                    }
                    Err(e) => {
                        drop_partial(&mut partial, &mut cached_bytes, storage.id, piece);
                        ctx.send(Completion::Write { storage: storage.id, piece, offset, error: Some(e) });
                    }
                }
            }
            Job::Restore { storage, piece, blocks } => {
                let n = storage.info.piece_size(piece).div_ceil(BLOCK_SIZE);
                let state = partial.entry((storage.id, piece)).or_insert_with(|| PartialHash::new(n));
                for (b, &on_disk) in blocks.iter().enumerate().take(n as usize) {
                    if on_disk && !state.received[b] {
                        state.received[b] = true;
                        state.restored[b] = true;
                    }
                }
                match advance_hash(&storage, &ctx, &mut partial, &mut cached_bytes, piece) {
                    Ok(Some(passed)) => ctx.send(Completion::Hashed { storage: storage.id, piece, passed }),
                    Ok(None) => {}
                    Err(_) => {
                        // Only possible if writes were queued before the restore: fail the piece.
                        drop_partial(&mut partial, &mut cached_bytes, storage.id, piece);
                        ctx.send(Completion::Hashed { storage: storage.id, piece, passed: false });
                    }
                }
            }
            Job::Read { storage, piece, offset, length, cookie } => {
                let result = read_block(&storage, &ctx.pool, piece, offset, length);
                ctx.send(Completion::Read { storage: storage.id, cookie, piece, offset, result });
            }
            Job::Check { storage, mode } => {
                if !storage.abort.load(Ordering::Relaxed) {
                    let result = check_files(&storage, &ctx, mode);
                    ctx.send(Completion::Checked { storage: storage.id, result });
                }
            }
            Job::Fence { storage, fence } => {
                let id = storage.id;
                let keys: Vec<_> = partial.keys().filter(|k| k.0 == id).copied().collect();
                for (_, piece) in keys {
                    drop_partial(&mut partial, &mut cached_bytes, id, piece);
                }
                if fence.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                    ctx.pool.release(id);
                    let error = match fence.action {
                        FenceAction::Release => None,
                        FenceAction::Delete => delete_files(&storage).err(),
                    };
                    ctx.send(Completion::Fenced { storage: id, action: fence.action, error });
                }
            }
            Job::Shutdown => break,
        }
    }
}

fn drop_partial(
    partial: &mut HashMap<(u64, u32), PartialHash>,
    cached_bytes: &mut usize,
    storage: u64,
    piece: u32,
) {
    if let Some(p) = partial.remove(&(storage, piece)) {
        *cached_bytes -= p.cached.values().map(Vec::len).sum::<usize>();
    }
}

fn io_error(storage: &Storage, file: usize, e: io::Error) -> (String, Error) {
    (storage.info.files().file_path(file, &storage.save_path), Error::from(e))
}

/// Writes one block and advances the piece's hash. Returns `Some(passed)` once the whole piece
/// has been hashed.
fn write_block(
    storage: &Storage,
    ctx: &Ctx,
    partial: &mut HashMap<(u64, u32), PartialHash>,
    cached_bytes: &mut usize,
    piece: u32,
    offset: u32,
    data: Vec<u8>,
) -> Result<Option<bool>, (String, Error)> {
    let fs = storage.info.files();
    let piece_size = storage.info.piece_size(piece);
    let num_blocks = piece_size.div_ceil(BLOCK_SIZE);
    let block = offset / BLOCK_SIZE;
    let state = partial.entry((storage.id, piece)).or_insert_with(|| PartialHash::new(num_blocks));
    if block >= num_blocks || state.received[block as usize] {
        // Out of range or a duplicate (end-game): nothing new to write.
        return Ok(None);
    }

    let mut pos = 0usize;
    for slice in fs.map_block(piece, offset as u64, data.len() as u64) {
        let n = slice.size as usize;
        if !storage.skip_file(slice.file_index) {
            let file = ctx
                .pool
                .open(storage.id, slice.file_index as u32, &storage.file_path(slice.file_index), true)
                .map_err(|e| io_error(storage, slice.file_index, e))?;
            write_at(&file, &data[pos..pos + n], slice.offset)
                .map_err(|e| io_error(storage, slice.file_index, e))?;
        }
        pos += n;
    }
    state.received[block as usize] = true;

    if block == state.cursor {
        state.hasher.update(&data);
        state.cursor += 1;
    } else if *cached_bytes + data.len() <= REORDER_CACHE_BYTES {
        *cached_bytes += data.len();
        state.cached.insert(block, data);
    }
    advance_hash(storage, ctx, partial, cached_bytes, piece)
}

/// Feeds the hasher every received block from the cursor on, reading back blocks that are not
/// cached. Returns `Some(passed)` once the whole piece has been hashed; the state is dropped then.
fn advance_hash(
    storage: &Storage,
    ctx: &Ctx,
    partial: &mut HashMap<(u64, u32), PartialHash>,
    cached_bytes: &mut usize,
    piece: u32,
) -> Result<Option<bool>, (String, Error)> {
    let fs = storage.info.files();
    let piece_size = storage.info.piece_size(piece);
    let num_blocks = piece_size.div_ceil(BLOCK_SIZE);
    let Some(state) = partial.get_mut(&(storage.id, piece)) else { return Ok(None) };
    while state.cursor < num_blocks && state.received[state.cursor as usize] {
        let b = state.cursor;
        match state.cached.remove(&b) {
            Some(buf) => {
                *cached_bytes -= buf.len();
                state.hasher.update(&buf);
            }
            None => {
                let len = (piece_size - b * BLOCK_SIZE).min(BLOCK_SIZE);
                match read_block(storage, &ctx.pool, piece, b * BLOCK_SIZE, len) {
                    Ok(buf) => {
                        state.hasher.update(&buf);
                    }
                    Err(_) if state.restored[b as usize] => {
                        // The resume data promised a block the files do not hold.
                        drop_partial(partial, cached_bytes, storage.id, piece);
                        return Ok(Some(false));
                    }
                    Err(e) => {
                        return Err((fs.file_path(fs.file_index_at_piece(piece), &storage.save_path), e));
                    }
                }
            }
        }
        state.cursor += 1;
    }
    if state.cursor < num_blocks {
        return Ok(None);
    }
    let state = partial.remove(&(storage.id, piece)).expect("present");
    let passed = Some(state.hasher.finalize()) == storage.info.hash_for_piece(piece);
    Ok(Some(passed))
}

/// Reads `length` bytes of `piece` at `offset`. Pad files read as zeros.
fn read_block(
    storage: &Storage,
    pool: &FilePool,
    piece: u32,
    offset: u32,
    length: u32,
) -> Result<Vec<u8>, Error> {
    let mut buf = vec![0u8; length as usize];
    read_into(storage, pool, piece, offset, &mut buf)?;
    Ok(buf)
}

fn read_into(
    storage: &Storage,
    pool: &FilePool,
    piece: u32,
    offset: u32,
    buf: &mut [u8],
) -> Result<(), Error> {
    let fs = storage.info.files();
    let mut pos = 0usize;
    for slice in fs.map_block(piece, offset as u64, buf.len() as u64) {
        let n = slice.size as usize;
        if !storage.skip_file(slice.file_index) {
            let file =
                pool.open(storage.id, slice.file_index as u32, &storage.file_path(slice.file_index), false)?;
            if read_at(&file, &mut buf[pos..pos + n], slice.offset)? != n {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "file too short").into());
            }
        }
        pos += n;
    }
    if pos != buf.len() {
        return Err(Error::Io {
            kind: io::ErrorKind::InvalidInput,
            message: "read past the end of the torrent".into(),
        });
    }
    Ok(())
}

fn check_files(storage: &Storage, ctx: &Ctx, mode: CheckMode) -> Result<CheckResult, (String, Error)> {
    let fs = storage.info.files();
    let num_pieces = storage.info.num_pieces();
    let mut resume_failed = false;

    // Empty files hold no pieces, so nothing else would ever create them.
    for f in 0..fs.num_files() {
        if fs.file_size(f) == 0 && !storage.skip_file(f) {
            let path = storage.file_path(f);
            if !path.exists() {
                ctx.pool.open(storage.id, f as u32, &path, true).map_err(|e| io_error(storage, f, e))?;
            }
        }
    }

    if let CheckMode::Resume(have) = mode {
        if have.len() == num_pieces as usize && resume_matches_files(storage, &have) {
            return Ok(CheckResult { have, resume_failed: false });
        }
        resume_failed = true;
    }

    let mut have = Bitfield::new(num_pieces as usize, false);
    // Nothing on disk: nothing to hash.
    let any_file = (0..fs.num_files())
        .any(|f| fs.file_size(f) > 0 && !storage.skip_file(f) && storage.file_path(f).exists());
    if !any_file {
        return Ok(CheckResult { have, resume_failed });
    }
    let mut buf = vec![0u8; storage.info.piece_length() as usize];
    let report_every = (num_pieces / 100).max(16);
    for piece in 0..num_pieces {
        if storage.abort.load(Ordering::Relaxed) {
            break;
        }
        let size = storage.info.piece_size(piece) as usize;
        if read_into(storage, &ctx.pool, piece, 0, &mut buf[..size]).is_ok()
            && Some(Sha1::digest(&buf[..size])) == storage.info.hash_for_piece(piece)
        {
            have.set(piece as usize, true);
        }
        if (piece + 1) % report_every == 0 {
            ctx.send(Completion::CheckProgress { storage: storage.id, checked: piece + 1 });
        }
    }
    // Checking may open hundreds of files; don't keep them all.
    ctx.pool.release(storage.id);
    Ok(CheckResult { have, resume_failed })
}

/// libtorrent `posix_storage::verify_resume_data`, reduced to what pread storage needs: every
/// file that holds part of a piece we claim to have must exist and reach that far.
fn resume_matches_files(storage: &Storage, have: &Bitfield) -> bool {
    let fs = storage.info.files();
    let piece_len = storage.info.piece_length() as u64;
    let mut need = vec![0u64; fs.num_files()];
    for piece in (0..have.len()).filter(|&p| have.get(p)) {
        let size = storage.info.piece_size(piece as u32) as u64;
        for s in fs.map_block(piece as u32, 0, size) {
            need[s.file_index] = need[s.file_index].max(s.offset + s.size);
        }
        debug_assert!(piece as u64 * piece_len < fs.total_size());
    }
    need.iter().enumerate().all(|(f, &n)| {
        n == 0 || storage.skip_file(f) || std::fs::metadata(storage.file_path(f)).is_ok_and(|m| m.len() >= n)
    })
}

/// Deletes the torrent's files, then the directories that became empty (deepest first),
/// never the save path itself.
fn delete_files(storage: &Storage) -> Result<(), Error> {
    let fs = storage.info.files();
    let mut dirs = std::collections::BTreeSet::new();
    let mut first_error = None;
    let save = PathBuf::from(&storage.save_path);
    for f in 0..fs.num_files() {
        if fs.pad_file_at(f) {
            continue;
        }
        let path = storage.file_path(f);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                first_error.get_or_insert(Error::from(e));
            }
        }
        let mut p = path.parent();
        while let Some(dir) = p {
            if !dir.starts_with(&save) || dir == save {
                break;
            }
            dirs.insert(dir.to_path_buf());
            p = dir.parent();
        }
    }
    for dir in dirs.iter().rev() {
        let _ = std::fs::remove_dir(dir);
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::bencode::Entry;
    use std::time::Duration;

    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lt-test-{name}-{}-{}",
            std::process::id(),
            crate::util::random::random_u64()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A multi-file torrent with deterministic contents. Returns (metadata, file contents).
    pub(crate) fn make_torrent(files: &[(&str, usize)], piece_length: u32) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut contents = Vec::new();
        let mut all = Vec::new();
        for (i, &(_, size)) in files.iter().enumerate() {
            let data: Vec<u8> = (0..size).map(|j| ((j * 31 + i * 7) % 251) as u8).collect();
            all.extend_from_slice(&data);
            contents.push(data);
        }
        let mut pieces = Vec::new();
        for chunk in all.chunks(piece_length as usize) {
            pieces.extend_from_slice(&Sha1::digest(chunk).0);
        }
        let mut info = Entry::new_dict();
        let mut list = Entry::new_list();
        for &(path, size) in files {
            let mut f = Entry::new_dict();
            *f.key("length") = Entry::Int(size as i64);
            let mut p = Entry::new_list();
            for part in path.split('/') {
                p.list_mut().push(Entry::Bytes(part.as_bytes().to_vec()));
            }
            *f.key("path") = p;
            list.list_mut().push(f);
        }
        *info.key("files") = list;
        *info.key("name") = Entry::Bytes(b"test".to_vec());
        *info.key("piece length") = Entry::Int(piece_length as i64);
        *info.key("pieces") = Entry::Bytes(pieces);
        let mut t = Entry::new_dict();
        *t.key("info") = info;
        (t.bencode(), contents)
    }

    struct Rig {
        disk: DiskIo,
        rx: mpsc::Receiver<Completion>,
        storage: Arc<Storage>,
        dir: PathBuf,
        contents: Vec<Vec<u8>>,
    }

    fn rig(name: &str, files: &[(&str, usize)], piece_length: u32) -> Rig {
        let (bytes, contents) = make_torrent(files, piece_length);
        let info = Arc::new(TorrentInfo::from_bytes(&bytes).unwrap());
        let dir = temp_dir(name);
        let (tx, rx) = mpsc::channel();
        let mut disk = DiskIo::new(3, 8, tx, Arc::new(|| {}));
        let storage = disk.new_storage(dir.to_string_lossy().into_owned(), info);
        Rig { disk, rx, storage, dir, contents }
    }

    fn next(rx: &mpsc::Receiver<Completion>) -> Completion {
        rx.recv_timeout(Duration::from_secs(10)).expect("disk completion")
    }

    fn torrent_bytes(contents: &[Vec<u8>]) -> Vec<u8> {
        contents.concat()
    }

    #[test]
    fn write_hash_read_check_and_delete() {
        // 3 pieces of 32 KiB over three files, the last piece short.
        let r = rig("disk", &[("a/one", 20_000), ("two", 40_000), ("a/b/three", 15_000)], 32 * 1024);
        let all = torrent_bytes(&r.contents);
        let info = r.storage.info.clone();
        assert_eq!(info.num_pieces(), 3);

        // Write piece 1 out of order (block 1 first), piece 2 in order, piece 0 corrupted.
        let block = |p: u32, b: u32| {
            let start = p as usize * 32768 + b as usize * BLOCK_SIZE as usize;
            let end = (start + BLOCK_SIZE as usize)
                .min(all.len())
                .min(p as usize * 32768 + info.piece_size(p) as usize);
            all[start..end].to_vec()
        };
        r.disk.async_write(&r.storage, 1, BLOCK_SIZE, block(1, 1));
        r.disk.async_write(&r.storage, 1, 0, block(1, 0));
        r.disk.async_write(&r.storage, 2, 0, block(2, 0));
        let mut bad = block(0, 0);
        bad[5] ^= 1;
        r.disk.async_write(&r.storage, 0, 0, bad);
        r.disk.async_write(&r.storage, 0, BLOCK_SIZE, block(0, 1));

        let mut hashed = HashMap::new();
        let mut writes = 0;
        while hashed.len() < 3 {
            match next(&r.rx) {
                Completion::Write { error: None, .. } => writes += 1,
                Completion::Hashed { piece, passed, .. } => {
                    hashed.insert(piece, passed);
                }
                c => panic!("unexpected {c:?}"),
            }
        }
        assert_eq!(hashed, HashMap::from([(0, false), (1, true), (2, true)]));
        assert!(writes >= 4);
        assert_eq!(r.disk.queued_write_bytes(), 0);

        // Fix piece 0, then read across the file boundary in piece 0 / 1.
        r.disk.async_write(&r.storage, 0, 0, block(0, 0));
        r.disk.async_write(&r.storage, 0, BLOCK_SIZE, block(0, 1));
        loop {
            if let Completion::Hashed { piece: 0, passed, .. } = next(&r.rx) {
                assert!(passed);
                break;
            }
        }
        r.disk.async_read(&r.storage, 0, BLOCK_SIZE, BLOCK_SIZE, 7);
        match next(&r.rx) {
            Completion::Read { cookie: 7, result: Ok(data), .. } => assert_eq!(data, block(0, 1)),
            c => panic!("unexpected {c:?}"),
        }
        assert_eq!(std::fs::read(r.dir.join("test/two")).unwrap(), r.contents[1]);

        // A full check finds all three pieces; resume data that matches is trusted.
        r.disk.async_check(&r.storage, CheckMode::Full);
        let have = loop {
            if let Completion::Checked { result, .. } = next(&r.rx) {
                break result.unwrap().have;
            }
        };
        assert!(have.all_set());
        r.disk.async_check(&r.storage, CheckMode::Resume(have.clone()));
        match next(&r.rx) {
            Completion::Checked { result: Ok(res), .. } => assert!(!res.resume_failed && res.have.all_set()),
            c => panic!("unexpected {c:?}"),
        }

        // Truncate a file: resume verification falls back to a full check and loses pieces.
        let f = std::fs::OpenOptions::new().write(true).open(r.dir.join("test/a/b/three")).unwrap();
        f.set_len(100).unwrap();
        drop(f);
        r.disk.async_fence(&r.storage, FenceAction::Release);
        assert!(matches!(next(&r.rx), Completion::Fenced { action: FenceAction::Release, error: None, .. }));
        let storage2 = Arc::new(Storage::new(99, r.storage.save_path.clone(), info.clone()));
        r.disk.async_check(&storage2, CheckMode::Resume(have));
        match next(&r.rx) {
            Completion::Checked { result: Ok(res), .. } => {
                assert!(res.resume_failed);
                // Pieces 1 and 2 both reach into the truncated file.
                assert_eq!(res.have.iter().collect::<Vec<_>>(), [true, false, false]);
            }
            c => panic!("unexpected {c:?}"),
        }

        r.disk.async_fence(&storage2, FenceAction::Delete);
        assert!(matches!(next(&r.rx), Completion::Fenced { action: FenceAction::Delete, error: None, .. }));
        assert!(!r.dir.join("test").exists(), "files and empty directories are gone");
        assert!(r.dir.exists(), "the save path stays");
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn restored_blocks_are_read_back_for_the_hash() {
        // One piece of 4 blocks, one of 2.
        let r = rig("disk-restore", &[("f", 6 * BLOCK_SIZE as usize)], 4 * BLOCK_SIZE);
        let all = torrent_bytes(&r.contents);
        let block = |p: u32, b: u32| {
            let start = (p * 4 + b) as usize * BLOCK_SIZE as usize;
            all[start..start + BLOCK_SIZE as usize].to_vec()
        };
        let hashed = |rx: &mpsc::Receiver<Completion>| loop {
            if let Completion::Hashed { piece, passed, .. } = next(rx) {
                break (piece, passed);
            }
        };
        // A previous session wrote blocks 0 and 2 of piece 0, then stopped.
        let old = Arc::new(Storage::new(50, r.storage.save_path.clone(), r.storage.info.clone()));
        r.disk.async_write(&old, 0, 0, block(0, 0));
        r.disk.async_write(&old, 0, 2 * BLOCK_SIZE, block(0, 2));
        r.disk.async_fence(&old, FenceAction::Release);
        while !matches!(next(&r.rx), Completion::Fenced { .. }) {}

        // This session restores them and downloads the other two, last block first.
        r.disk.async_restore_partial(&r.storage, 0, vec![true, false, true, false]);
        r.disk.async_write(&r.storage, 0, 3 * BLOCK_SIZE, block(0, 3));
        r.disk.async_write(&r.storage, 0, BLOCK_SIZE, block(0, 1));
        assert_eq!(hashed(&r.rx), (0, true));

        // A piece that was complete but not hashed at shutdown is hashed on restore.
        r.disk.async_write(&old, 1, 0, block(1, 0));
        r.disk.async_write(&old, 1, BLOCK_SIZE, block(1, 1));
        r.disk.async_fence(&old, FenceAction::Release);
        while !matches!(next(&r.rx), Completion::Fenced { .. }) {}
        r.disk.async_restore_partial(&r.storage, 1, vec![true, true]);
        assert_eq!(hashed(&r.rx), (1, true));

        // A restored block the file does not hold fails the piece instead of the torrent.
        r.disk.async_fence(&r.storage, FenceAction::Release);
        while !matches!(next(&r.rx), Completion::Fenced { .. }) {}
        std::fs::OpenOptions::new().write(true).open(r.dir.join("test/f")).unwrap().set_len(100).unwrap();
        let fresh = Arc::new(Storage::new(51, r.storage.save_path.clone(), r.storage.info.clone()));
        r.disk.async_restore_partial(&fresh, 0, vec![true, true, false, false]);
        r.disk.async_write(&fresh, 0, 2 * BLOCK_SIZE, block(0, 2));
        r.disk.async_write(&fresh, 0, 3 * BLOCK_SIZE, block(0, 3));
        assert_eq!(hashed(&r.rx), (0, false));
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn check_without_files_is_instant() {
        let r = rig("disk-empty", &[("x", 100_000), ("empty", 0)], 16 * 1024);
        r.disk.async_check(&r.storage, CheckMode::Full);
        match next(&r.rx) {
            Completion::Checked { result: Ok(res), .. } => assert!(res.have.none_set()),
            c => panic!("unexpected {c:?}"),
        }
        assert_eq!(std::fs::metadata(r.dir.join("test/empty")).unwrap().len(), 0, "empty files are created");
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}
