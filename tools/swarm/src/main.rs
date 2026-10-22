//! `lt-swarm`: scale, interop and swarm benchmarks (docs/ARCHITECTURE.md, "Performance targets").
//!
//! * `scale`: the engine API against the simulated engine: time to load N torrents, time to the
//!   first full status update, size of incremental updates, and process memory.
//! * `gen` / `parse`: a synthetic `.torrent` corpus, and how fast and how big parsed metadata is.
//!
//! * `loopback`: the real engine, a seed and a downloader in this process over 127.0.0.1:
//!   throughput and CPU time per byte.
//! * `load`: the real engine's cold start with N saved torrents (`--keep DIR` leaves the state there,
//!   `--reuse` opens it again), then with `--idle` the idle CPU while polled as the GUIs poll.
//!   It reports the working set and the live heap per torrent (a counting allocator).
//! * `interop`: the real engine downloading from qBittorrent, Transmission and Deluge, over TCP or
//!   uTP, or (`--super-seed`) super-seeding to them.
//! * `seed-disk`: the real engine seeding to several downloaders from a cold disk, with the read
//!   cache off and on: upload rate and disk reads per request.
//!
//! Output is `key=value` lines (machine readable, used by CI) followed by a human summary.

mod corpus;
mod engine;
mod heap;
mod interop;
mod seed_disk;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use patorrent_core::{Alert, MockConfig, Session, Settings};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const USAGE: &str = "\
lt-swarm: PaTorrent benchmarks

USAGE:
    lt-swarm scale [--torrents N] [--active FRACTION] [--seconds N]
    lt-swarm gen --out DIR [--count N] [--seed N]
    lt-swarm parse DIR
    lt-swarm loopback [--mb N] [--piece-kib N]
    lt-swarm load [--count N] [--peers N] [--keep DIR [--reuse]] [--idle SECONDS]
    lt-swarm interop [--clients qbittorrent,transmission,deluge] [--mb N]
                     [--encryption forced|enabled|disabled] [--version v1|hybrid|v2]
                     [--transport tcp|utp|utp-only] [--super-seed]
                     [--qbittorrent PATH] [--wsl-distro NAME] [--keep]
        --transport   tcp: no uTP anywhere it can be switched off; utp: our engine tries uTP
                      first (TCP fallback), qBittorrent uTP only, Transmission uTP on;
                      utp-only: as utp, with TCP off in our engine too
        --super-seed  reverse direction: the clients download from our engine, which
                      super-seeds (BEP 16); reports the copies uploaded
    lt-swarm seed-disk [--dir DIR] [--mb N] [--piece-kib N] [--peers N] [--seconds N]
                       [--cache off|on|both] [--line-blocks N] [--cache-mb N]
                       [--load THREADS] [--keep]
