//! `seed-disk`: seeding from a cold disk, the case of libtorrent issue 9004 (upload speed falls
//! when the disk is busy, because every 16 KiB request is a read of its own).
//!
//! One seeding session serves `--peers` downloading sessions over 127.0.0.1 from a file in
//! `--dir` (put it on the disk to measure, a hard disk shows it best). Every downloader asks
//! for pieces in its own random order, so the seed reads all over the file. Before each run the
//! file is dropped from the OS cache (Windows: an unbuffered open purges it; Linux and FreeBSD:
//! `posix_fadvise(DONTNEED)`), so the reads reach the disk. `--load N` adds N threads reading
//! 64 KiB at random offsets of the same file, uncached, as another program on the disk would.
//! Each run reports the seed's upload rate and its disk counters, with the read cache off (the
//! one-read-per-request path libtorrent 2.x has) and on.

use std::fs::File;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, Session, SessionParams, SessionStats, TorrentFlags, TorrentSource, TorrentState,
};

use crate::engine::{settings, temp_dir, wait};

pub struct Options {
    pub dir: Option<PathBuf>,
    pub mb: u64,
    pub piece_kib: u64,
    pub peers: usize,
    pub seconds: u64,
    /// The runs, cache off and / or on.
    pub cache: Vec<bool>,
    pub load: usize,
    /// Leave the data file and its `.torrent` in `dir` for the next run.
    pub keep: bool,
    /// The seed's `read_cache_line_size` and `cache_size`, in 16 KiB blocks (engine defaults
    /// when `None`).
    pub line_blocks: Option<i32>,
    pub cache_blocks: Option<i32>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            dir: None,
            mb: 2048,
            piece_kib: 1024,
            peers: 8,
            seconds: 30,
            cache: vec![false, true],
            load: 0,
            keep: false,
            line_blocks: None,
            cache_blocks: None,
        }
    }
}

const NAME: &str = "lt-swarm-seed-disk.bin";

