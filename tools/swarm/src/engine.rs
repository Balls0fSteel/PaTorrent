//! Benchmarks of the real engine.
//!
//! * `loopback`: one seeding and one downloading session in this process, on 127.0.0.1;
//!   reports throughput and the process CPU time per downloaded byte.
//! * `load`: saves N synthetic torrents in a session store, then times a cold restart until
//!   every torrent is listed (PLAN.md Phase 2 exit criterion: 1,000 in under 0.5 s).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, Session, SessionHandle, SessionParams, Settings, TorrentFlags, TorrentSource,
    TorrentState,
};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lt-swarm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

fn settings() -> Settings {
    // A queue large enough that bulk adds lose no alerts.
    Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        alert_queue_size: 1 << 20,
        enable_dht: false,
        enable_lsd: false,
        ..Settings::default()
    }
}

/// Pops alerts until `f` returns `Some`, or `None` after `limit`.
fn wait<T>(h: &SessionHandle, limit: Duration, mut f: impl FnMut(&Alert) -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + limit;
    let mut alerts = Vec::new();
    let mut next_update = Instant::now();
    while Instant::now() < deadline {
        if Instant::now() >= next_update {
            h.post_torrent_updates();
            next_update += Duration::from_millis(100);
        }
        std::thread::sleep(Duration::from_millis(2));
        h.pop_alerts(&mut alerts);
        for a in &alerts {
            if let Some(v) = f(a) {
                return Some(v);
            }
        }
    }
    None
}

pub fn loopback(mb: usize, piece_kib: usize, cpu: fn() -> Option<Duration>) -> ExitCode {
    let size = mb << 20;
    let piece = piece_kib << 10;
    let seed_dir = temp_dir("loopback-seed");
    let leech_dir = temp_dir("loopback-leech");

    // Pseudo-random contents (incompressible, so nothing on the path can cheat).
    let mut data = vec![0u8; size];
    let mut x = 0x2545_f491_4f6c_dd1du64;
    for chunk in data.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        chunk.copy_from_slice(&x.to_le_bytes()[..chunk.len()]);
    }
    let mut pieces = Vec::with_capacity(size / piece * 20 + 20);
    for c in data.chunks(piece) {
        pieces.extend_from_slice(&Sha1::digest(c).0);
    }
    let mut info = Entry::new_dict();
    *info.key("length") = Entry::Int(size as i64);
    *info.key("name") = Entry::Bytes(b"loopback.bin".to_vec());
    *info.key("piece length") = Entry::Int(piece as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    let torrent = t.bencode();
    if let Err(e) = std::fs::write(seed_dir.join("loopback.bin"), &data) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    drop(data);

    let start = |dir: &Path| {
        let s = Session::start(SessionParams { settings: settings(), state_dir: None }).expect("session");
        let h = s.handle();
        let port = wait(&h, Duration::from_secs(5), |a| match a {
            Alert::ListenSucceeded { address, .. } => Some(address.port()),
            _ => None,
        })
        .expect("listening");
        let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(torrent.clone()), dir.to_string_lossy());
        p.flags = TorrentFlags::empty();
        h.add_torrent(p);
        let id = wait(&h, Duration::from_secs(5), |a| match a {
            Alert::TorrentAdded { id, .. } => Some(*id),
            _ => None,
        })
        .expect("added");
        (s, port, id)
    };
    let (seed, port, seed_id) = start(&seed_dir);
    let t_check = Instant::now();
    let checked = wait(&seed.handle(), Duration::from_secs(600), |a| match a {
        Alert::StateUpdate(v) => {
            v.iter().find(|s| s.id == seed_id && s.state == TorrentState::Seeding).map(|_| ())
        }
        _ => None,
    });
    if checked.is_none() {
        eprintln!("the seed never finished checking");
        return ExitCode::FAILURE;
    }
    let check_time = t_check.elapsed();

    let (leech, _, id) = start(&leech_dir);
    let cpu0 = cpu();
    let t0 = Instant::now();
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], port)));
    let done = wait(&leech.handle(), Duration::from_secs(900), |a| {
        matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(())
    });
    let elapsed = t0.elapsed();
    let cpu1 = cpu();
    leech.shutdown();
    seed.shutdown();
    let _ = std::fs::remove_dir_all(&seed_dir);
    let _ = std::fs::remove_dir_all(&leech_dir);
    if done.is_none() {
        eprintln!("download did not finish");
        return ExitCode::FAILURE;
    }
    let mbps = size as f64 / elapsed.as_secs_f64() / 1e6;
    println!("size_mb={mb}");
    println!("piece_kib={piece_kib}");
    println!("seed_check_ms={:.0}", check_time.as_secs_f64() * 1e3);
    println!("download_ms={:.0}", elapsed.as_secs_f64() * 1e3);
    println!("throughput_mb_s={mbps:.1}");
    if let (Some(a), Some(b)) = (cpu0, cpu1) {
        let cpu_s = (b - a).as_secs_f64();
        println!("cpu_s={cpu_s:.2}");
        println!("cpu_ns_per_byte={:.2}", cpu_s * 1e9 / size as f64);
    }
    println!();
    println!(
        "{mb} MiB over loopback in {:.2} s: {mbps:.0} MB/s ({:.1} Gbit/s); seed check {:.2} s",
        elapsed.as_secs_f64(),
        mbps * 8.0 / 1000.0,
        check_time.as_secs_f64()
    );
    ExitCode::SUCCESS
}

