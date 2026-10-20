// Not in libtorrent 2.0.11, which has no read cache: its mmap backend leaves reads to the page
// cache, and its pread backend reads every requested block on its own. Modelled on the read
// path of libtorrent 1.2's `block_cache` (`disk_io_thread::do_read`, `block_cache::try_read`):
// a miss reads `read_cache_line_size` blocks, capped by the piece, the line holding the
// request, and later requests in the line are copied from memory. Settings names and defaults
// are 1.2's (`use_read_cache`, `cache_size`, `read_cache_line_size`, `cache_expiry`).
// Changes: no write cache (writes go straight to the files, PLAN.md D3), so a line is only
// ever clean; each disk thread keeps its own lines (jobs for a piece always run on one thread),
// so nothing is locked; a line whose blocks have all been sent once is evicted before any line
// still being read, which keeps the lines peers are part way through when many pieces are read
// at once; `cache_expiry` is how long an unused line stays, 0 for as long as there is room
// (1.2: how long a dirty block may wait in the write cache); `cache_size` -1 is 2048 blocks
// (1.2: sized by the machine's memory).

//! The disk threads' read cache: whole lines of a piece read with one call, so seeding many
//! pieces at once costs one disk seek per line instead of one per 16 KiB block.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// (storage id, piece, offset of the line in the piece).
type Key = (u64, u32, u32);

/// How big the cache may be, shared by the disk threads and changed by the network thread.
#[derive(Default)]
pub(crate) struct Limits {
    /// Bytes each disk thread may keep; 0 turns the cache off.
    bytes_per_thread: AtomicUsize,
    line_bytes: AtomicU32,
    expiry_ms: AtomicU64,
}

impl Limits {
    pub fn set(&self, bytes_per_thread: usize, line_bytes: u32, expiry: Duration) {
        self.bytes_per_thread.store(bytes_per_thread, Ordering::Relaxed);
        self.line_bytes.store(line_bytes, Ordering::Relaxed);
        self.expiry_ms.store(expiry.as_millis() as u64, Ordering::Relaxed);
    }

    /// Bytes per thread and line size; `None` when the cache is off.
    pub fn get(&self) -> Option<(usize, u32)> {
        let bytes = self.bytes_per_thread.load(Ordering::Relaxed);
        let line = self.line_bytes.load(Ordering::Relaxed);
        (bytes >= line as usize && line > 0).then_some((bytes, line))
    }

    pub fn expiry(&self) -> Duration {
        Duration::from_millis(self.expiry_ms.load(Ordering::Relaxed))
    }
}

struct Line {
    data: Vec<u8>,
    /// Its place in `ReadCache::order`.
    rank: u64,
    used: Instant,
    /// Blocks not sent yet; once none are left the line goes first.
    unsent: Vec<bool>,
    unsent_count: usize,
}

/// One disk thread's lines.
#[derive(Default)]
pub(crate) struct ReadCache {
    lines: BTreeMap<Key, Line>,
    /// Lines by rank, the first to evict first: lines read through come before every other
    /// line, then the others least recently used first.
    order: BTreeMap<u64, Key>,
    /// Counts up from the middle of the range for lines being read, down for lines read through.
    next_hot: u64,
    next_cold: u64,
    bytes: usize,
    /// When a line may next have expired; `None` while empty.
    next_check: Option<Instant>,
}

const MIDDLE: u64 = u64::MAX / 2;

impl ReadCache {
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn hot_rank(&mut self) -> u64 {
        if self.next_hot == 0 {
            (self.next_hot, self.next_cold) = (MIDDLE + 1, MIDDLE);
        }
        self.next_hot += 1;
        self.next_hot
    }

    fn cold_rank(&mut self) -> u64 {
        if self.next_hot == 0 {
            (self.next_hot, self.next_cold) = (MIDDLE + 1, MIDDLE);
        }
        self.next_cold -= 1;
        self.next_cold
    }

