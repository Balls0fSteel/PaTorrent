//! Smart disk I/O (D48, `docs/DISKIO.md`): the settings the disk threads read, the drive each
//! save path is on (detected once per path, on a disk thread), each drive's limit on jobs in
//! flight and the timings of its writes and reads that tell when it is busy, and the pacing of
//! gentle checks.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::drive::{self, DriveId, DriveKind};

/// Checking threads: `active_checking` of them take drives, one drive per thread.
pub(crate) const MAX_CHECKS: usize = 8;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The smart disk I/O settings, as the disk threads see them.
pub(crate) struct Config {
    gentle: AtomicBool,
    /// Bytes a second a check may read; 0: no cap.
    check_rate: AtomicU64,
    checks: AtomicUsize,
    tune: AtomicBool,
    overrides: Mutex<String>,
    /// Bumped when the overrides change, so the drives are looked up again.
    generation: AtomicU64,
    /// Time writes and reads (the busy-disk throttle is on).
    measure: AtomicBool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            gentle: AtomicBool::new(true),
            check_rate: AtomicU64::new(0),
            checks: AtomicUsize::new(4),
            tune: AtomicBool::new(true),
            overrides: Mutex::new(String::new()),
            generation: AtomicU64::new(0),
            measure: AtomicBool::new(false),
        }
    }
}

impl Config {
    /// `check_rate_mb`: `None` for no cap. `checks`: `active_checking`, -1 or more than
    /// [`MAX_CHECKS`] being [`MAX_CHECKS`].
    pub fn set(&self, gentle: bool, check_rate_mb: Option<u32>, checks: i32, tune: bool, overrides: &str) {
        self.gentle.store(gentle, Ordering::Relaxed);
        let rate = check_rate_mb.map_or(0, |mb| u64::from(mb.max(1)) * 1_000_000);
        self.check_rate.store(rate, Ordering::Relaxed);
        let checks = if checks < 1 { MAX_CHECKS } else { (checks as usize).min(MAX_CHECKS) };
        self.checks.store(checks, Ordering::Relaxed);
        self.tune.store(tune, Ordering::Relaxed);
        let mut o = lock(&self.overrides);
        if *o != overrides {
            *o = overrides.to_owned();
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn set_measure(&self, on: bool) {
        self.measure.store(on, Ordering::Relaxed);
    }

    pub fn measure(&self) -> bool {
        self.measure.load(Ordering::Relaxed)
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn gentle(&self) -> bool {
        self.gentle.load(Ordering::Relaxed)
    }

    pub fn check_rate(&self) -> u64 {
        self.check_rate.load(Ordering::Relaxed)
    }

    pub fn checks(&self) -> usize {
        self.checks.load(Ordering::Relaxed)
    }

    pub fn tune(&self) -> bool {
        self.tune.load(Ordering::Relaxed)
    }
}

/// How much longer than its usual a drive may take before it counts as busy.
fn allowance(kind: DriveKind) -> Duration {
    Duration::from_millis(match kind {
        DriveKind::Ssd => 10,
        DriveKind::Network => 100,
        DriveKind::Hdd | DriveKind::Removable | DriveKind::Unknown => 50,
    })
}

/// The timings in the last [`RECENT`] are the drive's current delay.
const RECENT: Duration = Duration::from_secs(2);
/// The drive's usual delay is the lowest per-second 10th percentile in this window.
const BASELINE: Duration = Duration::from_secs(180);
/// Timings needed in [`RECENT`] before the drive can count as busy.
const MIN_SAMPLES: usize = 8;
/// Timings kept per second (more are not needed for the percentiles).
const MAX_PER_SECOND: usize = 2048;

/// One kind of operation's timings on a drive, in microseconds.
#[derive(Default)]
struct Timings {
    recent: VecDeque<(Instant, u32)>,
    second: Vec<u32>,
    second_start: Option<Instant>,
    /// Per second: its start and its 10th percentile.
    seconds: VecDeque<(Instant, u32)>,
}

fn percentile(v: &mut [u32], p: usize) -> u32 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[(v.len() - 1) * p / 100]
}

impl Timings {
    fn add(&mut self, now: Instant, micros: u32) {
        self.roll(now);
        self.recent.push_back((now, micros));
        if self.recent.len() > MAX_PER_SECOND * 2 {
            self.recent.pop_front();
        }
        if self.second.len() < MAX_PER_SECOND {
            self.second.push(micros);
        }
        self.second_start.get_or_insert(now);
    }

    /// Closes the second that ended and drops what is too old.
    fn roll(&mut self, now: Instant) {
        if let Some(start) = self.second_start {
            if now.duration_since(start) >= Duration::from_secs(1) {
                let p10 = percentile(&mut self.second, 10);
                self.seconds.push_back((start, p10));
                self.second.clear();
                self.second_start = None;
            }
        }
        while self.seconds.front().is_some_and(|(t, _)| now.duration_since(*t) > BASELINE) {
            self.seconds.pop_front();
        }
        while self.recent.front().is_some_and(|(t, _)| now.duration_since(*t) > RECENT) {
            self.recent.pop_front();
        }
    }

    /// The current delay is more than `allowance` above the usual one.
    fn busy(&mut self, now: Instant, allowance: Duration) -> bool {
        self.roll(now);
        if self.recent.len() < MIN_SAMPLES || self.seconds.is_empty() {
            return false;
        }
        let usual = self.seconds.iter().map(|(_, p)| *p).min().unwrap_or(0);
        let mut recent: Vec<u32> = self.recent.iter().map(|(_, m)| *m).collect();
        let current = percentile(&mut recent, 90);
        u64::from(current) > u64::from(usual) + allowance.as_micros() as u64
    }
}

/// A drive, shared by every save path on it.
pub(crate) struct Drive {
    pub id: DriveId,
    in_flight: Mutex<usize>,
    freed: Condvar,
    writes: Mutex<Timings>,
    reads: Mutex<Timings>,
}

/// A job's place among a drive's jobs in flight, given back on drop.
pub(crate) struct Slot<'a>(Option<&'a Drive>);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        if let Some(d) = self.0 {
            *lock(&d.in_flight) -= 1;
            d.freed.notify_one();
        }
    }
}