";

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("load") {
        // before anything is allocated that it should see (`load` reports the heap per torrent)
        heap::enable();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("scale") => match parse_scale(&args[1..]) {
            Ok((n, active, secs)) => scale(n, active, secs),
            Err(e) => {
                eprintln!("error: {e}\n\n{USAGE}");
                ExitCode::from(2)
            }
        },
        Some("gen") => {
            let (mut count, mut out, mut seed) = (1000usize, None, 1u64);
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                let Some(v) = it.next() else { return usage(&format!("{a} needs a value")) };
                match a.as_str() {
                    "--count" => match v.parse() {
                        Ok(n) => count = n,
                        Err(_) => return usage("--count: not a number"),
                    },
                    "--seed" => match v.parse() {
                        Ok(n) => seed = n,
                        Err(_) => return usage("--seed: not a number"),
                    },
                    "--out" => out = Some(std::path::PathBuf::from(v)),
                    _ => return usage(&format!("unknown option {a}")),
                }
            }
            match out {
                Some(out) => corpus::generate(count, &out, seed),
                None => usage("gen: --out is required"),
            }
        }
        Some("loopback") => {
            let (mut mb, mut piece) = (256usize, 256usize);
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                let v = it.next().and_then(|v| v.parse().ok());
                match (a.as_str(), v) {
                    ("--mb", Some(v)) => mb = v,
                    ("--piece-kib", Some(v)) => piece = v,
                    _ => return usage(&format!("bad option {a}")),
                }
            }
            engine::loopback(mb.max(1), piece.clamp(16, 16 * 1024), cpu_time)
        }
        Some("load") => {
            let mut o = engine::LoadOptions { count: 1000, peers: 0, keep: None, reuse: false, idle_secs: 0 };
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                if a == "--reuse" {
                    o.reuse = true;
                    continue;
                }
                match (a.as_str(), it.next()) {
                    ("--count", Some(n)) => match n.parse() {
                        Ok(n) => o.count = n,
                        Err(_) => return usage("--count: not a number"),
                    },
                    ("--peers", Some(n)) => match n.parse() {
                        Ok(n) => o.peers = n,
                        Err(_) => return usage("--peers: not a number"),
                    },
                    ("--idle", Some(n)) => match n.parse() {
                        Ok(n) => o.idle_secs = n,
                        Err(_) => return usage("--idle: not a number"),
                    },
                    ("--keep", Some(d)) => o.keep = Some(std::path::PathBuf::from(d)),
                    _ => {
                        return usage(
                            "load: expected --count N, --peers N, --keep DIR, --reuse, --idle SECONDS",
                        );
                    }
                }
            }
            engine::load(o, rss_bytes, cpu_time)
        }
        Some("interop") => {
            let mut opts = interop::Options::default();
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                if a == "--keep" {
                    opts.keep = true;
                    continue;
                }
                if a == "--super-seed" {
                    opts.super_seed = true;
                    continue;
                }
                let Some(v) = it.next() else { return usage(&format!("{a} needs a value")) };
                match a.as_str() {
                    "--clients" => {
                        let parsed: Option<Vec<_>> = v.split(',').map(interop::Client::parse).collect();
                        match parsed {
                            Some(c) if !c.is_empty() => opts.clients = c,
                            _ => return usage("--clients: expected qbittorrent, transmission or deluge"),
                        }
                    }
                    "--mb" => match v.parse() {
                        Ok(n) => opts.mb = n,
                        Err(_) => return usage("--mb: not a number"),
                    },
                    "--qbittorrent" => opts.qbittorrent = v.into(),
                    "--wsl-distro" => opts.wsl_distro = v.clone(),
                    "--version" => match interop::Version::parse(v) {
                        Some(version) => opts.version = version,
                        None => return usage("--version: expected v1, hybrid or v2"),
                    },
                    "--encryption" => match interop::Encryption::parse(v) {
                        Some(e) => opts.encryption = Some(e),
                        None => return usage("--encryption: expected forced, enabled or disabled"),
                    },
                    "--transport" => match interop::Transport::parse(v) {
                        Some(t) => opts.transport = Some(t),
                        None => return usage("--transport: expected tcp, utp or utp-only"),
                    },
                    _ => return usage(&format!("unknown option {a}")),
                }
            }
            interop::run(&opts)
        }
        Some("seed-disk") => {
            let mut opts = seed_disk::Options::default();
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                if a == "--keep" {
                    opts.keep = true;
                    continue;
                }
                let Some(v) = it.next() else { return usage(&format!("{a} needs a value")) };
                let num = |v: &str| v.parse::<u64>().ok().filter(|&n| n > 0);
                match (a.as_str(), num(v)) {
                    ("--dir", _) => opts.dir = Some(v.into()),
                    ("--mb", Some(n)) => opts.mb = n,
                    ("--piece-kib", Some(n)) if n >= 16 && n.is_power_of_two() => opts.piece_kib = n,
                    ("--peers", Some(n)) => opts.peers = n as usize,
                    ("--seconds", Some(n)) => opts.seconds = n,
                    ("--line-blocks", Some(n)) => opts.line_blocks = Some(n.min(1 << 16) as i32),
                    ("--cache-mb", Some(n)) => opts.cache_blocks = Some((n.min(1 << 16) * 64) as i32),
                    ("--load", _) if v == "0" => opts.load = 0,
                    ("--load", Some(n)) => opts.load = n as usize,
                    ("--cache", _) => {
                        opts.cache = match v.as_str() {
                            "off" => vec![false],
                            "on" => vec![true],
                            "both" => vec![false, true],
                            _ => return usage("--cache: expected off, on or both"),
                        }
                    }
                    _ => return usage(&format!("bad option {a} {v}")),
                }
            }
            seed_disk::run(&opts)
        }
        Some("parse") => match args.get(1) {
            Some(dir) => corpus::parse(std::path::Path::new(dir), rss_bytes),
            None => usage("parse: directory missing"),
        },
        _ => {
            print!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn usage(e: &str) -> ExitCode {
    eprintln!(
        "error: {e}

{USAGE}"
    );
    ExitCode::from(2)
}

fn parse_scale(args: &[String]) -> Result<(usize, f64, u64), String> {
    let (mut n, mut active, mut secs) = (10_000usize, 0.02f64, 5u64);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let v = it.next().ok_or_else(|| format!("{a} needs a value"))?;
        match a.as_str() {
            "--torrents" => n = v.parse().map_err(|_| "--torrents: not a number")?,
            "--active" => active = v.parse().map_err(|_| "--active: not a number")?,
            "--seconds" => secs = v.parse().map_err(|_| "--seconds: not a number")?,
            _ => return Err(format!("unknown option {a}")),
        }
    }
    Ok((n, active, secs))
}

