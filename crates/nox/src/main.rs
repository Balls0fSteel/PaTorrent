//! `libretorrent-nox`: the headless LibreTorrent daemon (PLAN.md Phase 6).
//!
//! Phase 0: a command-line driver for the engine API, running on the simulated engine. It
//! exercises the same calls the daemon will use (add, alert notify, dirty-only status updates,
//! session stats). Arguments are parsed by hand to keep the dependency list empty.

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use libretorrent_core::{
    AddTorrentParams, Alert, MockConfig, Session, SessionStats, Settings, TorrentId, TorrentSource,
    TorrentStatus, VERSION,
};
use libretorrent_frontend::{fmt, status};

const USAGE: &str = "\
libretorrent-nox: headless LibreTorrent (Phase 0: simulated engine)

USAGE:
    libretorrent-nox add <magnet-uri | file.torrent>... [--save DIR] [--seconds N]
    libretorrent-nox demo [--torrents N] [--active FRACTION] [--seconds N]
    libretorrent-nox --version | --help

COMMANDS:
    add     Add torrents and print their progress once per second until they finish
            (or --seconds elapse).
    demo    Load N synthetic torrents and print session totals once per second.
";

struct Opts {
    save: String,
    seconds: Option<u64>,
    torrents: usize,
    active: f64,
    sources: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { save: ".".into(), seconds: None, torrents: 1000, active: 0.02, sources: Vec::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--save" => o.save = value("--save")?,
            "--seconds" => {
                o.seconds = Some(value("--seconds")?.parse().map_err(|_| "--seconds: not a number")?)
            }
            "--torrents" => {
                o.torrents = value("--torrents")?.parse().map_err(|_| "--torrents: not a number")?
            }
            "--active" => o.active = value("--active")?.parse().map_err(|_| "--active: not a number")?,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => o.sources.push(s.to_owned()),
        }
    }
    Ok(o)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => ("--help", &[][..]),
    };
    let result = match cmd {
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "-V" | "--version" => {
            println!("libretorrent-nox {VERSION}");
            return ExitCode::SUCCESS;
        }
        "add" => parse(rest).and_then(|o| {
            if o.sources.is_empty() { Err("add: nothing to add".into()) } else { Ok(run(o, false)) }
        }),
        "demo" => parse(rest).map(|o| run(o, true)),
        other => Err(format!("unknown command {other:?}")),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(o: Opts, demo: bool) -> ExitCode {
    let config = MockConfig {
        initial_torrents: if demo { o.torrents } else { 0 },
        active_fraction: o.active,
        save_path: o.save.clone(),
        ..MockConfig::default()
    };
    let started = Instant::now();
    let session = Session::start_mock(Settings::default(), config);
    let h = session.handle();

    // The engine thread calls this when alerts become available; we just wake our loop.
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    h.set_alert_notify(move || {
        let _ = wake_tx.send(());
    });

    let mut pending = 0usize;
    for src in &o.sources {
        let source = if src.starts_with("magnet:") {
            TorrentSource::Magnet(src.clone())
        } else {
            match std::fs::read(src) {
                Ok(bytes) => TorrentSource::TorrentFile(bytes),
                Err(e) => {
                    eprintln!("{src}: {e}");
                    continue;
                }
            }
        };
        h.add_torrent(AddTorrentParams::new(source, o.save.clone()));
        pending += 1;
    }
    if !demo && pending == 0 {
        return ExitCode::FAILURE;
    }

    let mut rows: HashMap<TorrentId, TorrentStatus> = HashMap::new();
    let mut stats = SessionStats::default();
    let mut alerts = Vec::new();
    let mut failed = 0usize;
    let mut first_full_update: Option<Duration> = None;
    let deadline = o.seconds.map(|s| started + Duration::from_secs(s));
    let mut next_report = Instant::now();
    loop {
        let now = Instant::now();
        if now >= next_report {
            h.post_torrent_updates();
            h.post_session_stats();
            next_report = now + Duration::from_secs(1);
        }
        let _ = wake_rx.recv_timeout(next_report.saturating_duration_since(Instant::now()));
        h.pop_alerts(&mut alerts);
        let mut updated = None;
        for a in alerts.drain(..) {
            match a {
                Alert::StateUpdate(v) => {
                    updated = Some(v.len());
                    for st in v {
                        rows.insert(st.id, st);
                    }
                    if demo && first_full_update.is_none() && rows.len() >= o.torrents {
                        first_full_update = Some(started.elapsed());
                    }
                }
                Alert::SessionStats(s) => stats = s,
                Alert::AddTorrentFailed { .. } => {
                    failed += 1;
                    eprintln!("! {}", a.message());
                }
                Alert::TorrentRemoved { id, .. } => {
                    rows.remove(&id);
                }
                Alert::TorrentAdded { .. }
                | Alert::TorrentFinished { .. }
                | Alert::MetadataReceived { .. }
                | Alert::Log { .. }
                | Alert::AlertsDropped { .. }
                | Alert::ListenSucceeded { .. } => println!("* {}", a.message()),
                _ => {}
            }
        }
        if let Some(n) = updated {
            report(&rows, &stats, n, started.elapsed(), demo);
        }
        let all_done = !demo && rows.len() + failed >= pending && rows.values().all(|st| st.is_finished);
        if all_done || deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
    }
    h.clear_alert_notify();
    if let Some(t) = first_full_update {
        println!(
            "first full status update for {} torrents after {:.0} ms",
            o.torrents,
            t.as_secs_f64() * 1e3
        );
    }
    session.shutdown();
    if failed > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn report(
    rows: &HashMap<TorrentId, TorrentStatus>,
    stats: &SessionStats,
    updated: usize,
    t: Duration,
    demo: bool,
) {
    println!(
        "[{:>4}s] {} torrents ({} updated) | D: {} T: {} | U: {} T: {} | DHT: {} nodes | {} peers",
        t.as_secs(),
        rows.len(),
        updated,
        fmt::speed_always(stats.download_rate),
        fmt::size(stats.recv_payload_bytes),
        fmt::speed_always(stats.upload_rate),
        fmt::size(stats.sent_payload_bytes),
        stats.dht_nodes,
        stats.num_peers_connected,
    );
    if demo {
        return;
    }
    let mut v: Vec<&TorrentStatus> = rows.values().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    for st in v {
        println!(
            "         {:<40} {:>8} {:>9}  {:<22} {:>11}  ETA {}",
            truncate(&st.name, 40),
            fmt::size(st.total_wanted),
            fmt::percent(st.progress_ppm),
            status::status_text(st, false),
            fmt::speed(st.download_payload_rate),
            fmt::eta(st.eta()),
        );
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_owned() } else { s.chars().take(max - 1).chain(['…']).collect() }
}
