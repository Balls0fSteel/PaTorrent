//! `patorrent-nox`: the headless PaTorrent daemon (PLAN.md Phase 6).
//!
//! Phase 2: a command-line client on the real engine. `add` downloads `.torrent` files and,
//! with `--seed`, keeps seeding; `run` resumes everything saved in the state directory. State
//! (resume.db + metadata.pack) persists between runs, so a restart resumes without a recheck.
//! `demo` still drives the simulated engine for scale experiments. Arguments are parsed by hand
//! to keep the dependency list empty.
//!
//! Phase 6: `run` as a service (docs/DAEMON.md): where the files go (`paths`), signals and
//! console events (`signals`), `--daemon`, `--pid-file`, `--log` and systemd's notifications
//! (`daemon`); on Windows, the service (`service`).

mod create;
mod daemon;
mod paths;
#[cfg(windows)]
mod service;
mod signals;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use patorrent_core::{
    AddTorrentParams, Alert, IpFilter, MockConfig, MoveCompleted, PeerFlags, PeerInfo, PeerSource, ProxyType,
    Session, SessionParams, SessionStats, Settings, TorrentFlags, TorrentId, TorrentSource, TorrentStatus,
    VERSION,
};
use patorrent_frontend::auto_shutdown::{self, AutoShutdown, Step, When};
use patorrent_frontend::{fmt, status};
use patorrent_nox::webui;

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
    patorrent-nox service install [--account A] [--start TYPE] [OPTIONS] | uninstall | start | stop
    patorrent-nox --version | --help | --license

COMMANDS:
    add     Add torrents, download them, and exit when they finish (keep seeding with --seed).
    run     Resume every torrent saved in the state directory and keep running.
    demo    Load N synthetic torrents on the simulated engine and print session totals.
    create  Make a .torrent (v1, hybrid or v2) from a file or directory.
    service The Windows service PaTorrentNox, which runs `run` (see WINDOWS SERVICE).

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
    --v1                  A v1-only torrent (default: hybrid, BitTorrent v2 (BEP 52) plus v1)
    --v2                  A v2-only torrent (v1-only clients cannot load it)
    --hybrid              A hybrid torrent (the default)
    --pad                 Align files to pieces with pad files (BEP 47; always for hybrid and v2)
    --mtime               Store file modification times
    --symlinks            Store symlinks as links instead of following them (Unix)
    --no-attributes       Do not store hidden / executable attributes
    --skip-hidden         Leave out files and folders whose names start with a dot
    --magnet              Also print the magnet link