impl Drive {
    fn new(id: DriveId) -> Drive {
        Drive {
            id,
            in_flight: Mutex::new(0),
            freed: Condvar::new(),
            writes: Mutex::new(Timings::default()),
            reads: Mutex::new(Timings::default()),
        }
    }

    /// Disk jobs that may run on this drive at once when tuned for its type: two on a hard disk
    /// or a removable one (the seeks of more cost more than they bring), four per server on a
    /// share; no limit on an SSD or an unknown drive.
    fn limit(&self) -> Option<usize> {
        match self.id.kind {
            DriveKind::Hdd | DriveKind::Removable => Some(2),
            DriveKind::Network => Some(4),
            DriveKind::Ssd | DriveKind::Unknown => None,
        }
    }

    /// Waits for a free place among the drive's jobs in flight (none needed without `tune`).
    pub fn enter(&self, tune: bool) -> Slot<'_> {
        let Some(limit) = self.limit().filter(|_| tune) else { return Slot(None) };
        let mut n = lock(&self.in_flight);
        while *n >= limit {
            n = self.freed.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        Slot(Some(self))
    }

    /// A write (`write`) or a read for a peer took `took`.
    pub fn record(&self, write: bool, took: Duration) {
        let micros = took.as_micros().min(u32::MAX as u128) as u32;
        lock(if write { &self.writes } else { &self.reads }).add(Instant::now(), micros);
    }

    /// Whether the drive's writes and its reads are slower than usual now.
    pub fn busy(&self, now: Instant) -> [bool; 2] {
        let a = allowance(self.id.kind);
        [lock(&self.writes).busy(now, a), lock(&self.reads).busy(now, a)]
    }
}

/// The drives the save paths are on.
#[derive(Default)]
pub(crate) struct Drives {
    inner: Mutex<DrivesInner>,
}

#[derive(Default)]
struct DrivesInner {
    generation: u64,
    by_path: HashMap<String, Arc<Drive>>,
    by_key: HashMap<(String, DriveKind), Arc<Drive>>,
    /// Drives found since the engine last asked, and notes, for the Logger.
    found: Vec<String>,
    /// Background I/O priority was found missing (noted once).
    no_background: bool,
}

impl Drives {
    /// The drive `save_path` is on; detected the first time (which may take as long as a few
    /// calls on that path take: a disk thread's work).
    pub fn for_path(&self, config: &Config, save_path: &str) -> Arc<Drive> {
        let generation = config.generation.load(Ordering::Relaxed);
        {
            let mut inner = lock(&self.inner);
            if inner.generation != generation {
                inner.by_path.clear();
                inner.generation = generation;
            }
            if let Some(d) = inner.by_path.get(save_path) {
                return d.clone();
            }
        }
        let path = Path::new(save_path);
        let mut id = drive::detect(path);
        let overrides = lock(&config.overrides).clone();
        if let Some(kind) = drive::overridden(&overrides, path).filter(|k| *k != id.kind) {
            id.describe = format!("{}, set to {}", id.describe, kind.as_str());
            id.kind = kind;
        }
        let mut inner = lock(&self.inner);
        let key = (id.key.clone(), id.kind);
        let d = match inner.by_key.get(&key) {
            Some(d) => d.clone(),
            None => {
                inner.found.push(format!("drive {}", id.describe));
                let d = Arc::new(Drive::new(id));
                inner.by_key.insert(key, d.clone());
                d
            }
        };
        inner.by_path.insert(save_path.to_owned(), d.clone());
        d
    }

