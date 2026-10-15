//! `gen` and `parse`: a synthetic `.torrent` corpus and the cost of loading it.
//!
//! `parse` reports what matters for loadability: how fast metadata parses, and how much memory a
//! parsed torrent keeps (info section with piece hashes, file table) compared to the file size.
//! That number is why the engine keeps metadata only for active torrents (docs/ARCHITECTURE.md).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use patorrent_core::bencode::Entry;
use patorrent_core::metainfo::TorrentInfo;

pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

const WORDS: [&str; 16] = [
    "Open",
    "Public",
    "Libre",
    "Archive",
    "Field",
    "Studio",
    "Night",
    "Analog",
    "Recordings",
    "Dataset",
    "Footage",
    "Lectures",
    "Sampler",
    "Atlas",
    "Sessions",
    "Scans",
];

/// Writes `count` single- and multi-file v1 torrents with random piece hashes (no real data).
pub fn generate(count: usize, out: &Path, seed: u64) -> ExitCode {
    if let Err(e) = std::fs::create_dir_all(out) {
        eprintln!("{}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    let t0 = Instant::now();
    let mut bytes = 0u64;
    for i in 0..count {
        let mut rng = Rng::new(seed ^ i as u64);
        let buf = synthetic_torrent(&mut rng, i);
        bytes += buf.len() as u64;
        let path = out.join(format!("{i:06}.torrent"));
        if let Err(e) = std::fs::write(&path, &buf) {
            eprintln!("{}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }
    println!("generated={count}");
    println!("bytes={bytes}");
    println!("gen_ms={:.0}", t0.elapsed().as_secs_f64() * 1e3);
    ExitCode::SUCCESS
}

pub(crate) fn synthetic_torrent(rng: &mut Rng, i: usize) -> Vec<u8> {
    // sizes from 10 MB to 20 GB, log-uniform; ~1500 pieces like common creators
    let total = (10e6f64.ln() + (20e9f64.ln() - 10e6f64.ln()) * rng.unit()).exp() as u64;
    let piece_length = (total / 1500).next_power_of_two().clamp(16 << 10, 16 << 20);
    let num_pieces = total.div_ceil(piece_length) as usize;
    let name = format!(
        "{} {} {} #{i:06}",
        WORDS[rng.below(16) as usize],
        WORDS[rng.below(16) as usize],
        1990 + rng.below(36)
    );

    let mut info = Entry::new_dict();
    *info.key("name") = name.as_str().into();
    *info.key("piece length") = (piece_length as i64).into();
    let mut pieces = vec![0u8; num_pieces * 20];
    for chunk in pieces.chunks_mut(8) {
        let v = rng.next().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    *info.key("pieces") = pieces.into();

    if rng.unit() < 0.4 {
        *info.key("length") = (total as i64).into();
    } else {
        let n = 2 + rng.below(40) as usize;
        let weights: Vec<u64> = (0..n).map(|_| 1 + rng.below(100)).collect();
        let wsum: u64 = weights.iter().sum();
        let mut left = total;
        let mut files = Vec::with_capacity(n);
        for (k, w) in weights.iter().enumerate() {
            let size = if k + 1 == n { left } else { total * w / wsum };
            left -= size;
            let mut f = Entry::new_dict();
            *f.key("length") = (size as i64).into();
            let mut path = Vec::new();
            if rng.unit() < 0.3 {
                path.push(Entry::from(format!("Disc {}", 1 + k / 10)));
            }
            path.push(Entry::from(format!("{:02} {}.bin", k + 1, WORDS[rng.below(16) as usize])));
            *f.key("path") = Entry::from(path);
            files.push(f);
        }
        *info.key("files") = Entry::from(files);
    }

    let mut t = Entry::new_dict();
    *t.key("announce") = "udp://tracker.example:6969/announce".into();
    *t.key("announce-list") = Entry::from(vec![
        Entry::from(vec![Entry::from("udp://tracker.example:6969/announce")]),
        Entry::from(vec![Entry::from("http://tracker2.example/announce")]),
    ]);
    *t.key("created by") = "lt-swarm".into();
    *t.key("creation date") = (1_500_000_000 + rng.below(250_000_000) as i64).into();
    *t.key("info") = info;
    t.bencode()
}

/// Parses every `.torrent` in `dir`, keeping all of them alive, and reports time and memory.
pub fn parse(dir: &Path, rss: fn() -> Option<u64>) -> ExitCode {
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "torrent"))
            .collect(),
        Err(e) => {
            eprintln!("{}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
    };
    paths.sort();

    // Read and parse one file at a time so the read buffers never accumulate: RSS growth over the
    // baseline is then the memory held by the parsed torrents themselves.
    let rss_before = rss();
    let mut loaded = Vec::with_capacity(paths.len());
    let mut failed = 0;
    let mut bytes = 0u64;
    let mut parse = std::time::Duration::ZERO;
    let t_total = Instant::now();
    for p in &paths {
        let Ok(b) = std::fs::read(p) else {
            failed += 1;
            continue;
        };
        bytes += b.len() as u64;
        let t = Instant::now();
        let r = TorrentInfo::from_bytes(&b);
        parse += t.elapsed();
        match r {
            Ok(ti) => loaded.push(ti),
            Err(_) => failed += 1,
        }
    }
    let read_ms = (t_total.elapsed() - parse).as_secs_f64() * 1e3;
    let rss_loaded = rss();
    let files: usize = loaded.iter().map(TorrentInfo::num_files).sum();
    let n = loaded.len();
    drop(loaded);
    let rss_dropped = rss();

    let per_torrent_us = if n == 0 { 0.0 } else { parse.as_secs_f64() * 1e6 / n as f64 };
    let mb = |b: u64| b as f64 / (1 << 20) as f64;
    let held = match (rss_before, rss_loaded) {
        (Some(a), Some(b)) => Some(b.saturating_sub(a)),
        _ => None,
    };
    println!("torrents={n}");
    println!("failed={failed}");
    println!("files={files}");
    println!("torrent_bytes={bytes}");
    println!("read_ms={read_ms:.0}");
    println!("parse_ms={:.1}", parse.as_secs_f64() * 1e3);
    println!("parse_us_per_torrent={per_torrent_us:.1}");
    println!("parse_mb_per_s={:.0}", mb(bytes) / parse.as_secs_f64().max(1e-9));
    if let Some(h) = held {
        println!("held_bytes_per_torrent={}", if n == 0 { 0 } else { h / n as u64 });
    }
    println!();
    println!(
        "{n} torrents ({:.1} MB of .torrent files, {files} files) parsed in {:.0} ms ({per_torrent_us:.1} µs each, {:.0} MB/s); \
         RSS {} -> {} with all parsed torrents held -> {} after dropping them",
        mb(bytes),
        parse.as_secs_f64() * 1e3,
        mb(bytes) / parse.as_secs_f64().max(1e-9),
        rss_before.map_or("n/a".into(), |b| format!("{:.1} MB", mb(b))),
        rss_loaded.map_or("n/a".into(), |b| format!("{:.1} MB", mb(b))),
        rss_dropped.map_or("n/a".into(), |b| format!("{:.1} MB", mb(b))),
    );
    if failed > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}
