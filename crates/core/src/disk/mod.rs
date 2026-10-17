// Ported from libtorrent-rasterbar 2.0.11: src/mmap_disk_io.cpp (job, fence and thread-pool
// structure), src/posix_storage.cpp, src/storage_utils.cpp, src/disk_job_fence.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: positional read/write instead of mmap (PLAN.md D3). Jobs for a piece always run on
// the same thread (piece % threads), so per-piece hash state needs no locks; a fence is a job
// broadcast to every thread whose last finisher runs the fenced action. Blocks are hashed as
// they are written, in order, so a completed piece is never read back unless its blocks arrived
// out of order and did not fit in the small reorder cache. Blocks restored from resume data
// (`unfinished`) are on disk but not in any hash state, so they are read back as the cursor
// reaches them. File priority changes (libtorrent `set_file_priority`, a fence job) are a
// barrier: every thread parks the storage's later jobs until the last one has run the change.

//! Disk I/O: a thread pool doing file reads, writes, piece hashing and file checking for the
//! network thread.

pub(crate) mod file_pool;
pub(crate) mod part_file;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};
use std::thread::JoinHandle;

use crate::crypto::Sha1;
use crate::error::Error;
use crate::metainfo::TorrentInfo;
use crate::metainfo::file_storage::FileFlags;
use crate::util::bitfield::Bitfield;
use file_pool::{FilePool, read_at, write_at};
use part_file::{PartFile, PartFiles};

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
    files: Mutex<FileState>,
    part_files: Arc<PartFiles>,
}

/// libtorrent `posix_storage::m_file_priority`, `m_use_partfile` and `m_part_file`.
struct FileState {
    priority: Vec<u8>,
    /// False for a skipped file that existed on disk when it was skipped: it keeps its blocks
    /// (libtorrent's compatibility with versions without part files).
    use_partfile: Vec<bool>,
    part: Option<Arc<PartFile>>,
}

impl Storage {
    fn new(
        id: u64,
        save_path: String,
        info: Arc<TorrentInfo>,
        mut priority: Vec<u8>,
        part_files: Arc<PartFiles>,
    ) -> Storage {
        let n = info.files().num_files();
        priority.resize(n.max(priority.len()), DEFAULT_PRIORITY);
        let files = Mutex::new(FileState { priority, use_partfile: vec![true; n], part: None });
        Storage { id, save_path, info, abort: AtomicBool::new(false), files, part_files }
    }

    fn file_path(&self, file: usize) -> PathBuf {
        PathBuf::from(self.info.files().file_path(file, &self.save_path))
    }

    fn skip_file(&self, file: usize) -> bool {
        let fs = self.info.files();
        fs.file_flags(file).intersects(FileFlags::PAD_FILE | FileFlags::SYMLINK)
    }

    fn lock_files(&self) -> MutexGuard<'_, FileState> {
        self.files.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// libtorrent `m_part_file_name`.
    fn part_file_name(&self) -> String {
        format!(".{}.parts", self.info.info_hash().to_hex())
    }

    fn part_file_path(&self) -> PathBuf {
        PathBuf::from(&self.save_path).join(self.part_file_name())
    }

    /// libtorrent `need_partfile`: the part file object (the file itself is created lazily).
    fn need_partfile(&self, st: &mut FileState) -> Arc<PartFile> {
        st.part
            .get_or_insert_with(|| {
                self.part_files.get_or_open(
                    std::path::Path::new(&self.save_path),
                    &self.part_file_name(),
                    self.info.num_pieces(),
                    self.info.piece_length(),
                )
            })
            .clone()
    }

    /// The part file, if `file`'s blocks go there: it has priority 0 and was not on disk.
    fn part_file_for(&self, file: usize) -> Option<Arc<PartFile>> {
        let mut st = self.lock_files();
        if st.priority.get(file).copied() == Some(0) && st.use_partfile.get(file).copied().unwrap_or(true) {
            Some(self.need_partfile(&mut st))
        } else {
            None
        }
    }

    fn part_error(&self, e: io::Error) -> (String, Error) {
        (self.part_file_path().to_string_lossy().into_owned(), Error::from(e))
    }
}

