//! `patorrent-nox`: the headless PaTorrent daemon (PLAN.md Phase 6).
//!
//! Phase 2: a command-line client on the real engine. `add` downloads `.torrent` files and,
//! with `--seed`, keeps seeding; `run` resumes everything saved in the state directory. State
//! (resume.db + metadata.pack) persists between runs, so a restart resumes without a recheck.
//! `demo` still drives the simulated engine for scale experiments. Arguments are parsed by hand
//! to keep the dependency list empty.

mod create;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use patorrent_core::{
    AddTorrentParams, Alert, IpFilter, MockConfig, PeerFlags, PeerInfo, PeerSource, ProxyType, Session,
    SessionParams, SessionStats, Settings, TorrentFlags, TorrentId, TorrentSource, TorrentStatus, VERSION,
};
use patorrent_frontend::{fmt, status};

/// PaTorrent's own license and the third-party license texts, built into the binary so a
/// copied-out executable still carries them (PolyForm "Notices", BSD-3-Clause clause 2).
const LICENSE: &str = include_str!("../../../LICENSE.md");
const NOTICES: &str = include_str!("../../../THIRD-PARTY-NOTICES.md");

const USAGE: &str = "\
patorrent-nox: headless PaTorrent

USAGE:
    patorrent-nox add <file.torrent>... [--save DIR] [OPTIONS]
    patorrent-nox run [OPTIONS]
    patorrent-nox demo [--torrents N] [--active FRACTION] [--seconds N]
    patorrent-nox create <file or dir> [-o OUT] [CREATE OPTIONS]
    patorrent-nox --version | --help | --license

COMMANDS:
    add     Add torrents, download them, and exit when they finish (keep seeding with --seed).
    run     Resume every torrent saved in the state directory and keep running.
    demo    Load N synthetic torrents on the simulated engine and print session totals.
    create  Make a .torrent (v1) from a file or directory.

CREATE OPTIONS:
    -o, --output FILE     Where to write the torrent (default: <name>.torrent)
    -t, --tracker URL     Tracker URL (repeatable; one tier each, or put several in one
                          tier with commas: -t URL1,URL2)
    -w, --web-seed URL    Web seed (BEP 19, repeatable)
    --node HOST:PORT      DHT bootstrap node (repeatable)
    --private             Set the private flag (no DHT, PEX or LSD)
    --piece-size KIB      Piece size in KiB (default: automatic)
    --comment TEXT        Comment
    --source TEXT         info \"source\" (private trackers use it for cross-seeding)
    --creator TEXT        \"created by\" (default: PaTorrent/<version>; \"\" to leave out)
    --no-date             Leave out the creation date
    --pad                 Align files to pieces with pad files (BEP 47)
    --mtime               Store file modification times
    --symlinks            Store symlinks as links instead of following them (Unix)
    --no-attributes       Do not store hidden / executable attributes
    --skip-hidden         Leave out files and folders whose names start with a dot
    --magnet              Also print the magnet link