/// With `keep`, the state is written there and left in place (for example, to open the same
/// 10,000 torrents in the GUI).
pub fn load(count: usize, keep: Option<PathBuf>, rss: fn() -> Option<u64>) -> ExitCode {
    let state = match &keep {
        Some(d) => {
            let _ = std::fs::remove_dir_all(d);
            std::fs::create_dir_all(d).expect("state dir");
            d.clone()
        }
        None => temp_dir("load-state"),
    };
    let save = temp_dir("load-data");
    {
        let s = Session::start(SessionParams { settings: settings(), state_dir: Some(state.clone()) })
            .expect("session");
        let h = s.handle();
        let mut rng = crate::corpus::Rng::new(7);
        for i in 0..count {
            let bytes = crate::corpus::synthetic_torrent(&mut rng, i);
            let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(bytes), save.to_string_lossy());
            // Stopped torrents: what a large, mostly idle library looks like.
            p.flags = TorrentFlags::PAUSED;
            h.add_torrent(p);
        }
        let mut added = 0;
        wait(&h, Duration::from_secs(120), |a| {
            if matches!(a, Alert::TorrentAdded { .. }) {
                added += 1;
            }
            (added == count).then_some(())
        })
        .expect("all added");
        s.shutdown();
    }
    let db = std::fs::metadata(state.join("resume.db")).map(|m| m.len()).unwrap_or(0);
    let pack = std::fs::metadata(state.join("metadata.pack")).map(|m| m.len()).unwrap_or(0);

    let rss0 = rss();
    let t0 = Instant::now();
    let s = Session::start(SessionParams { settings: settings(), state_dir: Some(state.clone()) })
        .expect("session");
    let h = s.handle();
    let mut restored = 0;
    let mut listed = 0;
    let mut load_time = None;
    let ok = wait(&h, Duration::from_secs(60), |a| {
        match a {
            Alert::TorrentAdded { .. } => restored += 1,
            Alert::StateUpdate(v) => listed += v.len(),
            _ => {}
        }
        if restored == count && load_time.is_none() {
            load_time = Some(t0.elapsed());
        }
        (restored == count && listed >= count).then_some(())
    });
    let load_time = load_time.unwrap_or_default();
    let first_update = t0.elapsed();
    let rss1 = rss();
    s.shutdown();
    if keep.is_none() {
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(&save);
    }
    if ok.is_none() {
        eprintln!("only {restored} of {count} torrents restored");
        return ExitCode::FAILURE;
    }
    let mb = |b: Option<u64>| b.map_or("n/a".to_owned(), |b| format!("{:.1}", b as f64 / (1 << 20) as f64));
    println!("torrents={count}");
    println!("resume_db_kb={}", db / 1024);
    println!("metadata_pack_kb={}", pack / 1024);
    println!("restore_ms={:.1}", load_time.as_secs_f64() * 1e3);
    println!("first_update_torrents={listed}");
    println!("first_update_ms={:.1}", first_update.as_secs_f64() * 1e3);
    println!("rss_before_mb={}", mb(rss0));
    println!("rss_loaded_mb={}", mb(rss1));
    println!();
    println!(
        "{count} saved torrents restored in {:.0} ms (listed after {:.0} ms); store {} KB + {} KB",
        load_time.as_secs_f64() * 1e3,
        first_update.as_secs_f64() * 1e3,
        db / 1024,
        pack / 1024
    );
    ExitCode::SUCCESS
}