/// libtorrent `default_priority`.
const DEFAULT_PRIORITY: u8 = 4;

#[derive(Debug)]
pub(crate) enum CheckMode {
    /// Hash every piece (libtorrent `check_files` without resume data, or `force_recheck`).
    Full,
    /// Trust these pieces if every file holding them exists and is long enough; otherwise fall
    /// back to a full check (libtorrent `verify_resume_data`).
    Resume(Bitfield),
}

enum Job {
    Write {
        storage: Arc<Storage>,
        piece: u32,
        offset: u32,
        data: Vec<u8>,
    },
    Read {
        storage: Arc<Storage>,
        piece: u32,
        offset: u32,
        length: u32,
        cookie: u64,
    },
    Restore {
        storage: Arc<Storage>,
        piece: u32,
        blocks: Vec<bool>,
    },
    Check {
        storage: Arc<Storage>,
        mode: CheckMode,
    },
    Fence {
        storage: Arc<Storage>,
        fence: Arc<Fence>,
    },
    Barrier {
        storage: Arc<Storage>,
        barrier: Arc<Barrier>,
    },
    /// The barrier `seq` of `storage` is done: run the jobs parked behind it.
    Unblock {
        storage: u64,
        seq: u64,
    },
    Shutdown,
}

impl Job {
    fn storage_id(&self) -> Option<u64> {
        match self {
            Job::Write { storage, .. }
            | Job::Read { storage, .. }
            | Job::Restore { storage, .. }
            | Job::Check { storage, .. }
            | Job::Fence { storage, .. }
            | Job::Barrier { storage, .. } => Some(storage.id),
            Job::Unblock { .. } | Job::Shutdown => None,
        }
    }
}

/// A file priority change, run by the last thread to reach it while the others hold back the
/// storage's later jobs.
struct Barrier {
    remaining: AtomicUsize,
    seq: u64,
    priorities: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FenceAction {
    /// Close the files and drop all cached state (libtorrent `release_files`, `stop_torrent`).
    Release,
    /// Release, then delete the files, any directories left empty and the part file
    /// (libtorrent `delete_files`).
    Delete,
    /// Release, then delete only the part file (libtorrent `delete_partfile`).
    DeletePartFile,
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
    /// Answer to [`DiskIo::async_set_file_priority`]; on error the change stopped at that file.
    FilePriority {
        storage: u64,
        error: Option<(String, Error)>,
    },
}

pub(crate) struct DiskIo {
    threads: Vec<mpsc::Sender<Job>>,
    hasher: mpsc::Sender<Job>,
    handles: Vec<JoinHandle<()>>,
    pool: Arc<FilePool>,
    part_files: Arc<PartFiles>,
    queued_write_bytes: Arc<AtomicU64>,
    next_storage_id: u64,
    next_barrier: u64,
}

struct Ctx {
    pool: Arc<FilePool>,
    done: mpsc::Sender<Completion>,
    wake: Arc<dyn Fn() + Send + Sync>,
    queued_write_bytes: Arc<AtomicU64>,
    /// Every thread's queue, for [`Job::Unblock`].
    queues: Arc<OnceLock<Vec<mpsc::Sender<Job>>>>,
    /// Index of this thread in `queues`.
    index: usize,
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
        let queues = Arc::new(OnceLock::new());
        let ctx = |index| Ctx {
            pool: pool.clone(),
            done: done.clone(),
            wake: wake.clone(),
            queued_write_bytes: queued_write_bytes.clone(),
            queues: queues.clone(),
            index,
        };
        let mut senders = Vec::new();
        let mut handles = Vec::new();
        let spawn = |name: String, rx: mpsc::Receiver<Job>, ctx: Ctx| {
            std::thread::Builder::new()
                .name(name)
                .spawn(move || disk_thread(rx, ctx))
                .expect("failed to spawn a disk thread")
        };
        let n = threads.clamp(1, 64);
        for i in 0..n {
            let (tx, rx) = mpsc::channel();
            handles.push(spawn(format!("lt-disk-{i}"), rx, ctx(i)));
            senders.push(tx);
        }
        let (hasher, rx) = mpsc::channel();
        handles.push(spawn("lt-hash".into(), rx, ctx(n)));
        let _ = queues.set(senders.iter().chain([&hasher]).cloned().collect());
        DiskIo {
            threads: senders,
            hasher,
            handles,
            pool,
            part_files: Arc::new(PartFiles::default()),
            queued_write_bytes,
            next_storage_id: 1,
            next_barrier: 1,
        }
    }