OPTIONS:
    --save DIR        Download directory (default: current directory)
    --state DIR       Session state directory (default: per-user data directory)
    --no-state        Do not load or save any session state
    --seed            After downloading, keep seeding until stopped
    --super-seed      Super seed (BEP 16, \"initial seeding\") torrents that are complete;
                      implies --seed
    --seconds N       Stop after N seconds
    --port N          Listen port (default 6881)
    --listen LIST     Listen interfaces, as libtorrent's listen_interfaces: IP:PORT,
                      [IPv6]:PORT or DEVICE:PORT, comma-separated (overrides --port)
    --dl-limit KIB    Download limit in KiB/s (0 = unlimited)
    --ul-limit KIB    Upload limit in KiB/s (0 = unlimited)
    --ip-filter FILE  Block the address ranges in FILE (ipfilter.dat or P2P format)
    --proxy URL       Connect through a proxy: TYPE://[USER:PASS@]HOST:PORT with TYPE socks4,
                      socks5 or http (USER alone for socks4). Peers, web seeds, trackers and
                      (socks5 only) uTP and the DHT go through it; nothing incoming is accepted
    --verbose         Print tracker and peer events, and every 10 s where the connected
                      peers came from

Type q and Enter to stop cleanly (state is saved on the way out).
";

struct Opts {
    save: String,
    state: Option<PathBuf>,
    no_state: bool,
    seed: bool,
    super_seed: bool,
    seconds: Option<u64>,
    port: Option<u16>,
    listen: Option<String>,
    dl_limit: u32,
    ul_limit: u32,
    ip_filter: Option<PathBuf>,
    proxy: Option<Proxy>,
    verbose: bool,
    torrents: usize,
    active: f64,
    sources: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        save: ".".into(),
        state: None,
        no_state: false,
        seed: false,
        super_seed: false,
        seconds: None,
        port: None,
        listen: None,
        dl_limit: 0,
        ul_limit: 0,
        ip_filter: None,
        proxy: None,
        verbose: false,
        torrents: 1000,
        active: 0.02,
        sources: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        fn num<T: std::str::FromStr>(name: &str, v: String) -> Result<T, String> {
            v.parse().map_err(|_| format!("{name}: not a number"))
        }
        match a.as_str() {
            "--save" => o.save = value("--save")?,
            "--state" => o.state = Some(value("--state")?.into()),
            "--no-state" => o.no_state = true,
            "--seed" => o.seed = true,
            "--super-seed" => {
                o.super_seed = true;
                o.seed = true;
            }
            "--verbose" | "-v" => o.verbose = true,
            "--seconds" => o.seconds = Some(num("--seconds", value("--seconds")?)?),
            "--port" => o.port = Some(num("--port", value("--port")?)?),
            "--listen" => o.listen = Some(value("--listen")?),
            "--dl-limit" => o.dl_limit = num::<u32>("--dl-limit", value("--dl-limit")?)?.saturating_mul(1024),
            "--ul-limit" => o.ul_limit = num::<u32>("--ul-limit", value("--ul-limit")?)?.saturating_mul(1024),
            "--ip-filter" => o.ip_filter = Some(value("--ip-filter")?.into()),
            "--proxy" => o.proxy = Some(parse_proxy(&value("--proxy")?)?),
            "--torrents" => o.torrents = num("--torrents", value("--torrents")?)?,
            "--active" => o.active = num("--active", value("--active")?)?,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => o.sources.push(s.to_owned()),
        }
    }
    Ok(o)
}

/// A proxy from `--proxy`.
#[derive(Debug, PartialEq)]
struct Proxy {
    kind: ProxyType,
    host: String,
    port: u16,
    user: String,
    pass: String,
}

/// Parses `TYPE://[USER[:PASS]@]HOST:PORT`. Credentials pick the `_pw` types (libtorrent
/// `socks5_pw`, `http_pw`); SOCKS4 sends the user as its user id and has no password.
fn parse_proxy(s: &str) -> Result<Proxy, String> {
    let bad = |why: &str| format!("--proxy {s}: {why} (expected TYPE://[USER:PASS@]HOST:PORT)");
    let (scheme, rest) = s.split_once("://").ok_or_else(|| bad("no type"))?;
    let rest = rest.trim_end_matches('/');
    let (auth, hostport) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(a), h),
        None => (None, rest),
    };
    let (user, pass) = match auth {
        Some(a) => match a.split_once(':') {
            Some((u, p)) => (u.to_owned(), Some(p.to_owned())),
            None => (a.to_owned(), None),
        },
        None => (String::new(), None),
    };
    let kind = match (scheme.to_ascii_lowercase().as_str(), auth.is_some()) {
        ("socks4" | "socks4a", _) if pass.is_some() => return Err(bad("SOCKS4 has no password")),
        ("socks4" | "socks4a", _) => ProxyType::Socks4,
        ("socks5" | "socks5h", false) => ProxyType::Socks5,
        ("socks5" | "socks5h", true) => ProxyType::Socks5Pw,
        ("http", false) => ProxyType::Http,
        ("http", true) => ProxyType::HttpPw,
        _ => return Err(bad("TYPE must be socks4, socks5 or http")),
    };
    let (host, port) = hostport.rsplit_once(':').ok_or_else(|| bad("no port"))?;
    let port: u16 = port.parse().ok().filter(|&p| p != 0).ok_or_else(|| bad("invalid port"))?;
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if host.is_empty() {
        return Err(bad("no host"));
    }
    Ok(Proxy { kind, host: host.to_owned(), port, user, pass: pass.unwrap_or_default() })
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
            println!("patorrent-nox {VERSION}");
            return ExitCode::SUCCESS;
        }
        "--license" => {
            print!("{LICENSE}\n\n{NOTICES}");
            return ExitCode::SUCCESS;
        }
        "add" => parse(rest).and_then(|o| {
            if o.sources.is_empty() { Err("add: nothing to add".into()) } else { Ok(run(o, Mode::Add)) }
        }),
        "run" => parse(rest).map(|o| run(o, Mode::Run)),
        "create" => create::parse(rest).map(create::create),
        "demo" => parse(rest).map(demo),
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Add,
    Run,
}

