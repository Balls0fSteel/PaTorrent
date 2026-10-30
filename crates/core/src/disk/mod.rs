// Ported from libtorrent-rasterbar 2.0.11: src/mmap_disk_io.cpp (job, fence and thread-pool
// structure), src/posix_storage.cpp, src/storage_utils.cpp, src/disk_job_fence.cpp;
// fixes from 2.0.12 (3417f6479d) and 2.1.2 (36582475fc)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: positional read/write instead of mmap (PLAN.md D3). Jobs for a piece always run on
// the same thread (piece % threads), so per-piece hash state needs no locks; a fence is a job
// broadcast to every thread whose last finisher runs the fenced action. Blocks are hashed as
// they are written, in order, so a completed piece is never read back unless its blocks arrived
// out of order and did not fit in the small reorder cache. Blocks restored from resume data
// (`unfinished`) are on disk but not in any hash state, so they are read back as the cursor
// reaches them. File priority changes (libtorrent `set_file_priority`, a fence job) are a
// barrier: every thread parks the storage's later jobs until the last one has run the change.
// Renames (libtorrent `rename_file`, a fence job) are barriers too, and keep the renamed layout
// beside the shared `TorrentInfo` as `posix_storage::m_mapped_files` does. Reads for peers go
// through each thread's read cache (`read_cache`, after libtorrent 1.2), which writes, fences
// and barriers of the storage clear.

//! Disk I/O: a thread pool doing file reads, writes, piece hashing and file checking for the
//! network thread.

pub(crate) mod file_pool;
pub(crate) mod part_file;
pub(crate) mod read_cache;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::alert::FileOperation;
use crate::crypto::{Sha1, Sha256};
use crate::error::Error;
use crate::metainfo::TorrentInfo;
use crate::metainfo::file_storage::{FileFlags, FileStorage};
use crate::metainfo::merkle::merkle_piece_root;
use crate::params::MoveFlags;
use crate::types::Sha256Hash;
use crate::util::bitfield::Bitfield;
use file_pool::{FilePool, read_at, write_at};
use part_file::{PartFile, PartFiles};
use read_cache::ReadCache;

/// The BitTorrent block size (libtorrent `default_block_size`).
pub(crate) const BLOCK_SIZE: u32 = 16 * 1024;