    pub fn all(&self) -> Vec<Arc<Drive>> {
        lock(&self.inner).by_key.values().cloned().collect()
    }

    /// A gentle check could not lower its I/O priority: noted once for the Logger.
    pub fn no_background_io(&self) {
        let mut inner = lock(&self.inner);
        if !std::mem::replace(&mut inner.no_background, true) {
            let note =
                "no background I/O priority on this system: gentle checks only wait while the drive is slow";
            inner.found.push(note.into());
        }
    }

    pub fn take_found(&self) -> Vec<String> {
        std::mem::take(&mut lock(&self.inner).found)
    }
}

/// Assigns drives to checking threads: one drive keeps its thread, so its checks run one after
/// another, and a new drive takes the next thread in turn.
#[derive(Default)]
pub(crate) struct CheckThreads {
    by_drive: HashMap<String, usize>,
    next: usize,
}

impl CheckThreads {
    pub fn thread_for(&mut self, key: &str, threads: usize) -> usize {
        let threads = threads.clamp(1, MAX_CHECKS);
        if let Some(&t) = self.by_drive.get(key).filter(|t| **t < threads) {
            return t;
        }
        let t = self.next % threads;
        self.next = (t + 1) % threads;
        self.by_drive.insert(key.to_owned(), t);
        t
    }
}

/// Reads faster than this (bytes a second) came from the system's cache, not the drive.
const CACHED: f64 = 4e9;
/// Reads the usual speed is taken from.
const PACE_SAMPLES: usize = 128;

/// Paces a check's reads: gently, it waits as long as a read took when that read was clearly
/// slower than the drive's usual (someone else uses the disk), so it takes at most about half
/// of a busy disk; with a cap, it keeps the bytes read under the rate.
pub(crate) struct Pacer {
    gentle: bool,
    rate: u64,
    start: Instant,
    bytes: u64,
    /// Seconds per byte of recent reads from the drive.
    speeds: VecDeque<f64>,
}

impl Pacer {
    pub fn new(config: &Config, now: Instant) -> Pacer {
        Pacer {
            gentle: config.gentle(),
            rate: config.check_rate(),
            start: now,
            bytes: 0,
            speeds: VecDeque::new(),
        }
    }