/// Per-user state directory: `%APPDATA%\PaTorrent\nox` on Windows, `$XDG_DATA_HOME/patorrent/nox`
/// (or `~/.local/share/...`) elsewhere.
fn default_state_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("PaTorrent").join("nox"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
            .map(|d| d.join("patorrent").join("nox"))
    }
}

/// Lines typed on stdin ("q" quits).
fn stdin_lines() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        while std::io::stdin().read_line(&mut line).is_ok_and(|n| n > 0) {
            if tx.send(line.trim().to_owned()).is_err() {
                break;
            }
            line.clear();
        }
    });
    rx
}

fn run(o: Opts, mode: Mode) -> ExitCode {
    let state_dir = if o.no_state { None } else { o.state.clone().or_else(default_state_dir) };
    let mut settings =
        Settings { download_rate_limit: o.dl_limit, upload_rate_limit: o.ul_limit, ..Settings::default() };
    if let Some(p) = o.port {
        settings.listen_interfaces = format!("0.0.0.0:{p},[::]:{p}");
    }
    if let Some(l) = &o.listen {
        settings.listen_interfaces = l.clone();
    }
    if let Some(p) = &o.proxy {
        settings.proxy_type = p.kind;
        settings.proxy_hostname = p.host.clone();
        settings.proxy_port = p.port;
        settings.proxy_username = p.user.clone();
        settings.proxy_password = p.pass.clone();
        println!("* proxy: {:?} {}:{}", p.kind, p.host, p.port);
    }
    let session = match Session::start(SessionParams { settings, state_dir: state_dir.clone() }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot start the engine: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(d) = &state_dir {
        println!("* state: {}", d.display());
    }
    let h = session.handle();
    if let Some(path) = &o.ip_filter {
        match IpFilter::load(path) {
            Ok(l) => {
                println!(
                    "* IP filter: {} rules from {} ({} lines skipped)",
                    l.rules,
                    path.display(),
                    l.invalid
                );
                h.set_ip_filter(l.filter);
            }
            Err(e) => {
                eprintln!("error: {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    h.set_alert_notify(move || {
        let _ = wake_tx.send(());
    });

    let save =
        std::fs::canonicalize(&o.save).map(|p| p.to_string_lossy().into_owned()).unwrap_or(o.save.clone());
    let save = save.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(save);
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
        let mut p = AddTorrentParams::new(source, save.clone());
        // Started right away, outside the queue (µTorrent's "Force Start").
        p.flags = TorrentFlags::APPLY_IP_FILTER;
        if o.super_seed {
            // dropped by the engine once the check finds the torrent incomplete
            p.flags |= TorrentFlags::SUPER_SEEDING;
        }
        h.add_torrent(p);
        pending += 1;
    }
    if mode == Mode::Add && pending == 0 {
        return ExitCode::FAILURE;
    }

    let input = stdin_lines();
    let started = Instant::now();
    let mut rows: HashMap<TorrentId, TorrentStatus> = HashMap::new();
    let mut added: Vec<TorrentId> = Vec::new();
    let mut stats = SessionStats::default();
    let mut alerts = Vec::new();
    let mut failed = 0usize;
    let mut finished_at: Option<Instant> = None;
    let deadline = o.seconds.map(|s| started + Duration::from_secs(s));
    let mut next_report = Instant::now();
    let mut next_peer_report = Instant::now() + PEER_REPORT_INTERVAL;
    loop {
        let now = Instant::now();
        if now >= next_report {
            h.post_torrent_updates();
            h.post_session_stats();
            next_report = now + Duration::from_secs(1);
        }
        if o.verbose && now >= next_peer_report {
            for &id in &added {
                h.torrent(id).post_peer_info();
            }
            next_peer_report = now + PEER_REPORT_INTERVAL;
        }
        let _ = wake_rx.recv_timeout(next_report.saturating_duration_since(Instant::now()));
        h.pop_alerts(&mut alerts);
        let mut updated = false;
        for a in alerts.drain(..) {
            match a {
                Alert::StateUpdate(v) => {
                    updated = true;
                    for st in v {
                        rows.insert(st.id, st);
                    }
                }
                Alert::SessionStats(s) => stats = s,
                Alert::AddTorrentFailed { .. } => {
                    failed += 1;
                    eprintln!("! {}", a.message());
                }
                Alert::TorrentAdded { id, ref name, .. } => {
                    added.push(id);
                    println!("* {name}: added");
                }
                Alert::TorrentRemoved { id, .. } => {
                    rows.remove(&id);
                }
                Alert::TorrentFinished { id } => {
                    let name = rows.get(&id).map_or(String::new(), |r| r.name.clone());
                    println!("* {name}: finished downloading");
                }
                Alert::TorrentError { .. }
                | Alert::FileError { .. }
                | Alert::ListenFailed { .. }
                | Alert::LsdError { .. }
                | Alert::Proxy { .. }
                | Alert::PortmapFailed { .. } => {
                    eprintln!("! {}", a.message())
                }
                Alert::TrackerError { .. }
                | Alert::TrackerWarning { .. }
                | Alert::TrackerReply { .. }
                | Alert::DhtReply { .. }
                | Alert::LsdPeer { .. }
                | Alert::PortmapLog { .. }
                | Alert::PeerBlocked { .. }
                    if o.verbose =>
                {
                    println!("* {}", a.message())
                }
                Alert::ListenSucceeded { .. }
                | Alert::Log { .. }
                | Alert::AlertsDropped { .. }
                | Alert::DhtBootstrap
                | Alert::MetadataReceived { .. }
                | Alert::MetadataFailed { .. }
                | Alert::PortmapSucceeded { .. }
                | Alert::ExternalIp { .. } => {
                    println!("* {}", a.message())
                }
                Alert::StateChanged { .. } if o.verbose => println!("* {}", a.message()),
                Alert::PeerInfo { id, ref peers } => {
                    let name = rows.get(&id).map_or(String::new(), |r| r.name.clone());
                    println!("* {name}: {}", peer_sources(peers));
                }
                _ => {}
            }
        }
        if updated {
            report(&rows, &stats, started.elapsed());
        }
        let quit = matches!(input.try_recv().as_deref(), Ok("q" | "quit"));
        let mine_done = mode == Mode::Add
            && added.len() + failed >= pending
            && added.iter().all(|id| rows.get(id).is_some_and(|st| st.is_finished && st.error.is_empty()));
        if mine_done && finished_at.is_none() {
            finished_at = Some(Instant::now());
            if o.seed {
                println!("* download complete; seeding (q + Enter to stop)");
            }
        }
        if quit || (mine_done && !o.seed) || deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
    }
    println!("* shutting down");
    h.clear_alert_notify();
    session.shutdown();
    if failed > 0 || (mode == Mode::Add && finished_at.is_none() && o.seconds.is_none()) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn demo(o: Opts) -> ExitCode {
    let config = MockConfig {
        initial_torrents: o.torrents,
        active_fraction: o.active,
        save_path: o.save.clone(),
        ..MockConfig::default()
    };
    let started = Instant::now();
    let session = Session::start_mock(Settings::default(), config);
    let h = session.handle();
    let mut rows: HashMap<TorrentId, TorrentStatus> = HashMap::new();
    let mut stats = SessionStats::default();
    let mut alerts = Vec::new();
    let mut first_full_update = None;
    let deadline = started + Duration::from_secs(o.seconds.unwrap_or(5));
    while Instant::now() < deadline {
        h.post_torrent_updates();
        h.post_session_stats();
        std::thread::sleep(Duration::from_secs(1));
        h.pop_alerts(&mut alerts);
        for a in alerts.drain(..) {
            match a {
                Alert::StateUpdate(v) => {
                    for st in v {
                        rows.insert(st.id, st);
                    }
                    if first_full_update.is_none() && rows.len() >= o.torrents {
                        first_full_update = Some(started.elapsed());
                    }
                }
                Alert::SessionStats(s) => stats = s,
                _ => {}
            }
        }
        print_totals(&rows, &stats, started.elapsed());
    }
    if let Some(t) = first_full_update {
        println!(
            "first full status update for {} torrents after {:.0} ms",
            o.torrents,
            t.as_secs_f64() * 1e3
        );
    }
    session.shutdown();
    ExitCode::SUCCESS
}

fn print_totals(rows: &HashMap<TorrentId, TorrentStatus>, stats: &SessionStats, t: Duration) {
    println!(
        "[{:>4}s] {} torrents | D: {} T: {} | U: {} T: {} | {} peers | DHT: {} nodes | uTP: {} sockets{}{}",
        t.as_secs(),
        rows.len(),
        fmt::speed_always(stats.download_rate),
        fmt::size(stats.recv_payload_bytes),
        fmt::speed_always(stats.upload_rate),
        fmt::size(stats.sent_payload_bytes),
        stats.num_peers_connected,
        stats.dht_nodes,
        stats.utp_sockets,
        if stats.utp_packet_loss + stats.utp_timeouts > 0 {
            format!(" ({} lost, {} timeouts)", stats.utp_packet_loss, stats.utp_timeouts)
        } else {
            String::new()
        },
        if stats.has_incoming_connections { " | incoming OK" } else { "" },
    );
}

fn report(rows: &HashMap<TorrentId, TorrentStatus>, stats: &SessionStats, t: Duration) {
    print_totals(rows, stats, t);
    let mut v: Vec<&TorrentStatus> = rows.values().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    for st in v {
        println!(
            "         {:<40} {:>8} {:>7}  {:<20} D {:>10} U {:>10}  {:>3}({}) peers  ETA {}",
            truncate(&st.name, 40),
            fmt::size(st.total_wanted),
            fmt::percent(st.progress_ppm),
            truncate(&status::status_text(st, false), 20),
            fmt::speed(st.download_payload_rate),
            fmt::speed(st.upload_payload_rate),
            st.num_peers,
            st.list_peers,
            fmt::eta(st.eta()),
        );
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_owned() } else { s.chars().take(max - 1).chain(['…']).collect() }
}

/// How often `--verbose` reports where the connected peers came from.
const PEER_REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// "12 peers (8 connected, 5 over uTP): tracker 9/6, dht 4/2, pex 3/3, incoming 2/2": per
/// source, all connections and those past the handshake (a peer can count under several
/// sources).
fn peer_sources(peers: &[PeerInfo]) -> String {
    let up =
        |p: &&PeerInfo| !p.flags.contains(PeerFlags::CONNECTING) && !p.flags.contains(PeerFlags::HANDSHAKE);
    let count = |s: PeerSource| {
        let all = peers.iter().filter(|p| p.source.contains(s));
        format!("{}/{}", all.clone().count(), all.filter(up).count())
    };
    format!(
        "{} peers ({} connected, {} over uTP): tracker {}, dht {}, pex {}, lsd {}, incoming {}",
        peers.len(),
        peers.iter().filter(up).count(),
        peers.iter().filter(up).filter(|p| p.flags.contains(PeerFlags::UTP_SOCKET)).count(),
        count(PeerSource::TRACKER),
        count(PeerSource::DHT),
        count(PeerSource::PEX),
        count(PeerSource::LSD),
        count(PeerSource::INCOMING)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(kind: ProxyType, host: &str, port: u16, user: &str, pass: &str) -> Proxy {
        Proxy { kind, host: host.into(), port, user: user.into(), pass: pass.into() }
    }

    #[test]
    fn proxy_urls() {
        assert_eq!(
            parse_proxy("socks5://127.0.0.1:1080"),
            Ok(proxy(ProxyType::Socks5, "127.0.0.1", 1080, "", ""))
        );
        assert_eq!(
            parse_proxy("socks5://al:p@ss:w@proxy.example:1080/"),
            Ok(proxy(ProxyType::Socks5Pw, "proxy.example", 1080, "al", "p@ss:w"))
        );
        assert_eq!(
            parse_proxy("SOCKS4://bob@[::1]:9050"),
            Ok(proxy(ProxyType::Socks4, "::1", 9050, "bob", ""))
        );
        assert_eq!(parse_proxy("http://h:3128"), Ok(proxy(ProxyType::Http, "h", 3128, "", "")));
        assert_eq!(parse_proxy("http://u:p@h:3128"), Ok(proxy(ProxyType::HttpPw, "h", 3128, "u", "p")));
        for bad in ["h:1", "ftp://h:1", "socks5://h", "socks5://h:0", "socks5://:1", "socks4://u:p@h:1"] {
            assert!(parse_proxy(bad).is_err(), "{bad}");
        }
    }
}