pub fn run(o: &Options) -> ExitCode {
    let temp = o.dir.is_none();
    let dir = o.dir.clone().unwrap_or_else(|| temp_dir("seed-disk"));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("{}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    let data = dir.join(NAME);
    let torrent = match prepare(&dir, o.mb << 20, o.piece_kib << 10) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}: {e}", data.display());
            return ExitCode::FAILURE;
        }
    };
    println!("dir={}", dir.display());
    println!("size_mb={}", o.mb);
    println!("piece_kib={}", o.piece_kib);
    println!("peers={}", o.peers);
    println!("seconds={}", o.seconds);
    println!("load_threads={}", o.load);
    let d = patorrent_core::Settings::default();
    println!("read_cache_line_size={}", o.line_blocks.unwrap_or(d.read_cache_line_size));
    println!("cache_size={}", o.cache_blocks.unwrap_or(d.cache_size));
    let mut results = Vec::new();
    for &cache in &o.cache {
        match run_once(o, &dir, &data, &torrent, cache) {
            Ok(r) => results.push((cache, r)),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if !o.keep {
        let _ = std::fs::remove_file(&data);
        let _ = std::fs::remove_file(data.with_extension("torrent"));
        if temp {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    println!();
    for (cache, r) in &results {
        println!(
            "read cache {}: {:.1} MB/s up from a cold disk ({} requests, {} disk reads, {} cache hits{})",
            if *cache { "on " } else { "off" },
            r.mb_s,
            r.blocks_read,
            r.read_ops,
            r.cache_hits,
            if o.load > 0 { format!(", load {:.1} MB/s", r.load_mb_s) } else { String::new() },
        );
    }
    ExitCode::SUCCESS
}

struct RunResult {
    mb_s: f64,
    blocks_read: u64,
    read_ops: u64,
    cache_hits: u64,
    load_mb_s: f64,
}

/// The data file and its `.torrent`, made unless both are there from a `--keep` run.
fn prepare(dir: &Path, size: u64, piece: u64) -> std::io::Result<Vec<u8>> {
    let data = dir.join(NAME);
    let torrent = data.with_extension("torrent");
    if let (Ok(m), Ok(t)) = (std::fs::metadata(&data), std::fs::read(&torrent)) {
        if m.len() == size && torrent_matches(&t, size, piece) {
            return Ok(t);
        }
    }
    eprintln!("writing {} MiB to {} ...", size >> 20, data.display());
    let mut f = std::io::BufWriter::with_capacity(4 << 20, File::create(&data)?);
    let mut pieces = Vec::with_capacity((size / piece) as usize * 20 + 20);
    let mut buf = vec![0u8; piece as usize];
    let mut x = 0x2545_f491_4f6c_dd1du64;
    let mut left = size;
    while left > 0 {
        let n = left.min(piece) as usize;
        for chunk in buf[..n].chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes()[..chunk.len()]);
        }
        f.write_all(&buf[..n])?;
        pieces.extend_from_slice(&Sha1::digest(&buf[..n]).0);
        left -= n as u64;
    }
    f.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    let mut info = Entry::new_dict();
    *info.key("length") = Entry::Int(size as i64);
    *info.key("name") = Entry::Bytes(NAME.as_bytes().to_vec());
    *info.key("piece length") = Entry::Int(piece as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    let bytes = t.bencode();
    std::fs::write(&torrent, &bytes)?;
    Ok(bytes)
}

fn torrent_matches(bytes: &[u8], size: u64, piece: u64) -> bool {
    patorrent_core::metainfo::TorrentInfo::from_bytes(bytes)
        .is_ok_and(|t| t.total_size() == size && u64::from(t.piece_length()) == piece)
}

fn run_once(o: &Options, dir: &Path, data: &Path, torrent: &[u8], cache: bool) -> Result<RunResult, String> {
    drop_from_os_cache(data).map_err(|e| format!("cannot drop {} from the OS cache: {e}", data.display()))?;

    let mut s = settings();
    s.use_read_cache = cache;
    s.read_cache_line_size = o.line_blocks.unwrap_or(s.read_cache_line_size);
    s.cache_size = o.cache_blocks.unwrap_or(s.cache_size);
    // Room for every downloader in the choker, and the default disk threads.
    s.unchoke_slots_limit = o.peers as i32 + 1;
    let seed =
        Session::start(SessionParams { settings: s.clone(), state_dir: None }).map_err(|e| e.to_string())?;
    let sh = seed.handle();
    let port = wait(&sh, Duration::from_secs(5), |a| match a {
        Alert::ListenSucceeded { address, .. } => Some(address.port()),
        _ => None,
    })
    .ok_or("the seed is not listening")?;
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(torrent.to_vec()), dir.to_string_lossy());
    p.flags = TorrentFlags::SEED_MODE;
    sh.add_torrent(p);
    let seed_id = wait(&sh, Duration::from_secs(5), |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    })
    .ok_or("the seed did not add the torrent")?;
    wait(&sh, Duration::from_secs(60), |a| match a {
        Alert::StateUpdate(v) => {
            v.iter().find(|s| s.id == seed_id && s.state == TorrentState::Seeding).map(|_| ())
        }
        _ => None,
    })
    .ok_or("the seed is not seeding")?;

    let stop = Arc::new(AtomicBool::new(false));
    let loaded = Arc::new(AtomicU64::new(0));
    let size = std::fs::metadata(data).map_err(|e| e.to_string())?.len();
    let load: Vec<_> = (0..o.load)
        .map(|i| {
            let (path, stop, loaded) = (data.to_path_buf(), stop.clone(), loaded.clone());
            std::thread::spawn(move || load_thread(&path, size, i as u64, &stop, &loaded))
        })
        .collect();

    let mut leeches = Vec::new();
    for i in 0..o.peers {
        let ldir = temp_dir(&format!("seed-disk-leech{i}"));
        let mut ls = settings();
        ls.use_read_cache = cache;
        let l = Session::start(SessionParams { settings: ls, state_dir: None }).map_err(|e| e.to_string())?;
        let lh = l.handle();
        let mut p =
            AddTorrentParams::new(TorrentSource::TorrentFile(torrent.to_vec()), ldir.to_string_lossy());
        p.flags = TorrentFlags::empty();
        lh.add_torrent(p);
        let id = wait(&lh, Duration::from_secs(5), |a| match a {
            Alert::TorrentAdded { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or("a downloader did not add the torrent")?;
        leeches.push((l, id, ldir));
    }
    let t0 = Instant::now();
    let before = stats(&sh).ok_or("no session stats")?;
    for (l, id, _) in &leeches {
        l.handle().torrent(*id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    let mut finished = 0;
    let mut alerts = Vec::new();
    let mut last = before.sent_payload_bytes;
    let mut next = t0 + Duration::from_secs(1);
    let deadline = t0 + Duration::from_secs(o.seconds);
    let mut per_second = Vec::new();
    let mut cache_peak = 0;
    while Instant::now() < deadline && finished < leeches.len() {
        std::thread::sleep(Duration::from_millis(20));
        for (l, id, _) in &leeches {
            l.handle().pop_alerts(&mut alerts);
            finished +=
                alerts.iter().filter(|a| matches!(a, Alert::TorrentFinished { id: i } if i == id)).count();
        }
        if Instant::now() >= next {
            next += Duration::from_secs(1);
            if let Some(s) = stats(&sh) {
                let rate = (s.sent_payload_bytes - last) as f64 / 1e6;
                per_second.push(rate);
                last = s.sent_payload_bytes;
                cache_peak = cache_peak.max(s.read_cache_bytes);
                eprint!("\r{}: {:>3} s, {rate:7.1} MB/s ", tag(cache), per_second.len());
            }
        }
    }
    eprintln!();
    let elapsed = t0.elapsed().as_secs_f64();
    let after = stats(&sh).ok_or("no session stats")?;
    stop.store(true, Ordering::Relaxed);
    for t in load {
        let _ = t.join();
    }
    for (l, _, ldir) in leeches {
        l.shutdown();
        let _ = std::fs::remove_dir_all(ldir);
    }
    seed.shutdown();

    let sent = after.sent_payload_bytes - before.sent_payload_bytes;
    let r = RunResult {
        mb_s: sent as f64 / elapsed / 1e6,
        blocks_read: after.disk_blocks_read - before.disk_blocks_read,
        read_ops: after.disk_read_ops - before.disk_read_ops,
        cache_hits: after.disk_cache_hits - before.disk_cache_hits,
        load_mb_s: loaded.load(Ordering::Relaxed) as f64 / elapsed / 1e6,
    };
    let mut sorted = per_second.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    let t = tag(cache);
    println!("{t}_seconds={elapsed:.1}");
    println!("{t}_upload_mb={:.0}", sent as f64 / 1e6);
    println!("{t}_upload_mb_s={:.1}", r.mb_s);
    println!("{t}_upload_median_mb_s={median:.1}");
    println!("{t}_blocks_read={}", r.blocks_read);
    println!("{t}_read_ops={}", r.read_ops);
    println!("{t}_cache_hits={}", r.cache_hits);
    println!("{t}_read_cache_mb_peak={:.1}", cache_peak as f64 / (1 << 20) as f64);
    if o.load > 0 {
        println!("{t}_load_mb_s={:.1}", r.load_mb_s);
    }
    Ok(r)
}

fn tag(cache: bool) -> &'static str {
    if cache { "cache_on" } else { "cache_off" }
}

fn stats(h: &patorrent_core::SessionHandle) -> Option<SessionStats> {
    h.post_session_stats();
    wait(h, Duration::from_secs(5), |a| match a {
        Alert::SessionStats(s) => Some(*s),
        _ => None,
    })
}

/// Reads 64 KiB at random 64 KiB-aligned offsets, past the OS cache, until `stop`.
fn load_thread(path: &Path, size: u64, seed: u64, stop: &AtomicBool, loaded: &AtomicU64) {
    const CHUNK: usize = 64 << 10;
    let Ok(mut f) = open_uncached(path) else { return };
    // An aligned buffer: unbuffered reads on Windows need sector-aligned memory.
    let mut raw = vec![0u8; CHUNK + 4096];
    let at = raw.as_ptr().align_offset(4096);
    let buf = &mut raw[at..at + CHUNK];
    let chunks = (size / CHUNK as u64).max(1);
    let mut x = 0x9e37_79b9_7f4a_7c15u64 ^ (seed + 1).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    while !stop.load(Ordering::Relaxed) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let offset = (x % chunks) * CHUNK as u64;
        if read_uncached(&mut f, buf, offset).is_err() {
            return;
        }
        loaded.fetch_add(CHUNK as u64, Ordering::Relaxed);
    }
}

#[cfg(windows)]
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;

#[cfg(windows)]
fn open_uncached(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_NO_BUFFERING).open(path)
}

#[cfg(windows)]
fn read_uncached(f: &mut File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    f.seek_read(buf, offset).map(|_| ())
}

/// An unbuffered open purges the file from the cache manager (no other handle is open yet).
#[cfg(windows)]
fn drop_from_os_cache(path: &Path) -> std::io::Result<()> {
    drop(open_uncached(path)?);
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod fadvise {
    use std::os::fd::AsRawFd;

    const POSIX_FADV_DONTNEED: i32 = 4;

    unsafe extern "C" {
        fn posix_fadvise(fd: i32, offset: i64, len: i64, advice: i32) -> i32;
    }

    pub fn dont_need(f: &std::fs::File, offset: u64, len: u64) -> std::io::Result<()> {
        // SAFETY: a valid open descriptor; the call only drops clean cached pages.
        match unsafe { posix_fadvise(f.as_raw_fd(), offset as i64, len as i64, POSIX_FADV_DONTNEED) } {
            0 => Ok(()),
            e => Err(std::io::Error::from_raw_os_error(e)),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn open_uncached(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn read_uncached(f: &mut File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.read_exact_at(buf, offset)?;
    fadvise::dont_need(f, offset, buf.len() as u64)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn drop_from_os_cache(path: &Path) -> std::io::Result<()> {
    let f = File::open(path)?;
    f.sync_all()?;
    fadvise::dont_need(&f, 0, 0)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "freebsd")))]
fn open_uncached(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "freebsd")))]
fn read_uncached(f: &mut File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::io::{Read, Seek};
    f.seek(std::io::SeekFrom::Start(offset))?;
    f.read_exact(buf)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "freebsd")))]
fn drop_from_os_cache(_path: &Path) -> std::io::Result<()> {
    eprintln!("warning: cannot drop the file from the OS cache here; the reads may not reach the disk");
    Ok(())
}