    /// Copies `length` bytes at `offset` of the piece out of the line starting at `start`.
    pub fn read(&mut self, key: Key, offset: u32, length: u32, now: Instant) -> Option<Vec<u8>> {
        let line = self.lines.get_mut(&key)?;
        let at = (offset - key.2) as usize;
        let out = line.data.get(at..at + length as usize)?.to_vec();
        line.used = now;
        let block = at / super::BLOCK_SIZE as usize;
        if std::mem::take(&mut line.unsent[block]) {
            line.unsent_count -= 1;
        }
        let read_through = line.unsent_count == 0;
        let old = line.rank;
        let rank = if read_through { self.cold_rank() } else { self.hot_rank() };
        self.lines.get_mut(&key).expect("present").rank = rank;
        self.order.remove(&old);
        self.order.insert(rank, key);
        Some(out)
    }

    /// Keeps a line just read from disk, the block at `offset` already sent, evicting others to
    /// stay within `limit` bytes.
    pub fn insert(&mut self, key: Key, data: Vec<u8>, offset: u32, limit: usize, now: Instant) {
        if data.len() > limit {
            return;
        }
        self.remove(&key);
        self.trim(limit - data.len());
        let blocks = data.len().div_ceil(super::BLOCK_SIZE as usize);
        let mut unsent = vec![true; blocks];
        unsent[(offset - key.2) as usize / super::BLOCK_SIZE as usize] = false;
        let unsent_count = blocks - 1;
        let rank = if unsent_count == 0 { self.cold_rank() } else { self.hot_rank() };
        self.bytes += data.len();
        self.order.insert(rank, key);
        self.lines.insert(key, Line { data, rank, used: now, unsent, unsent_count });
        self.next_check.get_or_insert(now);
    }

    fn remove(&mut self, key: &Key) {
        if let Some(line) = self.lines.remove(key) {
            self.order.remove(&line.rank);
            self.bytes -= line.data.len();
        }
    }

    /// Evicts lines, the first in `order` first, until at most `limit` bytes are left.
    pub fn trim(&mut self, limit: usize) {
        while self.bytes > limit {
            let Some((_, key)) = self.order.pop_first() else { break };
            let line = self.lines.remove(&key).expect("ordered lines are cached");
            self.bytes -= line.data.len();
        }
    }

    /// Forgets the piece's lines: it is being written.
    pub fn drop_piece(&mut self, storage: u64, piece: u32) {
        if self.lines.is_empty() {
            return;
        }
        let keys: Vec<Key> =
            self.lines.range((storage, piece, 0)..=(storage, piece, u32::MAX)).map(|(k, _)| *k).collect();
        for k in keys {
            self.remove(&k);
        }
    }

    /// Forgets every line of the storage: its files are released, deleted, moved or renamed.
    pub fn drop_storage(&mut self, storage: u64) {
        if self.lines.is_empty() {
            return;
        }
        let keys: Vec<Key> =
            self.lines.range((storage, 0, 0)..=(storage, u32::MAX, u32::MAX)).map(|(k, _)| *k).collect();
        for k in keys {
            self.remove(&k);
        }
    }

    /// When [`expire`](Self::expire) has something to do, if ever.
    pub fn next_check(&self, expiry: Duration) -> Option<Instant> {
        self.next_check.map(|t| t + expiry)
    }