    /// A storage for `info` in `save_path`. Files whose `file_priority` is 0 keep their blocks
    /// in the part file.
    pub fn new_storage(
        &mut self,
        save_path: String,
        info: Arc<TorrentInfo>,
        file_priority: Vec<u8>,
    ) -> Arc<Storage> {
        let id = self.next_storage_id;
        self.next_storage_id += 1;
        Arc::new(Storage::new(id, save_path, info, file_priority, self.part_files.clone()))
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

    /// Applies new file priorities after every job queued so far for `storage`, and before any
    /// queued later (libtorrent `async_set_file_priority`, a fence job). A file leaving priority
    /// 0 gets its data moved out of the part file; one going to 0 starts using the part file if
    /// it does not exist. Answered by [`Completion::FilePriority`].
    pub fn async_set_file_priority(&mut self, storage: &Arc<Storage>, priorities: Vec<u8>) {
        let seq = self.next_barrier;
        self.next_barrier += 1;
        let barrier =
            Arc::new(Barrier { remaining: AtomicUsize::new(self.threads.len() + 1), seq, priorities });
        for tx in self.threads.iter().chain([&self.hasher]) {
            let _ = tx.send(Job::Barrier { storage: storage.clone(), barrier: barrier.clone() });
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

/// One disk thread's state.
#[derive(Default)]
struct Worker {
    partial: HashMap<(u64, u32), PartialHash>,
    cached_bytes: usize,
    /// Storages behind a barrier this thread has passed, by barrier sequence number.
    blocked: HashMap<u64, u64>,
    /// Their jobs, in arrival order.
    parked: Vec<Job>,
}

fn disk_thread(rx: mpsc::Receiver<Job>, ctx: Ctx) {
    let mut w = Worker::default();
    while let Ok(job) = rx.recv() {
        if matches!(job, Job::Shutdown) {
            break;
        }
        w.handle(job, &ctx);
    }
}

impl Worker {
    fn handle(&mut self, job: Job, ctx: &Ctx) {
        if job.storage_id().is_some_and(|id| self.blocked.contains_key(&id)) {
            self.parked.push(job);
            return;
        }
        let partial = &mut self.partial;
        let cached_bytes = &mut self.cached_bytes;
        match job {
            Job::Write { storage, piece, offset, data } => {
                let length = data.len() as u32;
                let result = write_block(&storage, ctx, partial, cached_bytes, piece, offset, data);
                ctx.queued_write_bytes.fetch_sub(length as u64, Ordering::Relaxed);
                match result {
                    Ok(hashed) => {
                        ctx.send(Completion::Write { storage: storage.id, piece, offset, error: None });
                        if let Some(passed) = hashed {
                            ctx.send(Completion::Hashed { storage: storage.id, piece, passed });
                        }
                    }
                    Err(e) => {
                        drop_partial(partial, cached_bytes, storage.id, piece);
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
                match advance_hash(&storage, ctx, partial, cached_bytes, piece) {
                    Ok(Some(passed)) => ctx.send(Completion::Hashed { storage: storage.id, piece, passed }),
                    Ok(None) => {}
                    Err(_) => {
                        // Only possible if writes were queued before the restore: fail the piece.
                        drop_partial(partial, cached_bytes, storage.id, piece);
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
                    let result = check_files(&storage, ctx, mode);
                    ctx.send(Completion::Checked { storage: storage.id, result });
                }
            }
            Job::Fence { storage, fence } => {
                let id = storage.id;
                let keys: Vec<_> = partial.keys().filter(|k| k.0 == id).copied().collect();
                for (_, piece) in keys {
                    drop_partial(partial, cached_bytes, id, piece);
                }
                if fence.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                    ctx.pool.release(id);
                    if let Some(part) = storage.lock_files().part.take() {
                        // libtorrent ignores this error too; the header is rewritten next time.
                        let _ = part.release();
                    }
                    let error = match fence.action {
                        FenceAction::Release => None,
                        FenceAction::Delete => delete_files(&storage).err(),
                        FenceAction::DeletePartFile => delete_part_file(&storage).err().map(Error::from),
                    };
                    ctx.send(Completion::Fenced { storage: id, action: fence.action, error });
                }
            }
            Job::Barrier { storage, barrier } => {
                if barrier.remaining.fetch_sub(1, Ordering::AcqRel) != 1 {
                    self.blocked.insert(storage.id, barrier.seq);
                    return;
                }
                // Every thread has passed it: nothing else of this storage is running.
                let error = set_file_priority(&storage, ctx, &barrier.priorities).err();
                ctx.send(Completion::FilePriority { storage: storage.id, error });
                for (i, tx) in ctx.queues.get().into_iter().flatten().enumerate() {
                    if i != ctx.index {
                        let _ = tx.send(Job::Unblock { storage: storage.id, seq: barrier.seq });
                    }
                }
            }
            Job::Unblock { storage, seq } => {
                if self.blocked.get(&storage) != Some(&seq) {
                    return;
                }
                self.blocked.remove(&storage);
                let (mine, rest) = std::mem::take(&mut self.parked)
                    .into_iter()
                    .partition(|j| j.storage_id() == Some(storage));
                self.parked = rest;
                // A parked barrier blocks the storage again and parks the rest anew, in order.
                for job in mine {
                    self.handle(job, ctx);
                }
            }
            Job::Shutdown => {}
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
        if storage.skip_file(slice.file_index) {
            // pad files are not written
        } else if let Some(part) = storage.part_file_for(slice.file_index) {
            part.write(&data[pos..pos + n], piece, offset + pos as u32).map_err(|e| storage.part_error(e))?;
        } else {
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
        if storage.skip_file(slice.file_index) {
            // pad files read as zeros (the buffer may hold an earlier piece)
            buf[pos..pos + n].fill(0);
        } else if let Some(part) = storage.part_file_for(slice.file_index) {
            part.read(&mut buf[pos..pos + n], piece, offset + pos as u32)?;
        } else {
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

    // libtorrent `posix_storage::initialize`: a skipped file that exists keeps its blocks; the
    // others go to the part file.
    let skipped = {
        let mut st = storage.lock_files();
        let skipped: Vec<bool> =
            (0..fs.num_files()).map(|f| st.priority[f] == 0 && !fs.pad_file_at(f)).collect();
        for (f, _) in skipped.iter().enumerate().filter(|(_, s)| **s) {
            st.use_partfile[f] = !storage.file_path(f).exists();
            if st.use_partfile[f] && fs.file_size(f) > 0 {
                storage.need_partfile(&mut st);
            }
        }
        skipped
    };

    export_unskipped(storage, ctx)?;

    // Empty files hold no pieces, so nothing else would ever create them. Skipped ones are not
    // created.
    for (f, &skip) in skipped.iter().enumerate() {
        if fs.file_size(f) == 0 && !storage.skip_file(f) && !skip {
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
    let any_file = storage.lock_files().part.as_ref().is_some_and(|p| !p.is_empty())
        || (0..fs.num_files())
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
/// file that holds part of a piece we claim to have must exist and reach that far. Where
/// libtorrent exempts skipped files, the part file must hold the piece.
fn resume_matches_files(storage: &Storage, have: &Bitfield) -> bool {
    let fs = storage.info.files();
    let piece_len = storage.info.piece_length() as u64;
    let mut need = vec![0u64; fs.num_files()];
    for piece in (0..have.len()).filter(|&p| have.get(p)) {
        let size = storage.info.piece_size(piece as u32) as u64;
        for s in fs.map_block(piece as u32, 0, size) {
            if storage.skip_file(s.file_index) {
                continue;
            }
            match storage.part_file_for(s.file_index) {
                Some(part) if !part.has_piece(piece as u32) => return false,
                Some(_) => {}
                None => need[s.file_index] = need[s.file_index].max(s.offset + s.size),
            }
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
    if let Err(e) = delete_part_file(storage) {
        first_error.get_or_insert(Error::from(e));
    }
    first_error.map_or(Ok(()), Err)
}

/// Deletes the part file. One still open elsewhere forgets its pieces, so it is not written
/// back.
fn delete_part_file(storage: &Storage) -> io::Result<()> {
    let part = storage.part_file_path();
    let removed = match storage.part_files.get(&part) {
        Some(pf) => pf.delete(),
        None => std::fs::remove_file(&part),
    };
    match removed {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// libtorrent `posix_storage::set_file_priority`. Runs with no other job of `storage` in
/// flight. Pieces left in the part file that no skipped file needs any more are freed.
fn set_file_priority(storage: &Storage, ctx: &Ctx, priorities: &[u8]) -> Result<(), (String, Error)> {
    let fs = storage.info.files();
    let mut st = storage.lock_files();
    for (f, &new) in priorities.iter().enumerate().take(fs.num_files()) {
        // pad files always have priority 0
        if fs.pad_file_at(f) {
            continue;
        }
        let old = st.priority[f];
        if old == 0 && new != 0 {
            if let (true, Some(part)) = (st.use_partfile[f], st.part.clone()) {
                export_file(storage, ctx, &part, f)?;
            }
        } else if old != 0 && new == 0 {
            // libtorrent does not move existing data into the part file: a file already on
            // disk keeps its blocks.
            st.use_partfile[f] = !storage.file_path(f).exists();
        }
        st.priority[f] = new;
        if new == 0 && st.use_partfile[f] && fs.file_size(f) > 0 {
            storage.need_partfile(&mut st);
        }
    }
    if let Some(part) = st.part.clone() {
        free_unneeded(storage, &st, &part)?;
    }
    Ok(())
}

/// Moves file `f`'s blocks out of the part file into the file.
fn export_file(storage: &Storage, ctx: &Ctx, part: &PartFile, f: usize) -> Result<(), (String, Error)> {
    let fs = storage.info.files();
    let path = storage.file_path(f);
    part.export_file(fs.file_offset(f), fs.file_size(f), |offset, buf| {
        let file = ctx.pool.open(storage.id, f as u32, &path, true)?;
        write_at(&file, buf, offset)
    })
    .map_err(|e| io_error(storage, f, e))
}

/// Frees the part file's pieces that overlap no file kept in it, then flushes the header (which
/// deletes the file once it is empty).
fn free_unneeded(storage: &Storage, st: &FileState, part: &PartFile) -> Result<(), (String, Error)> {
    let fs = storage.info.files();
    let uses_part = |f: usize| st.priority[f] == 0 && st.use_partfile[f] && !fs.pad_file_at(f);
    part.retain(|piece| {
        piece < storage.info.num_pieces()
            && fs
                .map_block(piece, 0, storage.info.piece_size(piece) as u64)
                .iter()
                .any(|s| uses_part(s.file_index))
    });
    part.flush_metadata().map_err(|e| storage.part_error(e))
}

/// A torrent has no storage while it is stopped, so a file unskipped then still has its
/// blocks in the part file: move them out before the check, as `set_file_priority` would have.
fn export_unskipped(storage: &Storage, ctx: &Ctx) -> Result<(), (String, Error)> {
    let fs = storage.info.files();
    let mut st = storage.lock_files();
    if st.part.is_none() && !storage.part_file_path().exists() {
        return Ok(());
    }
    let part = storage.need_partfile(&mut st);
    if part.is_empty() {
        return Ok(());
    }
    for f in 0..fs.num_files() {
        if st.priority[f] != 0
            && !storage.skip_file(f)
            && fs.file_size(f) > 0
            && !storage.file_path(f).exists()
        {
            export_file(storage, ctx, &part, f)?;
        }
    }
    free_unneeded(storage, &st, &part)
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
        rig_with(name, files, piece_length, Vec::new())
    }

    fn rig_with(name: &str, files: &[(&str, usize)], piece_length: u32, priorities: Vec<u8>) -> Rig {
        let (bytes, contents) = make_torrent(files, piece_length);
        let info = Arc::new(TorrentInfo::from_bytes(&bytes).unwrap());
        let dir = temp_dir(name);
        let (tx, rx) = mpsc::channel();
        let mut disk = DiskIo::new(3, 8, tx, Arc::new(|| {}));
        let storage = disk.new_storage(dir.to_string_lossy().into_owned(), info, priorities);
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
        let storage2 =
            Arc::new(Storage::new(99, r.storage.save_path.clone(), info.clone(), Vec::new(), Arc::default()));
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
        let old = Arc::new(Storage::new(
            50,
            r.storage.save_path.clone(),
            r.storage.info.clone(),
            Vec::new(),
            Arc::default(),
        ));
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
        let fresh = Arc::new(Storage::new(
            51,
            r.storage.save_path.clone(),
            r.storage.info.clone(),
            Vec::new(),
            Arc::default(),
        ));
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

    fn checked(rx: &mpsc::Receiver<Completion>) -> CheckResult {
        loop {
            if let Completion::Checked { result, .. } = next(rx) {
                break result.unwrap();
            }
        }
    }

    fn wanted(have: &Bitfield) -> Vec<usize> {
        (0..have.len()).filter(|&p| have.get(p)).collect()
    }

    /// Writes every piece in `pieces` (one block each) and waits for their hashes.
    fn write_pieces(r: &Rig, storage: &Arc<Storage>, pieces: &[u32]) {
        let all = torrent_bytes(&r.contents);
        let pl = r.storage.info.piece_length() as usize;
        for &p in pieces {
            let start = p as usize * pl;
            let end = (start + pl).min(all.len());
            r.disk.async_write(storage, p, 0, all[start..end].to_vec());
        }
        let mut left: std::collections::HashSet<u32> = pieces.iter().copied().collect();
        while !left.is_empty() {
            match next(&r.rx) {
                Completion::Hashed { piece, passed, .. } => {
                    assert!(passed, "piece {piece} failed");
                    left.remove(&piece);
                }
                Completion::Write { error: None, .. } | Completion::FilePriority { error: None, .. } => {}
                c => panic!("unexpected {c:?}"),
            }
        }
    }

    // a: pieces 0..=2, b: 2..=5, c: 5..=7, with 16 KiB pieces.
    const SKIP_MIDDLE: &[(&str, usize)] = &[("a", 40_000), ("b", 50_000), ("c", 30_000)];

    #[test]
    fn a_skipped_file_lives_in_the_part_file_until_it_is_wanted() {
        let r = rig_with("disk-part", SKIP_MIDDLE, BLOCK_SIZE, vec![4, 0, 4]);
        let info = r.storage.info.clone();
        let save = r.storage.save_path.clone();
        let part = r.dir.join(format!(".{}.parts", info.info_hash().to_hex()));
        let b = r.dir.join("test/b");
        assert_eq!(info.num_pieces(), 8);
        r.disk.async_check(&r.storage, CheckMode::Full);
        assert!(checked(&r.rx).have.none_set());
        assert!(!part.exists(), "the part file is created lazily");

        // The boundary pieces 2 and 5 pass; b is never created.
        write_pieces(&r, &r.storage, &[0, 1, 2, 5, 6, 7]);
        assert!(!b.exists(), "a skipped file is not created");
        assert_eq!(std::fs::read(r.dir.join("test/a")).unwrap(), r.contents[0]);
        assert_eq!(std::fs::read(r.dir.join("test/c")).unwrap(), r.contents[2]);
        r.disk.async_read(&r.storage, 2, 0, BLOCK_SIZE, 1);
        match next(&r.rx) {
            Completion::Read { result: Ok(data), .. } => {
                assert_eq!(data, torrent_bytes(&r.contents)[2 * 16384..3 * 16384])
            }
            c => panic!("unexpected {c:?}"),
        }

        // A recheck finds the boundary pieces through the part file; resume data is trusted.
        r.disk.async_check(&r.storage, CheckMode::Full);
        let have = checked(&r.rx).have;
        assert_eq!(wanted(&have), [0, 1, 2, 5, 6, 7]);
        r.disk.async_check(&r.storage, CheckMode::Resume(have.clone()));
        assert!(!checked(&r.rx).resume_failed);

        // Restart: the part file keeps its contents.
        r.disk.async_fence(&r.storage, FenceAction::Release);
        while !matches!(next(&r.rx), Completion::Fenced { .. }) {}
        assert!(part.exists());
        let Rig { mut disk, rx, storage, dir, contents } = r;
        drop(storage);
        let s2 = disk.new_storage(save.clone(), info.clone(), vec![4, 0, 4]);
        disk.async_check(&s2, CheckMode::Resume(have.clone()));
        let res = checked(&rx);
        assert!(!res.resume_failed);
        assert_eq!(wanted(&res.have), [0, 1, 2, 5, 6, 7]);
        disk.async_check(&s2, CheckMode::Full);
        assert_eq!(wanted(&checked(&rx).have), [0, 1, 2, 5, 6, 7]);

        // Unskip b, with its pieces queued right behind the change: they wait for the export.
        disk.async_set_file_priority(&s2, vec![4, 1, 4]);
        let mut r = Rig { disk, rx, storage: s2.clone(), dir, contents };
        write_pieces(&r, &s2, &[3, 4]);
        assert_eq!(std::fs::read(&b).unwrap(), r.contents[1], "exported and downloaded");
        assert!(!part.exists(), "no skipped file needs the part file any more");
        r.disk.async_check(&s2, CheckMode::Full);
        assert!(checked(&r.rx).have.all_set());

        // Skipping a file that exists keeps its data where it is.
        r.disk.async_set_file_priority(&s2, vec![4, 4, 0]);
        assert!(matches!(next(&r.rx), Completion::FilePriority { error: None, .. }));
        r.disk.async_check(&s2, CheckMode::Full);
        assert!(checked(&r.rx).have.all_set());
        assert!(!part.exists());

        r.disk.async_fence(&s2, FenceAction::Delete);
        assert!(matches!(next(&r.rx), Completion::Fenced { action: FenceAction::Delete, error: None, .. }));
        assert!(!r.dir.join("test").exists());
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn deleting_removes_the_part_file_and_resume_needs_its_pieces() {
        let files = &[("a", 40_000), ("b", 50_000), ("c", 30_000), ("empty", 0)];
        let mut r = rig_with("disk-part-delete", files, BLOCK_SIZE, vec![4, 0, 4, 0]);
        let info = r.storage.info.clone();
        let part = r.dir.join(format!(".{}.parts", info.info_hash().to_hex()));
        r.disk.async_check(&r.storage, CheckMode::Full);
        checked(&r.rx);
        assert!(!r.dir.join("test/empty").exists(), "a skipped empty file is not created");
        let storage = r.storage.clone();
        write_pieces(&r, &storage, &[1, 2]);
        assert!(part.exists());
        assert!(!r.dir.join("test/b").exists());

        // Resume data claiming piece 5 (partly in b) fails: the part file does not have it.
        let mut have = Bitfield::new(8, false);
        have.set(2, true);
        r.disk.async_check(&r.storage, CheckMode::Resume(have.clone()));
        assert!(!checked(&r.rx).resume_failed);
        have.set(5, true);
        r.disk.async_check(&r.storage, CheckMode::Resume(have));
        let res = checked(&r.rx);
        assert!(res.resume_failed);
        assert_eq!(wanted(&res.have), [1, 2]);

        // Remove deleting the part file only, while the old storage still holds it.
        let gone = r.disk.new_storage(r.storage.save_path.clone(), info.clone(), Vec::new());
        r.disk.async_fence(&gone, FenceAction::DeletePartFile);
        assert!(matches!(
            next(&r.rx),
            Completion::Fenced { action: FenceAction::DeletePartFile, error: None, .. }
        ));
        assert!(!part.exists());
        assert!(r.dir.join("test/a").exists(), "the files stay");

        // Remove with data.
        write_pieces(&r, &storage, &[2]);
        assert!(part.exists());
        let gone = r.disk.new_storage(r.storage.save_path.clone(), info, Vec::new());
        r.disk.async_fence(&gone, FenceAction::Delete);
        assert!(matches!(next(&r.rx), Completion::Fenced { action: FenceAction::Delete, error: None, .. }));
        assert!(!part.exists());
        assert!(!r.dir.join("test").exists());
        r.disk.async_fence(&r.storage, FenceAction::Release);
        while !matches!(next(&r.rx), Completion::Fenced { .. }) {}
        assert!(!part.exists(), "a released part file is not written back after the delete");
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn jobs_behind_a_priority_barrier_wait_for_it() {
        let r = rig("disk-barrier", &[("f", 2 * BLOCK_SIZE as usize)], BLOCK_SIZE);
        let all = torrent_bytes(&r.contents);
        let (tx, rx) = mpsc::channel();
        let ctx = Ctx {
            pool: Arc::new(FilePool::new(4)),
            done: tx,
            wake: Arc::new(|| {}),
            queued_write_bytes: Arc::new(AtomicU64::new(0)),
            queues: Arc::new(OnceLock::new()),
            index: 0,
        };
        let mut w = Worker::default();
        let s = r.storage.clone();
        let other =
            Arc::new(Storage::new(77, s.save_path.clone(), s.info.clone(), Vec::new(), Arc::default()));
        let barrier = |seq| Arc::new(Barrier { remaining: AtomicUsize::new(2), seq, priorities: vec![4] });
        // Another thread has yet to reach barrier 1: this one parks the storage's jobs.
        w.handle(Job::Barrier { storage: s.clone(), barrier: barrier(1) }, &ctx);
        w.handle(Job::Write { storage: s.clone(), piece: 0, offset: 0, data: all[..16384].to_vec() }, &ctx);
        w.handle(Job::Barrier { storage: s.clone(), barrier: barrier(2) }, &ctx);
        w.handle(Job::Write { storage: s.clone(), piece: 1, offset: 0, data: all[16384..].to_vec() }, &ctx);
        // Other storages are not held up.
        w.handle(Job::Read { storage: other, piece: 0, offset: 0, length: 1, cookie: 9 }, &ctx);
        assert!(matches!(rx.try_recv(), Ok(Completion::Read { cookie: 9, .. })));
        assert!(rx.try_recv().is_err());
        // An unblock for another barrier does nothing.
        w.handle(Job::Unblock { storage: s.id, seq: 2 }, &ctx);
        assert!(rx.try_recv().is_err());
        // Barrier 1 is done: piece 0 is written, then barrier 2 parks piece 1 again.
        w.handle(Job::Unblock { storage: s.id, seq: 1 }, &ctx);
        assert!(matches!(rx.try_recv(), Ok(Completion::Write { piece: 0, error: None, .. })));
        assert!(matches!(rx.try_recv(), Ok(Completion::Hashed { piece: 0, passed: true, .. })));
        assert!(rx.try_recv().is_err());
        w.handle(Job::Unblock { storage: s.id, seq: 2 }, &ctx);
        assert!(matches!(rx.try_recv(), Ok(Completion::Write { piece: 1, error: None, .. })));
        assert!(matches!(rx.try_recv(), Ok(Completion::Hashed { piece: 1, passed: true, .. })));
        assert!(w.parked.is_empty() && w.blocked.is_empty());
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn a_file_unskipped_while_stopped_is_exported_on_start() {
        let mut r = rig_with("disk-part-stopped", SKIP_MIDDLE, BLOCK_SIZE, vec![4, 0, 4]);
        let info = r.storage.info.clone();
        let part = r.dir.join(format!(".{}.parts", info.info_hash().to_hex()));
        r.disk.async_check(&r.storage, CheckMode::Full);
        checked(&r.rx);
        let storage = r.storage.clone();
        write_pieces(&r, &storage, &[2, 5]);
        r.disk.async_fence(&storage, FenceAction::Release);
        while !matches!(next(&r.rx), Completion::Fenced { .. }) {}
        drop(storage);
        assert!(part.exists());

        // Started again with b wanted: its boundary blocks move into b before the check.
        let s2 = r.disk.new_storage(r.storage.save_path.clone(), info, vec![4, 4, 4]);
        r.disk.async_check(&s2, CheckMode::Full);
        assert_eq!(wanted(&checked(&r.rx).have), [2, 5]);
        assert!(!part.exists());
        let b = std::fs::read(r.dir.join("test/b")).unwrap();
        let all = torrent_bytes(&r.contents);
        assert_eq!(b[..49152 - 40_000], all[40_000..49152]);
        assert_eq!(b[81920 - 40_000..], all[81920..90_000]);
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}