fn scale(n: usize, active: f64, secs: u64) -> ExitCode {
    let rss_before = rss_bytes();
    let t0 = Instant::now();
    let session = Session::start_mock(
        Settings::default(),
        MockConfig { initial_torrents: n, active_fraction: active, ..MockConfig::default() },
    );
    let h = session.handle();
    let mut alerts = Vec::new();

    // First full update: every torrent is dirty after load.
    h.post_torrent_updates();
    let first = loop {
        h.pop_alerts(&mut alerts);
        if let Some(len) = alerts.iter().find_map(|a| match a {
            Alert::StateUpdate(v) => Some(v.len()),
            _ => None,
        }) {
            break len;
        }
        if t0.elapsed() > Duration::from_secs(60) {
            eprintln!("timed out waiting for the first state update");
            return ExitCode::FAILURE;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let first_update = t0.elapsed();
    let rss_loaded = rss_bytes();

    // Steady state: one update per second, as a GUI would request.
    let mut sizes = Vec::new();
    let mut latencies = Vec::new();
    for _ in 0..secs {
        std::thread::sleep(Duration::from_secs(1));
        let t = Instant::now();
        h.post_torrent_updates();
        loop {
            h.pop_alerts(&mut alerts);
            if let Some(len) = alerts.iter().find_map(|a| match a {
                Alert::StateUpdate(v) => Some(v.len()),
                _ => None,
            }) {
                sizes.push(len);
                latencies.push(t.elapsed());
                break;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    }
    let rss_steady = rss_bytes();
    let t_shutdown = Instant::now();
    session.shutdown();
    let shutdown = t_shutdown.elapsed();

    let avg_update =
        if sizes.is_empty() { 0.0 } else { sizes.iter().sum::<usize>() as f64 / sizes.len() as f64 };
    let max_latency = latencies.iter().max().copied().unwrap_or_default();
    let mb = |b: Option<u64>| b.map_or("n/a".to_owned(), |b| format!("{:.1}", b as f64 / (1 << 20) as f64));

    println!("engine=mock");
    println!("torrents={n}");
    println!("first_update_torrents={first}");
    println!("first_update_ms={:.1}", first_update.as_secs_f64() * 1e3);
    println!("steady_update_avg_torrents={avg_update:.0}");
    println!("steady_update_max_latency_ms={:.2}", max_latency.as_secs_f64() * 1e3);
    println!("rss_before_mb={}", mb(rss_before));
    println!("rss_loaded_mb={}", mb(rss_loaded));
    println!("rss_steady_mb={}", mb(rss_steady));
    println!("shutdown_ms={:.1}", shutdown.as_secs_f64() * 1e3);
    println!();
    println!(
        "{n} torrents: full update in {:.0} ms; then ~{avg_update:.0} changed per second \
         (max {:.2} ms round trip); RSS {} -> {} -> {} MB",
        first_update.as_secs_f64() * 1e3,
        max_latency.as_secs_f64() * 1e3,
        mb(rss_before),
        mb(rss_loaded),
        mb(rss_steady),
    );
    ExitCode::SUCCESS
}

/// User + kernel CPU time of this process.
#[cfg(windows)]
fn cpu_time() -> Option<Duration> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut a, mut b, mut k, mut u) = (z, z, z, z);
    // SAFETY: valid pseudo-handle and out-pointers to live stack values.
    let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut k, &mut u) } != 0;
    let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) * 100;
    ok.then(|| Duration::from_nanos(t(k) + t(u)))
}

#[cfg(target_os = "linux")]
fn cpu_time() -> Option<Duration> {
    // Fields 14 and 15 of /proc/self/stat: utime and stime in clock ticks (100 Hz on Linux).
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split(' ').collect();
    let ticks: u64 = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    Some(Duration::from_millis(ticks * 10))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn cpu_time() -> Option<Duration> {
    None
}

/// Resident set size of this process, where the platform makes it cheap to read.
#[cfg(windows)]
fn rss_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        (K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0).then_some(c.WorkingSetSize as u64)
    }
}

#[cfg(target_os = "linux")]
fn rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))?
        .trim()
        .trim_end_matches(" kB")
        .trim()
        .parse()
        .ok()?;
    Some(kb * 1024)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn rss_bytes() -> Option<u64> {
    None
}