/// Out-of-order blocks each thread may keep in memory while waiting to hash them in order.
const REORDER_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// One torrent's files on disk (libtorrent `storage_interface` / `posix_storage`).
pub(crate) struct Storage {
    /// Unique for the life of the process; a restarted torrent gets a new one, so completions
    /// for an older instance can be told apart.
    pub id: u64,
    /// Changed only by a move, which runs with no other job of the storage in flight.
    save_path: RwLock<String>,
    pub info: Arc<TorrentInfo>,
    /// `info`'s files with the renames done by this storage (libtorrent `m_mapped_files`);
    /// `None` until a file is renamed. Changed only by a rename, which runs with no other job of
    /// the storage in flight.
    mapped: RwLock<Option<Arc<FileStorage>>>,
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
        let save_path = RwLock::new(save_path);
        let mapped = RwLock::new(None);
        Storage { id, save_path, info, mapped, abort: AtomicBool::new(false), files, part_files }
    }

    pub fn save_path(&self) -> String {
        self.save_path.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The renamed layout, if a file was renamed (libtorrent `posix_storage::files()`).
    fn mapped(&self) -> Option<Arc<FileStorage>> {
        self.mapped.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn file_path(&self, file: usize) -> PathBuf {
        let save_path = self.save_path.read().unwrap_or_else(|e| e.into_inner());
        match &*self.mapped.read().unwrap_or_else(|e| e.into_inner()) {
            Some(fs) => PathBuf::from(fs.file_path(file, &save_path)),
            None => PathBuf::from(self.info.files().file_path(file, &save_path)),
        }
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
        PathBuf::from(self.save_path()).join(self.part_file_name())
    }

    /// libtorrent `need_partfile`: the part file object (the file itself is created lazily).
    fn need_partfile(&self, st: &mut FileState) -> Arc<PartFile> {
        st.part
            .get_or_insert_with(|| {
                self.part_files.get_or_open(
                    std::path::Path::new(&self.save_path()),
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
    /// Hash a piece already on disk (a seed-mode piece before its first upload).
    Verify {
        storage: Arc<Storage>,
        piece: u32,
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
            | Job::Verify { storage, .. }
            | Job::Restore { storage, .. }
            | Job::Check { storage, .. }
            | Job::Fence { storage, .. }
            | Job::Barrier { storage, .. } => Some(storage.id),
            Job::Unblock { .. } | Job::Shutdown => None,
        }
    }
}

/// A file priority change or a move, run by the last thread to reach it while the others hold
/// back the storage's later jobs (libtorrent's fence jobs that keep the storage).
struct Barrier {
    remaining: AtomicUsize,
    seq: u64,
    action: BarrierAction,
}

enum BarrierAction {
    FilePriority(Vec<u8>),
    Move { path: String, flags: MoveFlags },
    Rename { file: u32, name: String },
}

/// libtorrent `status_t`, the values `move_storage` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MoveStatus {
    NoError,
    /// Moved (or not, for `ResetSavePath`), but the files must be checked at the new path.
    NeedFullCheck,
    /// `FailIfExist` found a file at the destination; nothing was moved.
    FileExist,
    FatalDiskError,
}

/// Why a move failed (libtorrent `storage_error`, with the file resolved to a path).
#[derive(Clone, Debug)]
pub(crate) struct MoveError {
    pub file: String,
    pub operation: FileOperation,
    pub error: Error,
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
    /// A hybrid piece whose v1 and v2 hashes disagreed; the check stopped there (libtorrent
    /// `handle_inconsistent_hashes`).
    pub inconsistent: Option<u32>,
}

/// How a piece compares with the metadata. v1 pieces are checked against their SHA-1, v2
/// pieces against the merkle root of their blocks from the piece layers (both known on the disk
/// thread), hybrids against both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HashVerdict {
    Passed,
    Failed,
    /// A hybrid piece that passed one of its hashes and failed the other (libtorrent
    /// `torrent_inconsistent_hashes`).
    Inconsistent,
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
    /// Every block of the piece was written and the piece hashed.
    Hashed {
        storage: u64,
        piece: u32,
        verdict: HashVerdict,
    },
    Read {
        storage: u64,
        cookie: u64,
        piece: u32,
        offset: u32,
        result: Result<Vec<u8>, Error>,
    },
    /// Answer to [`DiskIo::async_verify`].
    Verified {
        storage: u64,
        piece: u32,
        verdict: HashVerdict,
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
    /// Answer to [`DiskIo::async_move_storage`]: the storage's save path is now `save_path`
    /// (unchanged on error).
    Moved {
        storage: u64,
        status: MoveStatus,
        save_path: String,
        error: Option<MoveError>,
    },
    /// Answer to [`DiskIo::async_rename_file`]: file `file` is stored as `name` unless there
    /// is an error (the path it is about, and the error).
    Renamed {
        storage: u64,
        file: u32,
        name: String,
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
    read_cache: Arc<read_cache::Limits>,
    stats: Arc<DiskStats>,
    next_storage_id: u64,
    next_barrier: u64,
}

/// Counters of reads for peers (libtorrent `disk.num_blocks_read`, `disk.num_read_ops`,
/// `disk.num_blocks_cache_hits`), and the read cache's size per thread.
struct DiskStats {
    blocks_read: AtomicU64,
    read_ops: AtomicU64,
    cache_hits: AtomicU64,
    cache_bytes: Vec<AtomicUsize>,
}

impl DiskStats {
    fn new(threads: usize) -> DiskStats {
        DiskStats {
            blocks_read: AtomicU64::new(0),
            read_ops: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_bytes: (0..threads).map(|_| AtomicUsize::new(0)).collect(),
        }
    }
}

/// A snapshot of the disk counters, see [`DiskIo::counters`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiskCounters {
    /// Requests from peers read, from the cache or the files.
    pub blocks_read: u64,
    /// File reads made for them (a read cache line counts once).
    pub read_ops: u64,
    /// Requests answered from the read cache.
    pub cache_hits: u64,
    /// Bytes in the read cache.
    pub cache_bytes: u64,
}

struct Ctx {
    pool: Arc<FilePool>,
    done: mpsc::Sender<Completion>,
    wake: Arc<dyn Fn() + Send + Sync>,
    queued_write_bytes: Arc<AtomicU64>,
    read_cache: Arc<read_cache::Limits>,
    stats: Arc<DiskStats>,
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
        let n = threads.clamp(1, 64);
        let read_cache = Arc::new(read_cache::Limits::default());
        let stats = Arc::new(DiskStats::new(n + 1));
        let ctx = |index| Ctx {
            pool: pool.clone(),
            done: done.clone(),
            wake: wake.clone(),
            queued_write_bytes: queued_write_bytes.clone(),
            read_cache: read_cache.clone(),
            stats: stats.clone(),
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
            read_cache,
            stats,
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

    /// Sizes the read cache (libtorrent 1.2 `use_read_cache`, `cache_size` and
    /// `read_cache_line_size` in 16 KiB blocks, `cache_expiry` in seconds). `cache_blocks` is
    /// shared by the I/O threads; a negative one is the default, 2048 (32 MiB). Off when
    /// `enabled` is false or a thread's share is less than one line.
    pub fn set_read_cache(&self, enabled: bool, cache_blocks: i32, line_blocks: i32, expiry_secs: u32) {
        let blocks = if cache_blocks < 0 { 2048 } else { cache_blocks as usize };
        let per_thread = if enabled { blocks * BLOCK_SIZE as usize / self.threads.len() } else { 0 };
        let line = (line_blocks.max(0) as u32).saturating_mul(BLOCK_SIZE);
        self.read_cache.set(per_thread, line, Duration::from_secs(expiry_secs.into()));
    }

    pub fn counters(&self) -> DiskCounters {
        let s = &self.stats;
        DiskCounters {
            blocks_read: s.blocks_read.load(Ordering::Relaxed),
            read_ops: s.read_ops.load(Ordering::Relaxed),
            cache_hits: s.cache_hits.load(Ordering::Relaxed),
            cache_bytes: s.cache_bytes.iter().map(|b| b.load(Ordering::Relaxed) as u64).sum(),
        }
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

    /// Hashes `piece` as it is on disk; answered by [`Completion::Verified`]. Seed mode trusts
    /// a torrent's pieces until each is checked like this before its first upload (SEC-20).
    pub fn async_verify(&self, storage: &Arc<Storage>, piece: u32) {
        let _ = self.thread_for(piece).send(Job::Verify { storage: storage.clone(), piece });
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
        self.barrier(storage, BarrierAction::FilePriority(priorities));
    }

    /// Moves the storage's files to `path` after every job queued so far for `storage`, and
    /// before any queued later (libtorrent `async_move_storage`, a fence job). Answered by
    /// [`Completion::Moved`].
    pub fn async_move_storage(&mut self, storage: &Arc<Storage>, path: String, flags: MoveFlags) {
        self.barrier(storage, BarrierAction::Move { path, flags });
    }

    /// Renames file `file` to `name` (relative to the save path, or absolute) after every job
    /// queued so far for `storage`, and before any queued later (libtorrent `async_rename_file`,
    /// a fence job). Answered by [`Completion::Renamed`].
    pub fn async_rename_file(&mut self, storage: &Arc<Storage>, file: u32, name: String) {
        self.barrier(storage, BarrierAction::Rename { file, name });
    }

    fn barrier(&mut self, storage: &Arc<Storage>, action: BarrierAction) {
        let seq = self.next_barrier;
        self.next_barrier += 1;
        let barrier = Arc::new(Barrier { remaining: AtomicUsize::new(self.threads.len() + 1), seq, action });
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

/// Hashes one piece as its torrent needs it, fed whole blocks in order: SHA-1 over the piece,
/// pad bytes included (v1), and SHA-256 over each 16 KiB block of the file data in it (v2,
/// when the piece's v2 hash is known).
struct PieceHasher {
    sha1: Option<Sha1>,
    /// The block hashes so far, and the piece's v2 size (it ends at the end of its file).
    v2: Option<(Vec<Sha256Hash>, u32)>,
    fed: u32,
}

impl PieceHasher {
    fn new(info: &TorrentInfo, piece: u32) -> PieceHasher {
        let sha1 = info.v1().then(Sha1::new);
        let v2 = info.v2_piece_hash(piece).map(|_| (Vec::new(), info.orig_files().piece_size2(piece)));
        PieceHasher { sha1, v2, fed: 0 }
    }

    fn update(&mut self, block: &[u8]) {
        if let Some(h) = self.sha1.as_mut() {
            h.update(block);
        }
        if let Some((blocks, size2)) = self.v2.as_mut() {
            if self.fed < *size2 {
                let n = (block.len() as u32).min(*size2 - self.fed) as usize;
                blocks.push(Sha256::digest(&block[..n]));
            }
        }
        self.fed += block.len() as u32;
    }

    /// libtorrent `on_piece_verified` (downloads) and `on_piece_hashed` (`checking`: a v1
    /// failure is final there, the v2 hash is not looked at).
    fn verdict(self, info: &TorrentInfo, piece: u32, checking: bool) -> HashVerdict {
        let v1 = self.sha1.map(|h| Some(h.finalize()) == info.hash_for_piece(piece));
        if checking && v1 == Some(false) {
            return HashVerdict::Failed;
        }
        let v2 = self.v2.and_then(|(blocks, _)| {
            let (expected, leafs) = info.v2_piece_hash(piece)?;
            Some(
                !blocks.is_empty()
                    && blocks.len() <= leafs
                    && merkle_piece_root(&blocks, leafs, &mut Vec::new()) == expected,
            )
        });
        match (v1, v2) {
            (Some(a), Some(b)) if a != b => HashVerdict::Inconsistent,
            (Some(true), _) | (_, Some(true)) => HashVerdict::Passed,
            _ => HashVerdict::Failed,
        }
    }
}

/// Incremental hash of one piece being downloaded.
struct PartialHash {
    hasher: PieceHasher,
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
    fn new(info: &TorrentInfo, piece: u32, num_blocks: u32) -> PartialHash {
        // blocks wholly in pad files are never downloaded (the piece picker finishes them at
        // once); they hash as the zeros they read as
        let pad_blocks = info.files().pad_bytes_in_piece(piece) / BLOCK_SIZE;
        let payload = num_blocks.saturating_sub(pad_blocks) as usize;
        let mut received = vec![false; num_blocks as usize];
        received[payload..].fill(true);
        PartialHash {
            hasher: PieceHasher::new(info, piece),
            cursor: 0,
            received,
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
    cache: ReadCache,
}

fn disk_thread(rx: mpsc::Receiver<Job>, ctx: Ctx) {
    let mut w = Worker::default();
    loop {
        // Wake up for the read cache's expiry while it holds lines (an expiry of 0 is never).
        let expiry = ctx.read_cache.expiry();
        let check = if expiry.is_zero() { None } else { w.cache.next_check(expiry) };
        let job = match check {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(job) => Some(job),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(job) => Some(job),
                Err(_) => break,
            },
        };
        match job {
            Some(Job::Shutdown) => break,
            Some(job) => w.handle(job, &ctx),
            None => w.cache.expire(Instant::now(), expiry),
        }
        if let Some(b) = ctx.stats.cache_bytes.get(ctx.index) {
            b.store(w.cache.bytes(), Ordering::Relaxed);
        }
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
                self.cache.drop_piece(storage.id, piece);
                let length = data.len() as u32;
                let result = write_block(&storage, ctx, partial, cached_bytes, piece, offset, data);
                ctx.queued_write_bytes.fetch_sub(length as u64, Ordering::Relaxed);
                match result {
                    Ok(hashed) => {
                        ctx.send(Completion::Write { storage: storage.id, piece, offset, error: None });
                        if let Some(verdict) = hashed {
                            ctx.send(Completion::Hashed { storage: storage.id, piece, verdict });
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
                let state = partial
                    .entry((storage.id, piece))
                    .or_insert_with(|| PartialHash::new(&storage.info, piece, n));
                for (b, &on_disk) in blocks.iter().enumerate().take(n as usize) {
                    if on_disk && !state.received[b] {
                        state.received[b] = true;
                        state.restored[b] = true;
                    }
                }
                match advance_hash(&storage, ctx, partial, cached_bytes, piece) {
                    Ok(Some(verdict)) => ctx.send(Completion::Hashed { storage: storage.id, piece, verdict }),
                    Ok(None) => {}
                    Err(_) => {
                        // Only possible if writes were queued before the restore: fail the piece.
                        drop_partial(partial, cached_bytes, storage.id, piece);
                        let verdict = HashVerdict::Failed;
                        ctx.send(Completion::Hashed { storage: storage.id, piece, verdict });
                    }
                }
            }
            Job::Read { storage, piece, offset, length, cookie } => {
                let result = read_for_peer(&mut self.cache, &storage, ctx, piece, offset, length);
                ctx.send(Completion::Read { storage: storage.id, cookie, piece, offset, result });
            }
            Job::Verify { storage, piece } => {
                let mut buf = vec![0u8; storage.info.piece_size(piece) as usize];
                let verdict = match read_into(&storage, &ctx.pool, piece, 0, &mut buf) {
                    Ok(_) => {
                        let mut hasher = PieceHasher::new(&storage.info, piece);
                        for block in buf.chunks(BLOCK_SIZE as usize) {
                            hasher.update(block);
                        }
                        hasher.verdict(&storage.info, piece, true)
                    }
                    Err(_) => HashVerdict::Failed,
                };
                ctx.send(Completion::Verified { storage: storage.id, piece, verdict });
            }
            Job::Check { storage, mode } => {
                if !storage.abort.load(Ordering::Relaxed) {
                    let result = check_files(&storage, ctx, mode);
                    ctx.send(Completion::Checked { storage: storage.id, result });
                }
            }
            Job::Fence { storage, fence } => {
                let id = storage.id;
                self.cache.drop_storage(id);
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
                // A move, rename or priority change may put the storage's blocks elsewhere.
                self.cache.drop_storage(storage.id);
                if barrier.remaining.fetch_sub(1, Ordering::AcqRel) != 1 {
                    self.blocked.insert(storage.id, barrier.seq);
                    return;
                }
                // Every thread has passed it: nothing else of this storage is running.
                match &barrier.action {
                    BarrierAction::FilePriority(priorities) => {
                        let error = set_file_priority(&storage, ctx, priorities).err();
                        ctx.send(Completion::FilePriority { storage: storage.id, error });
                    }
                    BarrierAction::Move { path, flags } => {
                        let (status, save_path, error) = move_storage(&storage, ctx, path, *flags);
                        ctx.send(Completion::Moved { storage: storage.id, status, save_path, error });
                    }
                    BarrierAction::Rename { file, name } => {
                        let error = rename_file(&storage, ctx, *file as usize, name).err();
                        let (file, name) = (*file, name.clone());
                        ctx.send(Completion::Renamed { storage: storage.id, file, name, error });
                    }
                }
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
    (storage.file_path(file).to_string_lossy().into_owned(), Error::from(e))
}

/// Writes one block and advances the piece's hash. Returns the verdict once the whole piece
/// has been hashed.
fn write_block(
    storage: &Storage,
    ctx: &Ctx,
    partial: &mut HashMap<(u64, u32), PartialHash>,
    cached_bytes: &mut usize,
    piece: u32,
    offset: u32,
    data: Vec<u8>,
) -> Result<Option<HashVerdict>, (String, Error)> {
    let fs = storage.info.files();
    let piece_size = storage.info.piece_size(piece);
    let num_blocks = piece_size.div_ceil(BLOCK_SIZE);
    let block = offset / BLOCK_SIZE;
    let state = partial
        .entry((storage.id, piece))
        .or_insert_with(|| PartialHash::new(&storage.info, piece, num_blocks));
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
/// cached. Returns the verdict once the whole piece has been hashed; the state is dropped then.
fn advance_hash(
    storage: &Storage,
    ctx: &Ctx,
    partial: &mut HashMap<(u64, u32), PartialHash>,
    cached_bytes: &mut usize,
    piece: u32,
) -> Result<Option<HashVerdict>, (String, Error)> {
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
                        return Ok(Some(HashVerdict::Failed));
                    }
                    Err(e) => {
                        let file = storage.file_path(fs.file_index_at_piece(piece));
                        return Err((file.to_string_lossy().into_owned(), e));
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
    Ok(Some(state.hasher.verdict(&storage.info, piece, false)))
}

/// Reads a block a peer asked for: from the read cache, else the cache line holding it, read
/// with one call and kept, else (cache off, or the line could not be read) just the block.
fn read_for_peer(
    cache: &mut ReadCache,
    storage: &Storage,
    ctx: &Ctx,
    piece: u32,
    offset: u32,
    length: u32,
) -> Result<Vec<u8>, Error> {
    let stats = &ctx.stats;
    stats.blocks_read.fetch_add(1, Ordering::Relaxed);
    let Some((limit, line)) = ctx.read_cache.get() else {
        cache.trim(0);
        stats.read_ops.fetch_add(1, Ordering::Relaxed);
        return read_block(storage, &ctx.pool, piece, offset, length);
    };
    cache.trim(limit);
    let piece_size = storage.info.piece_size(piece);
    let start = offset / line * line;
    let end = start.saturating_add(line).min(piece_size);
    if offset.saturating_add(length) > end || length == 0 {
        stats.read_ops.fetch_add(1, Ordering::Relaxed);
        return read_block(storage, &ctx.pool, piece, offset, length);
    }
    let key = (storage.id, piece, start);
    let now = Instant::now();
    if let Some(data) = cache.read(key, offset, length, now) {
        stats.cache_hits.fetch_add(1, Ordering::Relaxed);
        return Ok(data);
    }
    stats.read_ops.fetch_add(1, Ordering::Relaxed);
    let mut buf = vec![0u8; (end - start) as usize];
    if read_into(storage, &ctx.pool, piece, start, &mut buf).is_err() {
        stats.read_ops.fetch_add(1, Ordering::Relaxed);
        return read_block(storage, &ctx.pool, piece, offset, length);
    }
    let at = (offset - start) as usize;
    let out = buf[at..at + length as usize].to_vec();
    cache.insert(key, buf, offset, limit, now);
    Ok(out)
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
            return Ok(CheckResult { have, resume_failed: false, inconsistent: None });
        }
        resume_failed = true;
    }

    let mut have = Bitfield::new(num_pieces as usize, false);
    // Nothing on disk: nothing to hash.
    let any_file = storage.lock_files().part.as_ref().is_some_and(|p| !p.is_empty())
        || (0..fs.num_files())
            .any(|f| fs.file_size(f) > 0 && !storage.skip_file(f) && storage.file_path(f).exists());
    if !any_file {
        return Ok(CheckResult { have, resume_failed, inconsistent: None });
    }
    let mut buf = vec![0u8; storage.info.piece_length() as usize];
    let report_every = (num_pieces / 100).max(16);
    let mut inconsistent = None;
    for piece in 0..num_pieces {
        if storage.abort.load(Ordering::Relaxed) {
            break;
        }
        let size = storage.info.piece_size(piece) as usize;
        if read_into(storage, &ctx.pool, piece, 0, &mut buf[..size]).is_ok() {
            let mut hasher = PieceHasher::new(&storage.info, piece);
            for block in buf[..size].chunks(BLOCK_SIZE as usize) {
                hasher.update(block);
            }
            match hasher.verdict(&storage.info, piece, true) {
                HashVerdict::Passed => have.set(piece as usize, true),
                HashVerdict::Failed => {}
                HashVerdict::Inconsistent => {
                    inconsistent = Some(piece);
                    break;
                }
            }
        }
        if (piece + 1) % report_every == 0 {
            ctx.send(Completion::CheckProgress { storage: storage.id, checked: piece + 1 });
        }
    }
    // Checking may open hundreds of files; don't keep them all.
    ctx.pool.release(storage.id);
    Ok(CheckResult { have, resume_failed, inconsistent })
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
    let save = PathBuf::from(storage.save_path());
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

/// libtorrent `complete`: `path` made absolute against the working directory.
pub(crate) fn complete(path: &str) -> String {
    let p = if path.is_empty() { "." } else { path };
    std::path::absolute(p).map_or_else(|_| p.to_owned(), |a| a.to_string_lossy().into_owned())
}

/// libtorrent `move_file`: creates the destination's directory, then renames.
fn move_file(old: &Path, new: &Path) -> Result<(), (FileOperation, io::Error)> {
    std::fs::metadata(old).map_err(|e| (FileOperation::FileStat, e))?;
    if let Some(parent) = new.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| (FileOperation::Mkdir, e))?;
    }
    if old == new {
        return Ok(());
    }
    std::fs::rename(old, new).map_err(|e| (FileOperation::FileRename, e))
}

fn is_not_found<T>(r: io::Result<T>) -> bool {
    matches!(r, Err(e) if e.kind() == io::ErrorKind::NotFound)
}

/// libtorrent `posix_storage::rename_file`: renames the file if it is on disk (making the
/// directories the new name lands in), then uses the new name for every later job. A file not
/// downloaded yet only changes its name. A file on disk is never renamed over an existing one:
/// that fails with `AlreadyExists` (libtorrent `file_exists`). Runs with no other job of
/// `storage` in flight.
fn rename_file(storage: &Storage, ctx: &Ctx, file: usize, name: &str) -> Result<(), (String, Error)> {
    let fs = storage.info.files();
    if file >= fs.num_files() {
        let e = io::Error::new(io::ErrorKind::InvalidInput, "no such file in the torrent");
        return Err((String::new(), e.into()));
    }
    let old = storage.file_path(file);
    let fail = |path: &Path, e: io::Error| Err((path.to_string_lossy().into_owned(), Error::from(e)));
    match old.try_exists() {
        Ok(true) => {
            let new = if crate::util::path::is_complete(name) {
                PathBuf::from(name)
            } else {
                PathBuf::from(storage.save_path()).join(name)
            };
            // An existing file is never overwritten (libtorrent 2.0.12), not even by itself.
            if matches!(new.try_exists(), Ok(true)) {
                let e = io::Error::new(io::ErrorKind::AlreadyExists, "file already exists");
                return fail(&new, e);
            }
            if let Some(dir) = new.parent().filter(|d| !d.as_os_str().is_empty()) {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    return fail(dir, e);
                }
            }
            // Open files cannot be renamed on Windows.
            ctx.pool.release(storage.id);
            match file_pool::rename_no_replace(&old, &new) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return fail(&new, e),
                Err(e) => return fail(&old, e),
            }
        }
        Ok(false) => {}
        Err(e) => return fail(&old, e),
    }
    let mut mapped = storage.mapped.write().unwrap_or_else(|e| e.into_inner());
    let files = mapped.get_or_insert_with(|| Arc::new(fs.clone()));
    Arc::make_mut(files).rename_file(file, name);
    Ok(())
}

/// libtorrent `posix_storage::move_storage` and `aux::move_storage`: moves every file (rename,
/// or copy and delete across volumes) and the part file to `dest`, rolls the renames back on
/// an error, then removes the directories left empty. Runs with no other job of `storage` in
/// flight. Returns the status, the save path now in use and the error, if any.
fn move_storage(
    storage: &Storage,
    ctx: &Ctx,
    dest: &str,
    flags: MoveFlags,
) -> (MoveStatus, String, Option<MoveError>) {
    // Open files cannot be renamed on Windows (libtorrent `mmap_storage` releases them too).
    ctx.pool.release(storage.id);
    let mapped = storage.mapped();
    let fs = mapped.as_deref().unwrap_or(storage.info.files());
    let save_path = storage.save_path();
    let new_save_path = complete(dest);
    let fail = |status, file: String, operation, e: io::Error| {
        (status, save_path.clone(), Some(MoveError { file, operation, error: e.into() }))
    };
    let movable = |i: &usize| !fs.file_absolute_path(*i) && !fs.pad_file_at(*i);

    if flags == MoveFlags::FailIfExist && !is_not_found(std::fs::metadata(&new_save_path)) {
        // The directory exists: so may the files.
        for i in (0..fs.num_files()).filter(movable) {
            let path = fs.file_path(i, &new_save_path);
            match std::fs::metadata(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return fail(MoveStatus::FileExist, path, FileOperation::FileStat, e),
                Ok(_) => {
                    let e = io::Error::new(io::ErrorKind::AlreadyExists, "file already exists");
                    return fail(MoveStatus::FileExist, path, FileOperation::FileStat, e);
                }
            }
        }
    }

    match std::fs::metadata(&new_save_path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Err(e) = std::fs::create_dir_all(&new_save_path) {
                return fail(MoveStatus::FatalDiskError, new_save_path.clone(), FileOperation::Mkdir, e);
            }
        }
        Err(e) => {
            return fail(MoveStatus::FatalDiskError, new_save_path.clone(), FileOperation::FileStat, e);
        }
    }

    let set_save_path =
        |p: &str| *storage.save_path.write().unwrap_or_else(|e| e.into_inner()) = p.to_owned();
    match flags {
        MoveFlags::ResetSavePath => {
            set_save_path(&new_save_path);
            return (MoveStatus::NeedFullCheck, new_save_path, None);
        }
        MoveFlags::ResetSavePathUnchecked => {
            set_save_path(&new_save_path);
            return (MoveStatus::NoError, new_save_path, None);
        }
        _ => {}
    }

    let mut status = MoveStatus::NoError;
    // Files we renamed (rolled back on an error) and files we copied (their sources are deleted
    // once everything is across).
    let mut renamed = vec![false; fs.num_files()];
    let mut copied = vec![false; fs.num_files()];
    let mut error = None;
    for i in (0..fs.num_files()).filter(movable) {
        let old = PathBuf::from(fs.file_path(i, &save_path));
        let new = PathBuf::from(fs.file_path(i, &new_save_path));
        if flags == MoveFlags::DontReplace && new.exists() {
            status = MoveStatus::NeedFullCheck;
            continue;
        }
        match move_file(&old, &new) {
            Ok(()) => renamed[i] = old != new,
            // A file that is not there (not downloaded yet) is not a problem.
            Err((_, e)) if e.kind() == io::ErrorKind::NotFound => {}
            Err((op, e))
                if matches!(e.kind(), io::ErrorKind::InvalidInput | io::ErrorKind::PermissionDenied) =>
            {
                error = Some((i, op, e));
                break;
            }
            // Renaming across volumes fails (EXDEV, ERROR_NOT_SAME_DEVICE): copy instead.
            Err(_) => match std::fs::copy(&old, &new) {
                Ok(_) => copied[i] = true,
                Err(e) => {
                    error = Some((i, FileOperation::FileCopy, e));
                    break;
                }
            },
        }
    }
    let mut error = error.map(|(i, operation, e)| MoveError {
        file: fs.file_path(i, &save_path),
        operation,
        error: e.into(),
    });

    if error.is_none() {
        let mut st = storage.lock_files();
        if st.part.is_none() && storage.part_file_path().exists() {
            storage.need_partfile(&mut st);
        }
        if let Some(part) = st.part.clone() {
            let old = storage.part_file_path();
            let new_dir = Path::new(&new_save_path);
            match part.move_partfile(new_dir) {
                Ok(()) => storage.part_files.rekey(&old, &new_dir.join(storage.part_file_name())),
                Err(e) => {
                    error = Some(MoveError {
                        file: old.to_string_lossy().into_owned(),
                        operation: FileOperation::PartfileMove,
                        error: e.into(),
                    })
                }
            }
        }
    }

    if let Some(error) = error {
        // Roll back. libtorrent moves back every file before the failing one; only the ones
        // renamed are moved here, so a destination file kept by `DontReplace` stays put.
        for i in (0..fs.num_files()).rev().filter(|&i| renamed[i]) {
            let new = PathBuf::from(fs.file_path(i, &new_save_path));
            let _ = move_file(&new, Path::new(&fs.file_path(i, &save_path)));
        }
        return (MoveStatus::FatalDiskError, save_path, Some(error));
    }
    set_save_path(&new_save_path);

    // Delete the sources of copied files, then the directories left empty (deepest first along
    // each file's path, stopping at the first that is not empty, never the save path).
    let mut subdirs = std::collections::BTreeSet::new();
    for i in (0..fs.num_files()).filter(movable) {
        let rel = PathBuf::from(fs.file_path(i, ""));
        if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            subdirs.insert(parent.to_path_buf());
        }
        if copied[i] {
            let _ = std::fs::remove_file(fs.file_path(i, &save_path));
        }
    }
    let root = Path::new(&save_path);
    for sub in subdirs {
        let mut dir = root.join(sub);
        while dir != root && std::fs::remove_dir(&dir).is_ok() {
            match dir.parent() {
                Some(p) => dir = p.to_path_buf(),
                None => break,
            }
        }
    }
    (status, new_save_path, None)
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
    fn reads_for_peers_go_through_the_read_cache() {
        // 3 pieces of 32 KiB, piece 1 across the boundary of files "one" and "two"
        let r = rig("read_cache", &[("one", 40_000), ("two", 40_000)], 32 * 1024);
        let all = torrent_bytes(&r.contents);
        let block = |p: usize, b: usize| all[p * 32768 + b * 16384..][..16384].to_vec();
        for p in 0..2u32 {
            r.disk.async_write(&r.storage, p, 0, block(p as usize, 0));
            r.disk.async_write(&r.storage, p, BLOCK_SIZE, block(p as usize, 1));
        }
        let mut hashed = 0;
        while hashed < 2 {
            if let Completion::Hashed { verdict, .. } = next(&r.rx) {
                assert_eq!(verdict, HashVerdict::Passed);
                hashed += 1;
            }
        }
        let read = |piece: u32, offset: u32| {
            r.disk.async_read(&r.storage, piece, offset, BLOCK_SIZE, 1);
            loop {
                if let Completion::Read { result, .. } = next(&r.rx) {
                    break result.unwrap();
                }
            }
        };
        let counters = |blocks_read, read_ops, cache_hits| DiskCounters {
            blocks_read,
            read_ops,
            cache_hits,
            cache_bytes: r.disk.counters().cache_bytes,
        };

        // Lines of two blocks: the whole piece is read at the first request.
        r.disk.set_read_cache(true, 2048, 2, 300);
        assert_eq!(read(1, BLOCK_SIZE), block(1, 1));
        assert_eq!(read(1, 0), block(1, 0));
        assert_eq!(read(0, 0), block(0, 0));
        assert_eq!(r.disk.counters(), counters(3, 2, 1));
        let deadline = Instant::now() + Duration::from_secs(5);
        while r.disk.counters().cache_bytes != 2 * 32768 {
            assert!(Instant::now() < deadline, "the threads report their lines");
            std::thread::sleep(Duration::from_millis(1));
        }

        // A write to the piece clears its line: the next read sees the new data.
        let mut changed = block(1, 0);
        changed[100] ^= 0xff;
        r.disk.async_write(&r.storage, 1, 0, changed.clone());
        assert!(matches!(next(&r.rx), Completion::Write { error: None, .. }));
        assert_eq!(read(1, 0), changed);
        assert_eq!(r.disk.counters(), counters(4, 3, 1));
        assert_eq!(read(1, BLOCK_SIZE), block(1, 1));
        assert_eq!(r.disk.counters(), counters(5, 3, 2));

        // A fence clears the storage's lines.
        r.disk.async_fence(&r.storage, FenceAction::Release);
        loop {
            if let Completion::Fenced { .. } = next(&r.rx) {
                break;
            }
        }
        assert_eq!(read(0, BLOCK_SIZE), block(0, 1));
        assert_eq!(r.disk.counters(), counters(6, 4, 2));

        // Off: every request is a read of its own.
        r.disk.set_read_cache(false, 2048, 2, 300);
        assert_eq!(read(0, 0), block(0, 0));
        assert_eq!(read(0, 0), block(0, 0));
        assert_eq!(r.disk.counters(), counters(8, 6, 2));
        let _ = std::fs::remove_dir_all(&r.dir);
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
                Completion::Hashed { piece, verdict, .. } => {
                    let passed = verdict == HashVerdict::Passed;
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
            if let Completion::Hashed { piece: 0, verdict, .. } = next(&r.rx) {
                let passed = verdict == HashVerdict::Passed;
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
            Arc::new(Storage::new(99, r.storage.save_path(), info.clone(), Vec::new(), Arc::default()));
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
            if let Completion::Hashed { piece, verdict, .. } = next(rx) {
                let passed = verdict == HashVerdict::Passed;
                break (piece, passed);
            }
        };
        // A previous session wrote blocks 0 and 2 of piece 0, then stopped.
        let old = Arc::new(Storage::new(
            50,
            r.storage.save_path(),
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
            r.storage.save_path(),
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
                Completion::Hashed { piece, verdict, .. } => {
                    let passed = verdict == HashVerdict::Passed;
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
        let save = r.storage.save_path();
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
        let gone = r.disk.new_storage(r.storage.save_path(), info.clone(), Vec::new());
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
        let gone = r.disk.new_storage(r.storage.save_path(), info, Vec::new());
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
            read_cache: Arc::default(),
            stats: Arc::new(DiskStats::new(1)),
            queues: Arc::new(OnceLock::new()),
            index: 0,
        };
        let mut w = Worker::default();
        let s = r.storage.clone();
        let other = Arc::new(Storage::new(77, s.save_path(), s.info.clone(), Vec::new(), Arc::default()));
        let barrier = |seq| {
            Arc::new(Barrier {
                remaining: AtomicUsize::new(2),
                seq,
                action: BarrierAction::FilePriority(vec![4]),
            })
        };
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
        assert!(matches!(
            rx.try_recv(),
            Ok(Completion::Hashed { piece: 0, verdict: HashVerdict::Passed, .. })
        ));
        assert!(rx.try_recv().is_err());
        w.handle(Job::Unblock { storage: s.id, seq: 2 }, &ctx);
        assert!(matches!(rx.try_recv(), Ok(Completion::Write { piece: 1, error: None, .. })));
        assert!(matches!(
            rx.try_recv(),
            Ok(Completion::Hashed { piece: 1, verdict: HashVerdict::Passed, .. })
        ));
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
        let s2 = r.disk.new_storage(r.storage.save_path(), info, vec![4, 4, 4]);
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

    // test_storage.cpp `setup_torrent`: 0x8000, 0x8000, 0, 0, 0x8000 bytes in 16 KiB pieces.
    const MOVE_FILES: &[(&str, usize)] = &[
        ("test1.tmp", 0x8000),
        ("folder1/test2.tmp", 0x8000),
        ("folder2/test3.tmp", 0),
        ("_folder3/test4.tmp", 0),
        ("_folder3/subfolder/test5.tmp", 0x8000),
    ];

    /// A storage in `<dir>/save` with every piece written and the empty files created.
    fn move_rig(name: &str, priorities: Vec<u8>) -> Rig {
        let (bytes, contents) = make_torrent(MOVE_FILES, BLOCK_SIZE);
        let info = Arc::new(TorrentInfo::from_bytes(&bytes).unwrap());
        let dir = temp_dir(name);
        let (tx, rx) = mpsc::channel();
        let mut disk = DiskIo::new(3, 8, tx, Arc::new(|| {}));
        let save = dir.join("save");
        let storage = disk.new_storage(save.to_string_lossy().into_owned(), info, priorities);
        let r = Rig { disk, rx, storage, dir, contents };
        r.disk.async_check(&r.storage, CheckMode::Full);
        checked(&r.rx);
        let storage = r.storage.clone();
        let wanted: Vec<u32> = (0..6).filter(|&p| r.storage.info.piece_size(p) > 0).collect();
        write_pieces(&r, &storage, &wanted);
        r
    }

    fn moved(rx: &mpsc::Receiver<Completion>) -> (MoveStatus, String, Option<MoveError>) {
        loop {
            match next(rx) {
                Completion::Moved { status, save_path, error, .. } => break (status, save_path, error),
                Completion::Write { error: None, .. }
                | Completion::Hashed { verdict: HashVerdict::Passed, .. } => {}
                c => panic!("unexpected {c:?}"),
            }
        }
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn renamed(rx: &mpsc::Receiver<Completion>) -> (u32, String, Option<(String, Error)>) {
        loop {
            match next(rx) {
                Completion::Renamed { file, name, error, .. } => break (file, name, error),
                Completion::Write { error: None, .. }
                | Completion::Hashed { verdict: HashVerdict::Passed, .. } => {}
                c => panic!("unexpected {c:?}"),
            }
        }
    }

    #[test]
    fn rename_file_on_disk_then_read_and_move() {
        // test_torrent.cpp `test_rename_file` / test_storage.cpp's rename cases, on one storage.
        let mut r = move_rig("rename", Vec::new());
        let save = r.dir.join("save");
        // A relative name lands under the save path, directories made; the file keeps its data.
        r.disk.async_rename_file(&r.storage, 1, "test/renamed/two.tmp".into());
        assert_eq!(renamed(&r.rx), (1, "test/renamed/two.tmp".into(), None));
        assert!(!save.join("test/folder1/test2.tmp").exists());
        assert_eq!(std::fs::read(save.join("test/renamed/two.tmp")).unwrap(), r.contents[1]);
        // An absolute name, and a name outside the torrent's folder.
        let abs = r.dir.join("elsewhere/one.tmp");
        r.disk.async_rename_file(&r.storage, 0, s(&abs));
        assert!(renamed(&r.rx).2.is_none());
        assert_eq!(std::fs::read(&abs).unwrap(), r.contents[0]);
        r.disk.async_rename_file(&r.storage, 4, "five.tmp".into());
        assert!(renamed(&r.rx).2.is_none());
        assert_eq!(std::fs::read(save.join("five.tmp")).unwrap(), r.contents[4]);

        // Later jobs use the new names: file 1 starts at piece 2.
        r.disk.async_read(&r.storage, 2, 0, BLOCK_SIZE, 1);
        match next(&r.rx) {
            Completion::Read { result: Ok(data), .. } => assert_eq!(data, r.contents[1][..16384]),
            c => panic!("unexpected {c:?}"),
        }
        r.disk.async_check(&r.storage, CheckMode::Full);
        assert!(checked(&r.rx).have.all_set());

        // A move takes the renamed relative files along and leaves the absolute one.
        let new = r.dir.join("save2");
        r.disk.async_move_storage(&r.storage, s(&new), MoveFlags::AlwaysReplaceFiles);
        assert_eq!(moved(&r.rx).0, MoveStatus::NoError);
        assert_eq!(std::fs::read(new.join("test/renamed/two.tmp")).unwrap(), r.contents[1]);
        assert_eq!(std::fs::read(new.join("five.tmp")).unwrap(), r.contents[4]);
        assert_eq!(std::fs::read(&abs).unwrap(), r.contents[0]);
        assert!(!save.join("test/renamed").exists(), "emptied directories are removed");

        // A file that is not on disk only changes its name.
        std::fs::remove_file(new.join("five.tmp")).unwrap();
        r.disk.async_rename_file(&r.storage, 4, "test/later.tmp".into());
        assert!(renamed(&r.rx).2.is_none());
        assert!(!new.join("test/later.tmp").exists());

        // A directory that cannot be made fails, and the name stays.
        std::fs::write(new.join("blocker"), b"x").unwrap();
        r.disk.async_rename_file(&r.storage, 1, "blocker/two.tmp".into());
        let (_, _, error) = renamed(&r.rx);
        let (path, _) = error.expect("an error");
        assert!(path.ends_with("blocker"), "{path}");
        assert_eq!(std::fs::read(new.join("test/renamed/two.tmp")).unwrap(), r.contents[1]);
        r.disk.async_rename_file(&r.storage, 99, "x".into());
        assert!(renamed(&r.rx).2.is_some());

        // An existing file is not replaced (libtorrent 2.0.12), nor is the file itself.
        std::fs::write(new.join("taken.tmp"), b"mine").unwrap();
        for name in ["taken.tmp", "test/renamed/two.tmp"] {
            r.disk.async_rename_file(&r.storage, 1, name.into());
            let (path, error) = renamed(&r.rx).2.expect("file exists");
            assert!(Path::new(&path).ends_with(name), "{path}");
            assert!(matches!(error, Error::Io { kind: io::ErrorKind::AlreadyExists, .. }), "{error:?}");
        }
        assert_eq!(std::fs::read(new.join("taken.tmp")).unwrap(), b"mine");
        assert_eq!(std::fs::read(new.join("test/renamed/two.tmp")).unwrap(), r.contents[1]);
        // The race-free primitive refuses too.
        let (a, b) = (new.join("test/renamed/two.tmp"), new.join("taken.tmp"));
        let e = file_pool::rename_no_replace(&a, &b).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&a).unwrap(), r.contents[1]);
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn move_storage_to_another_directory() {
        // test_storage.cpp `dont_move_intermingled_files`, plus the hash state kept across.
        let mut r = move_rig("move", Vec::new());
        let (old, new) = (r.dir.join("save"), r.dir.join("save2"));
        std::fs::create_dir_all(old.join("test/_folder3/alien_folder1")).unwrap();
        std::fs::write(old.join("test/alien1.tmp"), b"x").unwrap();
        std::fs::write(old.join("test/folder1/alien2.tmp"), b"x").unwrap();

        r.disk.async_move_storage(&r.storage, s(&new), MoveFlags::AlwaysReplaceFiles);
        let (status, path, error) = moved(&r.rx);
        assert_eq!((status, path.as_str()), (MoveStatus::NoError, s(&new).as_str()), "{error:?}");
        assert_eq!(r.storage.save_path(), s(&new));
        for (i, (name, _)) in MOVE_FILES.iter().enumerate() {
            assert_eq!(std::fs::read(new.join("test").join(name)).unwrap(), r.contents[i], "{name}");
            assert!(!old.join("test").join(name).exists(), "{name}");
        }
        // Intermingled files and directories stay; directories left empty go.
        assert!(old.join("test/alien1.tmp").exists());
        assert!(old.join("test/folder1/alien2.tmp").exists());
        assert!(old.join("test/_folder3/alien_folder1").exists());
        assert!(!new.join("test/alien1.tmp").exists());
        assert!(!old.join("test/folder2").exists());
        assert!(!old.join("test/_folder3/subfolder").exists());
        assert!(old.exists(), "the old save path stays");

        // Reads, writes and checks use the new path.
        r.disk.async_read(&r.storage, 0, 0, 16, 3);
        assert!(matches!(next(&r.rx), Completion::Read { cookie: 3, result: Ok(_), .. }));
        r.disk.async_check(&r.storage, CheckMode::Full);
        assert!(checked(&r.rx).have.all_set());
        let storage = r.storage.clone();
        r.disk.async_fence(&storage, FenceAction::Delete);
        assert!(matches!(next(&r.rx), Completion::Fenced { error: None, .. }));
        assert!(!new.join("test").exists());
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn move_storage_to_self() {
        // test_storage.cpp `test_move_storage_to_self`: a no-op.
        let mut r = move_rig("move-self", Vec::new());
        let save = r.storage.save_path();
        r.disk.async_move_storage(&r.storage, save.clone(), MoveFlags::AlwaysReplaceFiles);
        let (status, path, error) = moved(&r.rx);
        assert_eq!(status, MoveStatus::NoError, "{error:?}");
        assert_eq!(path, save);
        for (i, (name, _)) in MOVE_FILES.iter().enumerate() {
            assert_eq!(std::fs::read(Path::new(&save).join("test").join(name)).unwrap(), r.contents[i]);
        }
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn move_storage_into_self() {
        // test_storage.cpp `test_move_storage_into_self`: into a folder of the torrent itself.
        let mut r = move_rig("move-into", Vec::new());
        let inner = r.dir.join("save/test/folder1");
        r.disk.async_move_storage(&r.storage, s(&inner), MoveFlags::AlwaysReplaceFiles);
        let (status, _, error) = moved(&r.rx);
        assert_eq!(status, MoveStatus::NoError, "{error:?}");
        assert_eq!(std::fs::read(inner.join("test/folder1/test2.tmp")).unwrap(), r.contents[1]);
        assert!(inner.join("test/folder2/test3.tmp").exists());
        assert!(inner.join("test/_folder3/test4.tmp").exists());
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn move_storage_reset() {
        // test_storage.cpp `test_move_storage_reset`: the directory is made, nothing moves.
        for (flags, want) in [
            (MoveFlags::ResetSavePath, MoveStatus::NeedFullCheck),
            (MoveFlags::ResetSavePathUnchecked, MoveStatus::NoError),
        ] {
            let mut r = move_rig("move-reset", Vec::new());
            let new = r.dir.join("save2");
            r.disk.async_move_storage(&r.storage, s(&new), flags);
            let (status, path, error) = moved(&r.rx);
            assert_eq!((status, path), (want, s(&new)), "{error:?}");
            assert!(new.is_dir());
            assert!(!new.join("test").exists());
            assert!(r.dir.join("save/test/test1.tmp").exists());
            assert_eq!(r.storage.save_path(), s(&new));
            drop(r.disk);
            let _ = std::fs::remove_dir_all(&r.dir);
        }
    }

    #[test]
    fn fail_if_exist_and_dont_replace() {
        let mut r = move_rig("move-exist", Vec::new());
        let (old, new) = (r.dir.join("save"), r.dir.join("save2"));
        std::fs::create_dir_all(new.join("test/folder1")).unwrap();
        std::fs::write(new.join("test/folder1/test2.tmp"), b"theirs").unwrap();

        // FailIfExist moves nothing.
        r.disk.async_move_storage(&r.storage, s(&new), MoveFlags::FailIfExist);
        let (status, path, error) = moved(&r.rx);
        assert_eq!((status, path), (MoveStatus::FileExist, s(&old)));
        let error = error.unwrap();
        assert_eq!(error.operation, FileOperation::FileStat);
        assert!(error.file.ends_with("test2.tmp"), "{}", error.file);
        assert!(old.join("test/test1.tmp").exists() && !new.join("test/test1.tmp").exists());
        assert_eq!(r.storage.save_path(), s(&old));

        // DontReplace keeps their file and asks for a recheck; ours is left behind.
        r.disk.async_move_storage(&r.storage, s(&new), MoveFlags::DontReplace);
        let (status, path, error) = moved(&r.rx);
        assert_eq!((status, path), (MoveStatus::NeedFullCheck, s(&new)), "{error:?}");
        assert_eq!(std::fs::read(new.join("test/folder1/test2.tmp")).unwrap(), b"theirs");
        assert_eq!(std::fs::read(new.join("test/test1.tmp")).unwrap(), r.contents[0]);
        assert!(old.join("test/folder1/test2.tmp").exists());
        r.disk.async_check(&r.storage, CheckMode::Full);
        let have = checked(&r.rx).have;
        assert_eq!(wanted(&have), [0, 1, 4, 5], "test2.tmp's pieces fail");

        // AlwaysReplaceFiles (the default) overwrites.
        r.disk.async_move_storage(&r.storage, s(&old), MoveFlags::DontReplace);
        moved(&r.rx);
        r.disk.async_move_storage(&r.storage, s(&new), MoveFlags::AlwaysReplaceFiles);
        assert_eq!(moved(&r.rx).0, MoveStatus::NoError);
        assert_eq!(std::fs::read(new.join("test/folder1/test2.tmp")).unwrap(), r.contents[1]);
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn a_failed_move_is_rolled_back() {
        let mut r = move_rig("move-fail", Vec::new());
        let (old, new) = (r.dir.join("save"), r.dir.join("save2"));
        // test5.tmp cannot go where a non-empty directory of that name is.
        std::fs::create_dir_all(new.join("test/_folder3/subfolder/test5.tmp/x")).unwrap();
        r.disk.async_move_storage(&r.storage, s(&new), MoveFlags::AlwaysReplaceFiles);
        let (status, path, error) = moved(&r.rx);
        assert_eq!((status, path), (MoveStatus::FatalDiskError, s(&old)));
        let error = error.unwrap();
        assert!(error.file.ends_with("test5.tmp"), "{}", error.file);
        assert!(matches!(error.operation, FileOperation::FileRename | FileOperation::FileCopy));
        for (i, (name, _)) in MOVE_FILES.iter().enumerate() {
            assert_eq!(std::fs::read(old.join("test").join(name)).unwrap(), r.contents[i], "{name} back");
        }
        assert!(!new.join("test/test1.tmp").exists());
        assert_eq!(r.storage.save_path(), s(&old));
        r.disk.async_check(&r.storage, CheckMode::Full);
        assert!(checked(&r.rx).have.all_set());
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn the_part_file_moves_with_the_files() {
        // test2.tmp (pieces 2..=3) skipped: its pieces stay in the part file.
        let (bytes, contents) = make_torrent(MOVE_FILES, BLOCK_SIZE);
        let info = Arc::new(TorrentInfo::from_bytes(&bytes).unwrap());
        let dir = temp_dir("move-part");
        let (tx, rx) = mpsc::channel();
        let mut disk = DiskIo::new(3, 8, tx, Arc::new(|| {}));
        let (old, new) = (dir.join("save"), dir.join("save2"));
        let prio = vec![4, 0, 4, 4, 4];
        let storage = disk.new_storage(s(&old), info.clone(), prio.clone());
        let mut r = Rig { disk, rx, storage: storage.clone(), dir, contents };
        r.disk.async_check(&storage, CheckMode::Full);
        checked(&r.rx);
        write_pieces(&r, &storage, &[0, 1, 2, 3, 4, 5]);
        let part = format!(".{}.parts", info.info_hash().to_hex());
        assert!(old.join(&part).exists() && !old.join("test/folder1/test2.tmp").exists());

        // Writes queued behind the move go to the new place.
        r.disk.async_move_storage(&storage, s(&new), MoveFlags::AlwaysReplaceFiles);
        let all = torrent_bytes(&r.contents);
        r.disk.async_write(&storage, 0, 0, all[..BLOCK_SIZE as usize].to_vec());
        let (status, _, error) = moved(&r.rx);
        assert_eq!(status, MoveStatus::NoError, "{error:?}");
        assert!(!old.join(&part).exists());
        assert!(new.join(&part).exists());
        assert!(!old.join("test").exists(), "every directory left empty is gone");
        assert!(r.storage.part_files.get(&new.join(&part)).is_some(), "the registry follows");
        assert!(r.storage.part_files.get(&old.join(&part)).is_none());
        loop {
            if let Completion::Hashed { piece: 0, verdict, .. } = next(&r.rx) {
                let passed = verdict == HashVerdict::Passed;
                assert!(passed);
                break;
            }
        }
        r.disk.async_check(&storage, CheckMode::Full);
        assert!(checked(&r.rx).have.all_set(), "the boundary pieces are read from the part file");

        // Unskipping exports into the file at the new place.
        r.disk.async_set_file_priority(&storage, vec![4; 5]);
        assert!(matches!(next(&r.rx), Completion::FilePriority { error: None, .. }));
        assert_eq!(std::fs::read(new.join("test/folder1/test2.tmp")).unwrap()[..16], r.contents[1][..16]);
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn a_stopped_torrents_part_file_moves_too() {
        // No part file object is open (a fresh storage): the file on disk is moved anyway.
        let mut r = move_rig("move-part-stopped", vec![4, 0, 4, 4, 4]);
        let info = r.storage.info.clone();
        let part = format!(".{}.parts", info.info_hash().to_hex());
        let (old, new) = (r.dir.join("save"), r.dir.join("save2"));
        let storage = r.storage.clone();
        r.disk.async_fence(&storage, FenceAction::Release);
        while !matches!(next(&r.rx), Completion::Fenced { .. }) {}
        drop(storage);
        r.storage = r.disk.new_storage(s(&old), info, vec![4, 0, 4, 4, 4]);
        assert!(old.join(&part).exists());
        r.disk.async_move_storage(&r.storage, s(&new), MoveFlags::AlwaysReplaceFiles);
        assert_eq!(moved(&r.rx).0, MoveStatus::NoError);
        assert!(new.join(&part).exists() && !old.join(&part).exists());
        r.disk.async_check(&r.storage, CheckMode::Full);
        assert!(checked(&r.rx).have.all_set());
        drop(r.disk);
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}