OPTIONS:
    --save DIR        Download directory (default: current directory)
    --state-dir DIR   Session state directory: resume data, DHT state (default: per-user data
                      directory: $XDG_DATA_HOME/patorrent/nox or ~/.local/share/patorrent/nox;
                      %APPDATA%\\PaTorrent\\nox on Windows); also the config directory unless
                      --config-dir is given. --state DIR is the same
    --config-dir DIR  Web UI login and preferences (webui.json), default ipfilter.dat (default:
                      $XDG_CONFIG_HOME/patorrent/nox or ~/.config/patorrent/nox; the state
                      directory on Windows)
    --no-state        Do not load or save any session state
    --seed            After downloading, keep seeding until stopped
    --super-seed      Super seed (BEP 16, \"initial seeding\") torrents that are complete;
                      implies --seed
    --move-on-complete DIR
                      Move each torrent that finishes downloading into DIR (files and the
                      part file of skipped files; empty folders left behind are removed)
    --seconds N       Stop after N seconds
    --exit-when-done downloads|all
                      Exit (state saved) once nothing is downloading any more, or with all
                      nothing seeding either, after something has been (the GUI's Options >
                      Auto Shutdown > Quit When Downloads / Everything Complete)
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
    --webui [ADDR:]PORT  Serve the Web UI and qBittorrent-compatible WebAPI (ADDR defaults to
                      127.0.0.1; `run --webui 8080`)
    --webui-user NAME --webui-pass PASS
                      Web UI login (default user admin); the password is stored as a salted
                      PBKDF2 hash in the config directory. Without one, a temporary password
                      is printed at start
    --webui-https     Serve the Web UI over HTTPS with a self-signed certificate, made on first
                      use and kept in the config directory (webui-cert.pem, webui-key.pem;
                      renewed a month before it expires). Its fingerprint is printed at start
    --webui-cert FILE [--webui-key FILE]
                      Serve HTTPS with this certificate (PEM, the server's first, then its
                      chain) and private key (PEM, not encrypted; default: the certificate file)

SERVICE OPTIONS (docs/DAEMON.md):
    --daemon          Detach into the background (Unix): returns once the daemon is up
    --pid-file FILE   Write the process id to FILE and lock it; a second instance with the
                      same FILE refuses to start
    --log FILE        Append all output to FILE (reopened on SIGHUP)

WINDOWS SERVICE (docs/DAEMON.md; install, uninstall, start and stop need an administrator):
    service install [--account A] [--start TYPE] [OPTIONS]
                      Register the service PaTorrentNox (\"PaTorrent nox\"), which runs `run`
                      with OPTIONS (paths made absolute). The state defaults to
                      %ProgramData%\\PaTorrent\\nox (created so that only SYSTEM, Administrators
                      and the account can open it), the downloads to its Downloads folder and
                      --log to nox.log there. Restarts after a failure
        --account A   LocalService (default), NetworkService or LocalSystem
        --start TYPE  delayed (default; automatic, after the boot), auto or manual
    service uninstall Stop and remove the service (the state directory stays)
    service start | stop
                      Start or stop it and wait until it runs or has stopped (sc start / sc stop)
    service run       What the Service Control Manager starts; not for a console
    service debug [OPTIONS]
                      `service run` in this console, printing the status it would report

Type q and Enter, press Ctrl+C or send SIGTERM to stop cleanly (state is saved on the way
out; a second Ctrl+C quits at once). SIGHUP reloads the IP filter and reopens the log. Under
systemd (Type=notify) readiness is reported through NOTIFY_SOCKET. A Windows service stops on
`sc stop PaTorrentNox` and reloads on `sc control PaTorrentNox paramchange`.
";

struct Opts {
    save: String,
    state: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    no_state: bool,
    /// `--daemon`, `--pid-file`, `--log`.
    service: daemon::Options,
    seed: bool,
    super_seed: bool,
    move_on_complete: Option<String>,
    seconds: Option<u64>,
    exit_when_done: Option<When>,
    port: Option<u16>,
    listen: Option<String>,
    dl_limit: u32,
    ul_limit: u32,
    ip_filter: Option<PathBuf>,
    proxy: Option<Proxy>,
    verbose: bool,
    webui: Option<std::net::SocketAddr>,
    webui_user: Option<String>,
    webui_pass: Option<String>,
    webui_https: bool,
    webui_cert: Option<PathBuf>,
    webui_key: Option<PathBuf>,
    torrents: usize,
    active: f64,
    /// `run --mock-library N` (undocumented): the simulated engine with N synthetic torrents
    /// instead of the real one, to measure the Web UI against the 10,000-torrent profile.
    mock_library: usize,
    sources: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        save: ".".into(),
        state: None,
        config_dir: None,
        no_state: false,
        service: daemon::Options::default(),
        seed: false,
        super_seed: false,
        move_on_complete: None,
        seconds: None,
        exit_when_done: None,
        port: None,
        listen: None,
        dl_limit: 0,
        ul_limit: 0,
        ip_filter: None,
        proxy: None,
        verbose: false,
        webui: None,
        webui_user: None,
        webui_pass: None,
        webui_https: false,
        webui_cert: None,
        webui_key: None,
        torrents: 1000,
        active: 0.02,
        mock_library: 0,
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
            "--state" | "--state-dir" => o.state = Some(value(a.as_str())?.into()),
            "--config-dir" => o.config_dir = Some(value("--config-dir")?.into()),
            "--no-state" => o.no_state = true,
            "--daemon" => o.service.daemon = true,
            "--pid-file" => o.service.pid_file = Some(value("--pid-file")?.into()),
            "--log" => o.service.log = Some(value("--log")?.into()),
            "--seed" => o.seed = true,
            "--super-seed" => {
                o.super_seed = true;
                o.seed = true;
            }
            "--move-on-complete" => o.move_on_complete = Some(value("--move-on-complete")?),
            "--verbose" | "-v" => o.verbose = true,
            "--seconds" => o.seconds = Some(num("--seconds", value("--seconds")?)?),
            "--exit-when-done" => {
                o.exit_when_done = Some(match value("--exit-when-done")?.as_str() {
                    "downloads" => When::Downloads,
                    "all" => When::Everything,
                    v => return Err(format!("--exit-when-done {v}: expected downloads or all")),
                })
            }
            "--port" => o.port = Some(num("--port", value("--port")?)?),
            "--listen" => o.listen = Some(value("--listen")?),
            "--dl-limit" => o.dl_limit = num::<u32>("--dl-limit", value("--dl-limit")?)?.saturating_mul(1024),
            "--ul-limit" => o.ul_limit = num::<u32>("--ul-limit", value("--ul-limit")?)?.saturating_mul(1024),
            "--ip-filter" => o.ip_filter = Some(value("--ip-filter")?.into()),
            "--proxy" => o.proxy = Some(parse_proxy(&value("--proxy")?)?),
            "--webui" => o.webui = Some(webui::parse_addr(&value("--webui")?)?),
            "--webui-user" => o.webui_user = Some(value("--webui-user")?),
            "--webui-pass" => o.webui_pass = Some(value("--webui-pass")?),
            "--webui-https" => o.webui_https = true,
            "--webui-cert" => o.webui_cert = Some(value("--webui-cert")?.into()),
            "--webui-key" => o.webui_key = Some(value("--webui-key")?.into()),
            "--torrents" => o.torrents = num("--torrents", value("--torrents")?)?,
            "--active" => o.active = num("--active", value("--active")?)?,
            "--mock-library" => o.mock_library = num("--mock-library", value("--mock-library")?)?,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => o.sources.push(s.to_owned()),
        }
    }
    if o.webui_key.is_some() && o.webui_cert.is_none() {
        return Err("--webui-key needs --webui-cert".into());
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
        #[cfg(windows)]
        "service" => service::main(rest),
        #[cfg(not(windows))]
        "service" => {
            Err("service is for Windows; on Unix see docs/DAEMON.md (systemd, rc.d, --daemon)".into())
        }
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

/// `--daemon` changes to `/`: the paths given relative to the working directory are made
/// absolute first.
fn absolutize(o: &mut Opts) {
    fn abs(p: &mut PathBuf) {
        if let Ok(a) = std::path::absolute(&*p) {
            *p = a;
        }
    }
    fn abs_str(s: &mut String) {
        if let Ok(a) = std::path::absolute(&*s) {
            *s = a.to_string_lossy().into_owned();
        }
    }
    abs_str(&mut o.save);
    let dirs =
        [&mut o.state, &mut o.config_dir, &mut o.ip_filter, &mut o.service.pid_file, &mut o.service.log];
    for p in dirs.into_iter().flatten() {
        abs(p);
    }
    if let Some(d) = &mut o.move_on_complete {
        abs_str(d);
    }
    for s in o.sources.iter_mut().filter(|s| !s.starts_with("magnet:")) {
        abs_str(s);
    }
}

/// The state and config directories (`paths`), with `webui.json` moved from where it was
/// kept before the config directory existed. `None`s with `--no-state`.
fn state_dirs(o: &Opts) -> paths::Dirs {
    if o.no_state {
        return paths::Dirs::default();
    }
    let dirs = paths::resolve(o.state.clone(), o.config_dir.clone());
    if dirs.state.is_none() {
        eprintln!("! no state directory (HOME / APPDATA not set): nothing is saved; use --state-dir DIR");
    }
    if let (Some(old), Some(new)) = (&dirs.legacy_config, &dirs.config) {
        match paths::migrate(old, new) {
            Ok(done) => {
                for (file, moved) in done {
                    let how = if moved { "moved" } else { "copied" };
                    let name = file.file_name().unwrap_or_default().to_string_lossy();
                    println!(
                        "* {how} {name} from {} to the config directory {}",
                        old.display(),
                        new.display()
                    );
                }
            }
            Err(e) => {
                eprintln!("! moving the Web UI settings from {} to {}: {e}", old.display(), new.display())
            }
        }
    }
    dirs
}

/// SIGHUP: the `--log` file reopened and the IP filter's block list read again (the Web UI's
/// BitTorrent page choice when it runs, else `--ip-filter`). `webui.json` is not re-read: the
/// running daemon owns it (it rewrites it on every change made in the Web UI).
fn reload(
    o: &Opts,
    service: &mut daemon::Service,
    web: Option<&webui::WebUi>,
    h: &patorrent_core::SessionHandle,
) {
    let log_error = service.reloading();
    println!("* {}: reloading", if cfg!(windows) { "PARAMCHANGE" } else { "SIGHUP" });
    if let Some(e) = log_error {
        eprintln!("! {e}");
    }
    match (web, &o.ip_filter) {
        (Some(w), _) => {
            if let Some(line) = w.reload_ip_filter() {
                println!("* {line}");
            }
        }
        (None, Some(path)) => match IpFilter::load(path) {
            Ok(l) => {
                println!(
                    "* IP filter: {} rules from {} ({} lines skipped)",
                    l.rules,
                    path.display(),
                    l.invalid
                );
                h.set_ip_filter(l.filter);
            }
            Err(e) => eprintln!("! IP filter: {}: {e} (the old rules stay)", path.display()),
        },
        (None, None) => {}
    }
    service.reloaded();
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

fn run(mut o: Opts, mode: Mode) -> ExitCode {
    if o.service.daemon {
        absolutize(&mut o);
    }
    // first, while this is the only thread: `--daemon` forks
    let mut service = match daemon::Service::start(&o.service) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    // the language of the Web UI and of the status lines (PATORRENT_LANG; D20)
    for p in patorrent_frontend::lang::init("") {
        eprintln!("! {p}");
    }
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    let signal_wake = wake_tx.clone();
    if let Err(e) = signals::install(move || {
        let _ = signal_wake.send(());
    }) {
        eprintln!("! cannot install the signal handlers: {e}");
    }
    let dirs = state_dirs(&o);
    let state_dir = dirs.state.clone();
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
    let session = if o.mock_library > 0 {
        // the Web UI's scale profile: nothing auto-managed runs, only the --active fraction
        // (force-started) moves data, as in the nox webui tests
        settings.active_downloads = 0;
        settings.active_seeds = 0;
        settings.active_limit = 0;
        println!(
            "* simulated engine with {} synthetic torrents, {:.1} % of them active",
            o.mock_library,
            o.active * 100.0
        );
        Session::start_mock(
            settings.clone(),
            MockConfig {
                initial_torrents: o.mock_library,
                active_fraction: o.active,
                save_path: o.save.clone(),
                ..MockConfig::default()
            },
        )
    } else {
        match Session::start(SessionParams { settings: settings.clone(), state_dir: state_dir.clone() }) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot start the engine: {e}");
                return ExitCode::FAILURE;
            }
        }
    };
    if let Some(d) = &state_dir {
        println!("* state: {}", d.display());
    }
    if let Some(c) = dirs.config.as_ref().filter(|&c| Some(c) != state_dir.as_ref()) {
        println!("* config: {}", c.display());
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
    let move_dir = o
        .move_on_complete
        .as_ref()
        .map(|d| std::path::absolute(d).map_or(d.clone(), |p| p.to_string_lossy().into_owned()));
    let web = match o
        .webui
        .map(|addr| start_webui(&o, addr, &h, settings, dirs.config.clone(), &save, &move_dir))
    {
        Some(Err(e)) => {
            eprintln!("error: cannot start the Web UI: {e}");
            return ExitCode::FAILURE;
        }
        w => w.and_then(Result::ok),
    };
    service.ready(&match &web {
        Some(w) => format!("running, Web UI on {}", w.url()),
        None => "running".to_owned(),
    });

    let mut move_completed = move_dir.map(|d| {
        println!("* completed downloads move to {d}");
        MoveCompleted::new(d)
    });
    // the Web UI's Directories preferences take over "Move completed downloads" when it runs
    let mut gui_seen = 0u64;
    // Torrents being moved; `add` waits for them before it exits.
    let mut moving: std::collections::HashSet<TorrentId> = Default::default();
    let input = stdin_lines();
    let started = Instant::now();
    let mut rows: HashMap<TorrentId, TorrentStatus> = HashMap::new();
    let mut added: Vec<TorrentId> = Vec::new();
    let mut stats = SessionStats::default();
    let mut alerts = Vec::new();
    let mut failed = 0usize;
    let mut finished_at: Option<Instant> = None;
    let deadline = o.seconds.map(|s| started + Duration::from_secs(s));
    // a few seconds' grace: a torrent added just now, or a move about to start, calls it off
    let mut exit_when_done = AutoShutdown::new(Duration::from_secs(5));
    exit_when_done
        .set_mode(o.exit_when_done.map(|w| auto_shutdown::Mode::new(auto_shutdown::Action::Quit, w)));
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
        if let Some(w) = &web {
            w.on_alerts(&alerts);
            if w.sync_move_completed(&mut gui_seen, &mut move_completed) && gui_seen > 1 {
                match &move_completed {
                    Some(m) => println!("* completed downloads move to {}", m.dir),
                    None => println!("* completed downloads stay where they are"),
                }
            }
        }
        let mut updated = false;
        for a in alerts.drain(..) {
            if let Some(id) = move_completed.as_mut().and_then(|m| m.on_alert(&a, &h)) {
                moving.insert(id);
            }
            match a {
                Alert::StorageMoved { id, .. } => {
                    moving.remove(&id);
                    println!("* {}", a.message());
                }
                Alert::StorageMovedFailed { id, .. } => {
                    moving.remove(&id);
                    eprintln!("! {}", a.message());
                }
                Alert::FileRenamed { .. } => println!("* {}", a.message()),
                Alert::FileRenameFailed { .. } => eprintln!("! {}", a.message()),
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
            && moving.is_empty()
            && added.len() + failed >= pending
            && added.iter().all(|id| rows.get(id).is_some_and(|st| st.is_finished && st.error.is_empty()));
        if mine_done && finished_at.is_none() {
            finished_at = Some(Instant::now());
            if o.seed {
                println!("* download complete; seeding (q + Enter to stop)");
            }
        }
        let quit = quit || web.as_ref().is_some_and(|w| w.shutdown_requested());
        let ev = signals::take();
        if ev.reload {
            reload(&o, &mut service, web.as_ref(), &h);
        }
        if let Some(name) = ev.stop {
            println!("* {name} received (again to quit at once, without saving)");
        }
        let quit = quit || ev.stop.is_some();
        let busy =
            o.exit_when_done.is_some_and(|w| !moving.is_empty() || auto_shutdown::any_busy(w, rows.values()));
        let all_done = match exit_when_done.update(busy, Instant::now()) {
            Step::Start(secs) => {
                println!("* --exit-when-done: done, exiting in {secs} s");
                false
            }
            Step::Abort => {
                println!("* --exit-when-done: new work, not exiting yet");
                false
            }
            Step::Act(_) => true,
            Step::Wait | Step::Tick(_) => false,
        };
        if quit || all_done || (mine_done && !o.seed) || deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
    }
    println!("* shutting down");
    service.stopping();
    drop(web);
    h.clear_alert_notify();
    session.shutdown();
    println!("* stopped");
    // the pid file goes before a Windows close / shutdown handler lets the process end
    drop(service);
    signals::finished();
    if failed > 0 || (mode == Mode::Add && finished_at.is_none() && o.seconds.is_none()) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn start_webui(
    o: &Opts,
    addr: std::net::SocketAddr,
    h: &patorrent_core::SessionHandle,
    settings: Settings,
    config_dir: Option<PathBuf>,
    save: &str,
    move_dir: &Option<String>,
) -> std::io::Result<webui::WebUi> {
    let mut c = webui::Config::new(addr, settings, save);
    c.username = o.webui_user.clone().unwrap_or(c.username);
    c.password = o.webui_pass.clone();
    // webui.json and the default ipfilter.dat live in the config directory (`paths`)
    c.state_dir = config_dir;
    c.move_completed_dir = move_dir.clone();
    c.ip_filter = o.ip_filter.clone();
    c.https = match (&o.webui_cert, o.webui_https) {
        (Some(cert), _) => Some(webui::https::Certificate::Files {
            cert: cert.clone(),
            key: o.webui_key.clone().unwrap_or_else(|| cert.clone()),
        }),
        (None, true) => Some(webui::https::Certificate::Generated {
            dir: c.state_dir.clone(),
            names: webui::https::default_names(addr),
        }),
        (None, false) => None,
    };
    let w = webui::WebUi::start(h.clone(), c)?;
    println!("* Web UI: {}", w.url());
    if let Some(t) = w.https() {
        let expires = patorrent_frontend::fmt::date_time(t.not_after);
        match (&t.cert_path, t.generated) {
            (Some(path), true) => println!("* Web UI: new self-signed certificate {}", path.display()),
            (Some(path), false) => println!("* Web UI: certificate {}", path.display()),
            (None, _) => println!("* Web UI: self-signed certificate for this run only (--no-state)"),
        }
        println!("* Web UI: certificate SHA-256 {} (valid until {expires})", t.fingerprint);
    }
    if let Some(p) = w.temporary_password() {
        println!(
            "* Web UI: no password set; temporary password for this run: {p} (set one with --webui-pass)"
        );
    }
    Ok(w)
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
    fn move_on_complete_takes_a_directory() {
        let args: Vec<String> = ["a.torrent", "--move-on-complete", "done"].map(String::from).into();
        let o = parse(&args).unwrap();
        assert_eq!(o.move_on_complete.as_deref(), Some("done"));
        assert_eq!(o.sources, ["a.torrent"]);
        assert!(parse(&["--move-on-complete".to_owned()]).is_err());
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn service_and_directory_options() {
        let o = parse(&args(&[
            "--state-dir",
            "s",
            "--config-dir",
            "c",
            "--daemon",
            "--pid-file",
            "nox.pid",
            "--log",
            "nox.log",
        ]))
        .unwrap();
        assert_eq!((o.state, o.config_dir), (Some("s".into()), Some("c".into())));
        assert_eq!(
            o.service,
            daemon::Options { daemon: true, pid_file: Some("nox.pid".into()), log: Some("nox.log".into()) }
        );
        // --state is the older spelling
        assert_eq!(parse(&args(&["--state", "x"])).unwrap().state, Some("x".into()));
        let o = parse(&args(&[])).unwrap();
        assert_eq!((o.state, o.config_dir, o.service), (None, None, daemon::Options::default()));
        for flag in ["--state-dir", "--config-dir", "--pid-file", "--log"] {
            let e = parse(&args(&[flag])).err().expect(flag);
            assert_eq!(e, format!("{flag} needs a value"));
        }
        assert!(parse(&args(&["--deamon"])).is_err());
    }

    #[test]
    fn daemon_paths_are_made_absolute() {
        let mut o = parse(&args(&[
            "a.torrent",
            "magnet:?xt=urn:btih:x",
            "--save",
            "dl",
            "--pid-file",
            "p",
            "--log",
            "l",
        ]))
        .unwrap();
        absolutize(&mut o);
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(PathBuf::from(&o.save), cwd.join("dl"));
        assert_eq!(o.service.pid_file, Some(cwd.join("p")));
        assert_eq!(o.service.log, Some(cwd.join("l")));
        assert_eq!(
            o.sources,
            [cwd.join("a.torrent").to_string_lossy().into_owned(), "magnet:?xt=urn:btih:x".into()]
        );
        assert_eq!(o.state, None);
    }

    #[test]
    fn exit_when_done_takes_downloads_or_all() {
        let o = parse(&args(&["--exit-when-done", "downloads"])).unwrap();
        assert_eq!(o.exit_when_done, Some(When::Downloads));
        let o = parse(&args(&["--exit-when-done", "all"])).unwrap();
        assert_eq!(o.exit_when_done, Some(When::Everything));
        assert_eq!(parse(&args(&[])).unwrap().exit_when_done, None);
        assert!(parse(&args(&["--exit-when-done", "seeds"])).is_err());
        assert!(parse(&args(&["--exit-when-done"])).is_err());
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