    /// `bytes` were read in `took`; returns how long to wait before the next read.
    pub fn after_read(&mut self, bytes: usize, took: Duration, now: Instant) -> Duration {
        let mut wait = Duration::ZERO;
        let secs = took.as_secs_f64();
        if self.gentle && bytes > 0 && secs > 0.0 && bytes as f64 / secs < CACHED {
            let per_byte = secs / bytes as f64;
            if self.speeds.len() >= 8 && took >= Duration::from_millis(2) {
                let mut v: Vec<f64> = self.speeds.iter().copied().collect();
                v.sort_by(f64::total_cmp);
                let usual = v[v.len() / 5];
                if per_byte > usual * 2.0 {
                    wait = took.min(Duration::from_secs(1));
                }
            }
            self.speeds.push_back(per_byte);
            if self.speeds.len() > PACE_SAMPLES {
                self.speeds.pop_front();
            }
        }
        self.bytes += bytes as u64;
        if self.rate > 0 {
            let due = self.start + Duration::from_secs_f64(self.bytes as f64 / self.rate as f64);
            wait = wait.max(due.saturating_duration_since(now));
        }
        wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown(key: &str, kind: DriveKind) -> Drive {
        Drive::new(DriveId { key: key.into(), kind, describe: key.into() })
    }

    #[test]
    fn a_drive_is_busy_when_its_delay_rises_past_the_allowance() {
        let mut t = Timings::default();
        let start = Instant::now();
        for s in 0..10u64 {
            for i in 0..20u64 {
                t.add(start + Duration::from_millis(s * 1000 + i * 50), 200);
            }
        }
        let now = start + Duration::from_secs(10);
        assert!(!t.busy(now, Duration::from_millis(10)));
        for i in 0..20u64 {
            t.add(now + Duration::from_millis(i * 50), 30_000);
        }
        let later = now + Duration::from_secs(1);
        assert!(t.busy(later, Duration::from_millis(10)));
        assert!(!t.busy(later, Duration::from_millis(50)), "within a hard disk's allowance");
        // two quiet seconds later the slow timings are no longer recent
        assert!(!t.busy(later + Duration::from_secs(3), Duration::from_millis(10)));
    }

    #[test]
    fn a_few_slow_timings_say_nothing() {
        let mut t = Timings::default();
        let start = Instant::now();
        for i in 0..20u64 {
            t.add(start + Duration::from_millis(i * 100), 100);
        }
        let now = start + Duration::from_secs(2);
        for i in 0..3u64 {
            t.add(now + Duration::from_millis(i), 500_000);
        }
        // the 90th percentile of the last two seconds is still fast
        assert!(!t.busy(now + Duration::from_millis(10), Duration::from_millis(10)));
    }

    #[test]
    fn hard_disks_take_two_jobs_at_once_when_tuned() {
        let hdd = Arc::new(unknown("sda", DriveKind::Hdd));
        let a = hdd.enter(true);
        let b = hdd.enter(true);
        assert_eq!(*lock(&hdd.in_flight), 2);
        let h = hdd.clone();
        let waiter = std::thread::spawn(move || {
            let _c = h.enter(true);
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(!waiter.is_finished(), "a third job waits");
        drop(a);
        waiter.join().unwrap();
        drop(b);
        assert_eq!(*lock(&hdd.in_flight), 0);
        // untuned, or an SSD: no limit
        let _many: Vec<_> = (0..5).map(|_| hdd.enter(false)).collect();
        let ssd = unknown("nvme0", DriveKind::Ssd);
        let _more: Vec<_> = (0..5).map(|_| ssd.enter(true)).collect();
    }

    #[test]
    fn drives_keep_their_checking_thread() {
        let mut c = CheckThreads::default();
        assert_eq!(c.thread_for("a", 4), 0);
        assert_eq!(c.thread_for("b", 4), 1);
        assert_eq!(c.thread_for("a", 4), 0);
        assert_eq!(c.thread_for("c", 4), 2);
        // one thread: everything on it, as before smart disk I/O
        assert_eq!(c.thread_for("b", 1), 0);
        assert_eq!(c.thread_for("d", 1), 0);
    }

    #[test]
    fn overrides_change_the_drive_and_are_looked_up_again() {
        let config = Config::default();
        let drives = Drives::default();
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let first = drives.for_path(&config, &dir);
        assert!(Arc::ptr_eq(&first, &drives.for_path(&config, &dir)));
        assert_eq!(drives.take_found().len(), 1);
        config.set(true, None, 4, true, &format!("{dir}=hdd"));
        let d = drives.for_path(&config, &dir);
        assert_eq!(d.id.kind, DriveKind::Hdd);
        assert!(d.id.describe.ends_with("set to hdd"), "{}", d.id.describe);
    }

    #[test]
    fn gentle_checks_wait_after_a_slow_read() {
        let config = Config::default();
        let now = Instant::now();
        let mut p = Pacer::new(&config, now);
        let piece = 4 << 20;
        // 4 MiB in 20 ms: 200 MB/s, from the drive
        for _ in 0..10 {
            assert_eq!(p.after_read(piece, Duration::from_millis(20), now), Duration::ZERO);
        }
        // three times as long: another program reads too
        assert_eq!(p.after_read(piece, Duration::from_millis(60), now), Duration::from_millis(60));
        // from the system's cache: says nothing
        assert_eq!(p.after_read(piece, Duration::from_micros(500), now), Duration::ZERO);
        // not gently: never waits
        config.set(false, None, 4, true, "");
        let mut p = Pacer::new(&config, now);
        for _ in 0..10 {
            p.after_read(piece, Duration::from_millis(20), now);
        }
        assert_eq!(p.after_read(piece, Duration::from_millis(60), now), Duration::ZERO);
    }

    #[test]
    fn a_capped_check_keeps_under_its_rate() {
        let config = Config::default();
        config.set(false, Some(100), 4, true, "");
        let now = Instant::now();
        let mut p = Pacer::new(&config, now);
        // 50 MB read at once: due half a second after the start
        let wait = p.after_read(50_000_000, Duration::from_millis(10), now + Duration::from_millis(10));
        assert_eq!(wait, Duration::from_millis(490));
    }
}