    /// Evicts the lines unused for `expiry`.
    pub fn expire(&mut self, now: Instant, expiry: Duration) {
        let old: Vec<Key> = self
            .lines
            .iter()
            .filter(|(_, l)| now.duration_since(l.used) >= expiry)
            .map(|(k, _)| *k)
            .collect();
        for k in old {
            self.remove(&k);
        }
        self.next_check = self.lines.values().map(|l| l.used).min();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lines.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: u32 = super::super::BLOCK_SIZE;

    fn line(blocks: u32, fill: u8) -> Vec<u8> {
        vec![fill; (blocks * B) as usize]
    }

    #[test]
    fn hits_copy_the_block_out_of_the_line() {
        let mut c = ReadCache::default();
        let now = Instant::now();
        let mut data = line(4, 0);
        data[(2 * B) as usize..(3 * B) as usize].fill(7);
        c.insert((1, 5, 0), data, 0, 1 << 20, now);
        assert_eq!(c.read((1, 5, 0), 2 * B, B, now), Some(vec![7; B as usize]));
        assert_eq!(c.read((1, 5, 0), 2 * B + 10, 6, now), Some(vec![7; 6]), "a request inside a block");
        assert_eq!(c.read((1, 5, 4 * B), 4 * B, B, now), None, "another line");
        assert_eq!(c.read((1, 6, 0), 0, B, now), None, "another piece");
        assert_eq!(c.read((2, 5, 0), 0, B, now), None, "another storage");
        assert_eq!(c.bytes(), (4 * B) as usize);
    }

    #[test]
    fn evicts_read_through_lines_first_then_least_recently_used() {
        let mut c = ReadCache::default();
        let now = Instant::now();
        let limit = (6 * B) as usize;
        c.insert((1, 0, 0), line(2, 0), 0, limit, now);
        c.insert((1, 1, 0), line(2, 1), 0, limit, now);
        c.insert((1, 2, 0), line(2, 2), 0, limit, now);
        // piece 1 is read through; piece 0 is touched, so piece 2 is the least recently used
        assert!(c.read((1, 1, 0), B, B, now).is_some());
        assert!(c.read((1, 0, 0), 0, B, now).is_some());
        c.insert((1, 3, 0), line(2, 3), 0, limit, now);
        assert_eq!(c.len(), 3);
        assert!(c.read((1, 1, 0), 0, B, now).is_none(), "the line read through went first");
        c.insert((1, 4, 0), line(2, 4), 0, limit, now);
        assert!(c.read((1, 2, 0), 0, B, now).is_none(), "then the least recently used");
        assert!(c.read((1, 0, 0), B, B, now).is_some());
        assert_eq!(c.bytes(), limit);
    }

    #[test]
    fn a_line_larger_than_the_limit_is_not_kept() {
        let mut c = ReadCache::default();
        c.insert((1, 0, 0), line(4, 0), 0, (2 * B) as usize, Instant::now());
        assert_eq!(c.bytes(), 0);
        assert_eq!(c.next_check(Duration::from_secs(1)), None);
    }

    #[test]
    fn drops_pieces_and_storages() {
        let mut c = ReadCache::default();
        let now = Instant::now();
        for (s, p, o) in [(1, 0, 0), (1, 0, 2 * B), (1, 1, 0), (2, 0, 0)] {
            c.insert((s, p, o), line(2, 0), o, 1 << 20, now);
        }
        c.drop_piece(1, 0);
        assert_eq!(c.len(), 2);
        assert!(c.read((1, 1, 0), 0, B, now).is_some());
        c.drop_storage(1);
        assert_eq!(c.len(), 1);
        assert!(c.read((2, 0, 0), 0, B, now).is_some());
        assert_eq!(c.bytes(), (2 * B) as usize);
        c.trim(0);
        assert_eq!((c.len(), c.bytes()), (0, 0));
    }

    #[test]
    fn unused_lines_expire() {
        let mut c = ReadCache::default();
        let t0 = Instant::now();
        let expiry = Duration::from_secs(10);
        c.insert((1, 0, 0), line(2, 0), 0, 1 << 20, t0);
        c.insert((1, 1, 0), line(2, 0), 0, 1 << 20, t0 + Duration::from_secs(4));
        assert_eq!(c.next_check(expiry), Some(t0 + expiry));
        c.expire(t0 + Duration::from_secs(10), expiry);
        assert_eq!(c.len(), 1);
        assert_eq!(c.next_check(expiry), Some(t0 + Duration::from_secs(14)));
        // a hit keeps a line; the next check finds it fresh and moves on
        assert!(c.read((1, 1, 0), B, B, t0 + Duration::from_secs(12)).is_some());
        c.expire(t0 + Duration::from_secs(14), expiry);
        assert_eq!(c.len(), 1);
        assert_eq!(c.next_check(expiry), Some(t0 + Duration::from_secs(22)));
        c.expire(t0 + Duration::from_secs(22), expiry);
        assert_eq!((c.len(), c.bytes(), c.next_check(expiry)), (0, 0, None));
    }

    #[test]
    fn limits_turn_the_cache_off() {
        let l = Limits::default();
        assert_eq!(l.get(), None);
        l.set(1 << 20, 32 * B, Duration::from_secs(300));
        assert_eq!(l.get(), Some((1 << 20, 32 * B)));
        l.set(16 * B as usize, 32 * B, Duration::from_secs(300));
        assert_eq!(l.get(), None, "less than one line");
        l.set(1 << 20, 0, Duration::from_secs(300));
        assert_eq!(l.get(), None);
        assert_eq!(l.expiry(), Duration::from_secs(300));
    }
}
