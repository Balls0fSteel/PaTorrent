//! `interop`: PaTorrent downloads a test torrent from other clients (PLAN.md Phase 2 exit
//! criteria, reverse direction).
//!
//! For each client: generate a 3-file torrent with pseudo-random contents, start the client as a
//! seed of that data with a throwaway configuration (DHT, LSD, PeX and port mapping off, everything
//! else at its defaults), wait until a BitTorrent handshake shows it has every piece, then let an
//! in-process PaTorrent session download from it and compare the files byte for byte.
//!
//! * qBittorrent: the installed app with a temporary `--profile` (Windows; elsewhere point
//!   `--qbittorrent` at `qbittorrent-nox`).
//! * Transmission (`transmission-cli`) and Deluge (`deluged` + `deluge-console`): on Windows in a
//!   WSL distro (`--wsl-distro`, default `Debian`), elsewhere on this machine.
//!
//! Readiness is client-agnostic: the probe connects, handshakes and counts the pieces in the
//! client's `bitfield` and `have` messages, so no client needs a control API.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, Session, SessionHandle, SessionParams, Settings, TorrentFlags, TorrentId,
    TorrentSource, TorrentState, TorrentStatus,
};

const NAME: &str = "lt-interop";
const PIECE: usize = 256 * 1024;
const SEED_READY_LIMIT: Duration = Duration::from_secs(180);
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    QBittorrent,
    Transmission,
    Deluge,
}

impl Client {
    pub const ALL: [Client; 3] = [Client::QBittorrent, Client::Transmission, Client::Deluge];

    pub fn parse(s: &str) -> Option<Client> {
        match s {
            "qbittorrent" | "qbt" => Some(Client::QBittorrent),
            "transmission" | "tr" => Some(Client::Transmission),
            "deluge" | "de" => Some(Client::Deluge),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Client::QBittorrent => "qbittorrent",
            Client::Transmission => "transmission",
            Client::Deluge => "deluge",
        }
    }
}

pub struct Options {
    pub clients: Vec<Client>,
    pub mb: usize,
    pub qbittorrent: PathBuf,
    pub wsl_distro: String,
    pub keep: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            clients: Client::ALL.to_vec(),
            mb: 40,
            qbittorrent: if cfg!(windows) {
                PathBuf::from(r"C:\Program Files\qBittorrent\qbittorrent.exe")
            } else {
                PathBuf::from("qbittorrent-nox")
            },
            wsl_distro: "Debian".into(),
            keep: false,
        }
    }
}

/// The test torrent and where its data lives.
struct Fixture {
    work: PathBuf,
    torrent_path: PathBuf,
    torrent: Vec<u8>,
    info_hash: [u8; 20],
    num_pieces: usize,
    /// Relative paths below `NAME`.
    files: Vec<String>,
    total: u64,
}

fn fixture(work: &Path, mb: usize) -> std::io::Result<Fixture> {
    let total = mb.max(1) << 20;
    // Odd sizes so pieces straddle files and the last piece is short.
    let a = total / 4 + 12_345;
    let c = 54_321.min(total / 8);
    let sizes = [("alpha.bin", a), ("sub/beta.bin", total - a - c), ("sub/gamma.txt", c)];
    let seed = work.join("seed").join(NAME);
    let mut all = Vec::with_capacity(total);
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let mut list = Entry::new_list();
    let mut files = Vec::new();
    for (name, size) in sizes {
        let mut data = vec![0u8; size];
        for chunk in data.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes()[..chunk.len()]);
        }
        let path = seed.join(name);
        std::fs::create_dir_all(path.parent().expect("has a parent"))?;
        std::fs::write(&path, &data)?;
        all.extend_from_slice(&data);
        let mut f = Entry::new_dict();
        *f.key("length") = Entry::Int(size as i64);
        *f.key("path") = Entry::List(name.split('/').map(|p| Entry::Bytes(p.as_bytes().to_vec())).collect());
        list.list_mut().push(f);
        files.push(name.to_owned());
    }
    let mut pieces = Vec::new();
    for c in all.chunks(PIECE) {
        pieces.extend_from_slice(&Sha1::digest(c).0);
    }
    let mut info = Entry::new_dict();
    *info.key("files") = list;
    *info.key("name") = Entry::Bytes(NAME.as_bytes().to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let info_hash = Sha1::digest(&info.bencode()).0;
    let mut t = Entry::new_dict();
    *t.key("created by") = Entry::Bytes(b"lt-swarm interop".to_vec());
    *t.key("info") = info;
    let torrent = t.bencode();
    let torrent_path = work.join(format!("{NAME}.torrent"));
    std::fs::write(&torrent_path, &torrent)?;
    Ok(Fixture {
        work: work.to_path_buf(),
        torrent_path,
        torrent,
        info_hash,
        num_pieces: total.div_ceil(PIECE),
        files,
        total: total as u64,
    })
}

// --- Seeds --------------------------------------------------------------------------------------

/// A client seeding the fixture.
struct Seed {
    child: Child,
    addr: SocketAddr,
    /// Shell command that kills what the child started (WSL processes outlive `wsl.exe`).
    kill: Option<String>,
    log: PathBuf,
}

/// Runs a POSIX shell script: in the WSL distro on Windows, with `sh` elsewhere.
fn shell(opts: &Options, script: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("wsl.exe");
        // `-e` runs `sh` directly, not through the login shell, which would expand `$` again.
        c.args(["-d", &opts.wsl_distro, "-e", "sh", "-c", script]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", script]);
        c
    }
}

fn shell_output(opts: &Options, script: &str) -> Result<String, String> {
    let out =
        shell(opts, script).stdin(Stdio::null()).output().map_err(|e| format!("cannot run a shell: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`{script}` failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The fixture's work directory as the shell sees it.
fn shell_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        let s = s.trim_start_matches("//?/");
        let (drive, rest) = s.split_once(':').unwrap_or(("c", s));
        format!("/mnt/{}{rest}", drive.to_ascii_lowercase())
    } else {
        s
    }
}

/// Address to reach a shell-side client from here: the WSL distro's own address on Windows.
fn shell_host(opts: &Options) -> Result<std::net::IpAddr, String> {
    if !cfg!(windows) {
        return Ok([127, 0, 0, 1].into());
    }
    let out = shell_output(opts, "hostname -I")?;
    out.split_whitespace()
        .next()
        .and_then(|a| a.parse().ok())
        .ok_or_else(|| format!("no address in `hostname -I` output: {out:?}"))
}

fn free_local_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map_or(43_210, |a| a.port())
}

/// A port for a shell-side client. Distinct per run and per client, outside the usual defaults.
fn shell_port(client: Client) -> u16 {
    let base = 40_000 + (std::process::id() % 2_000) as u16 * 10;
    base + client as u16 * 3
}

fn start_seed(client: Client, f: &Fixture, opts: &Options) -> Result<Seed, String> {
    let log = f.work.join(format!("{}.log", client.key()));
    let log_file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let err_file = log_file.try_clone().map_err(|e| e.to_string())?;
    match client {
        Client::QBittorrent => {
            let port = free_local_port();
            let profile = f.work.join("qbt-profile");
            let config = profile.join("qBittorrent").join("config");
            std::fs::create_dir_all(&config).map_err(|e| e.to_string())?;
            let ini = format!(
                "[LegalNotice]\r\nAccepted=true\r\n\r\n\
                 [BitTorrent]\r\nSession\\Port={port}\r\nSession\\DHTEnabled=false\r\n\
                 Session\\LSDEnabled=false\r\nSession\\PeXEnabled=false\r\n\
                 Session\\AddTorrentStopped=false\r\n\r\n\
                 [Network]\r\nPortForwardingEnabled=false\r\n\r\n\
                 [Preferences]\r\nConnection\\ResolvePeerCountries=false\r\nGeneral\\NoSplashScreen=true\r\n"
            );
            std::fs::write(config.join("qBittorrent.ini"), ini).map_err(|e| e.to_string())?;
            let child = Command::new(&opts.qbittorrent)
                .arg(format!("--profile={}", profile.display()))
                .arg(format!("--save-path={}", f.work.join("seed").display()))
                .args(["--skip-dialog=true", "--add-stopped=false"])
                .arg(&f.torrent_path)
                .stdin(Stdio::null())
                .stdout(log_file)
                .stderr(err_file)
                .spawn()
                .map_err(|e| format!("cannot start {}: {e}", opts.qbittorrent.display()))?;
            Ok(Seed { child, addr: SocketAddr::from(([127, 0, 0, 1], port)), kill: None, log })
        }
        Client::Transmission | Client::Deluge => {
            let host = shell_host(opts)?;
            let port = shell_port(client);
            let dir = format!("/tmp/{NAME}-{}/{}", std::process::id(), client.key());
            let src = shell_path(&f.work);
            let prepare = format!(
                "set -e; rm -rf {dir}; mkdir -p {dir}/cfg; cp -r {src}/seed/{NAME} {dir}/; cp {src}/{NAME}.torrent {dir}/"
            );
            let run = if client == Client::Transmission {
                format!(
                    "printf '%s' '{{\"dht-enabled\": false, \"lpd-enabled\": false, \"pex-enabled\": false, \
                     \"port-forwarding-enabled\": false, \"rpc-enabled\": false, \"peer-port\": {port}}}' \
                     > {dir}/cfg/settings.json; \
                     transmission-cli --version 2>&1 | head -1; \
                     exec stdbuf -oL timeout 900 transmission-cli -g {dir}/cfg -w {dir} -p {port} -M -U -v {dir}/{NAME}.torrent"
                )
            } else {
                let daemon = port + 1;
                format!(
                    "printf '%s' '{{\"file\": 1, \"format\": 1}}{{\"listen_ports\": [{port}, {port}], \
                     \"random_port\": false, \"dht\": false, \"lsd\": false, \"upnp\": false, \"natpmp\": false, \
                     \"utpex\": false, \"daemon_port\": {daemon}, \"allow_remote\": false, \
                     \"new_release_check\": false, \"download_location\": \"{dir}\", \"add_paused\": false}}' \
                     > {dir}/cfg/core.conf; \
                     deluged --version 2>&1 | head -2; \
                     timeout 900 deluged -c {dir}/cfg -p {daemon} -d -L warning & D=$!; \
                     i=0; until deluge-console -c {dir}/cfg 'connect 127.0.0.1:{daemon}; info' >/dev/null 2>&1; do \
                       i=$((i+1)); [ $i -gt 100 ] && echo 'deluged did not start' && exit 1; sleep 0.2; done; \
                     deluge-console -c {dir}/cfg 'connect 127.0.0.1:{daemon}; add -p {dir} {dir}/{NAME}.torrent'; \
                     wait $D"
                )
            };
            let child = shell(opts, &format!("{prepare}; {run}"))
                .stdin(Stdio::null())
                .stdout(log_file)
                .stderr(err_file)
                .spawn()
                .map_err(|e| format!("cannot start a shell: {e}"))?;
            Ok(Seed {
                child,
                addr: SocketAddr::new(host, port),
                // The client's path must not appear literally in this command line, or `pkill -f`
                // kills the cleanup shell too: `[/]` in the pattern, and `$d` for `rm`.
                kill: Some(format!(
                    "d=/tmp/{NAME}-{pid}; pkill -f '[/]tmp/{NAME}-{pid}/{key}'; sleep 1; \
                     rm -rf \"$d/{key}\"; rmdir \"$d\" 2>/dev/null; true",
                    pid = std::process::id(),
                    key = client.key()
                )),
                log,
            })
        }
    }
}

fn stop_seed(mut s: Seed, opts: &Options) {
    if let Some(kill) = &s.kill {
        let _ = shell(opts, kill).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = s.child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = s.child.kill();
    let _ = s.child.wait();
}

// --- Readiness probe ----------------------------------------------------------------------------

/// Handshakes with `addr` and counts the pieces it announces. Returns the remote peer-id and
/// whether it has all `num_pieces`.
fn probe(addr: SocketAddr, info_hash: &[u8; 20], num_pieces: usize) -> std::io::Result<([u8; 20], bool)> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    s.set_read_timeout(Some(Duration::from_millis(1500)))?;
    let mut hs = vec![19u8];
    hs.extend_from_slice(b"BitTorrent protocol");
    hs.extend_from_slice(&[0; 8]);
    hs.extend_from_slice(info_hash);
    hs.extend_from_slice(b"-LS0001-interopprobe");
    s.write_all(&hs)?;
    let mut reply = [0u8; 68];
    s.read_exact(&mut reply)?;
    if &reply[28..48] != info_hash {
        return Err(std::io::Error::other("wrong info-hash in the reply"));
    }
    let peer_id: [u8; 20] = reply[48..68].try_into().expect("20 bytes");
    let mut have = vec![false; num_pieces];
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline && have.iter().any(|h| !h) {
        let mut len = [0u8; 4];
        if s.read_exact(&mut len).is_err() {
            break;
        }
        let n = u32::from_be_bytes(len) as usize;
        if n > 1 << 20 {
            break;
        }
        let mut msg = vec![0u8; n];
        if s.read_exact(&mut msg).is_err() {
            break;
        }
        match msg.first() {
            Some(5) => {
                for (i, h) in have.iter_mut().enumerate() {
                    *h |= msg.get(1 + i / 8).is_some_and(|b| b & (0x80 >> (i % 8)) != 0);
                }
            }
            Some(4) if n == 5 => {
                let i = u32::from_be_bytes(msg[1..5].try_into().expect("4 bytes")) as usize;
                if let Some(h) = have.get_mut(i) {
                    *h = true;
                }
            }
            _ => {}
        }
    }
    Ok((peer_id, have.iter().all(|&h| h)))
}

fn wait_until_seeding(seed: &mut Seed, f: &Fixture) -> Result<[u8; 20], String> {
    let deadline = Instant::now() + SEED_READY_LIMIT;
    let mut last = String::from("no answer");
    while Instant::now() < deadline {
        if let Ok(Some(status)) = seed.child.try_wait() {
            return Err(format!("the client exited ({status}); see {}", seed.log.display()));
        }
        match probe(seed.addr, &f.info_hash, f.num_pieces) {
            Ok((id, true)) => return Ok(id),
            Ok((_, false)) => last = "connected, not seeding yet".into(),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!("not seeding after {SEED_READY_LIMIT:?} ({last}); see {}", seed.log.display()))
}

// --- Leech --------------------------------------------------------------------------------------

fn wait<T>(h: &SessionHandle, limit: Duration, mut f: impl FnMut(&Alert) -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + limit;
    let mut alerts = Vec::new();
    let mut next_update = Instant::now();
    while Instant::now() < deadline {
        if Instant::now() >= next_update {
            h.post_torrent_updates();
            next_update += Duration::from_millis(200);
        }
        std::thread::sleep(Duration::from_millis(5));
        h.pop_alerts(&mut alerts);
        for a in &alerts {
            if let Some(v) = f(a) {
                return Some(v);
            }
        }
    }
    None
}

fn status(a: &Alert, id: TorrentId) -> Option<TorrentStatus> {
    match a {
        Alert::StateUpdate(v) => v.iter().find(|s| s.id == id).cloned(),
        _ => None,
    }
}

struct Transfer {
    elapsed: Duration,
    /// When the first payload showed up in a status update (200 ms resolution). Some clients
    /// unchoke only on their periodic rechoke: Transmission's runs every 10 s.
    first_data: Option<Duration>,
    status: TorrentStatus,
}

fn download(f: &Fixture, from: SocketAddr, dir: &Path) -> Result<Transfer, String> {
    let settings = Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        enable_dht: false,
        enable_lsd: false,
        ..Settings::default()
    };
    let session = Session::start(SessionParams { settings, state_dir: None }).map_err(|e| e.to_string())?;
    let h = session.handle();
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    h.add_torrent(p);
    let id = wait(&h, Duration::from_secs(5), |a| match a {
        Alert::TorrentAdded { id, .. } => Some(Ok(*id)),
        Alert::AddTorrentFailed { error, .. } => Some(Err(error.to_string())),
        _ => None,
    })
    .ok_or("the torrent was not added")??;
    wait(&h, Duration::from_secs(10), |a| status(a, id).filter(|s| s.state == TorrentState::Downloading))
        .ok_or("the torrent did not start downloading")?;
    let t0 = Instant::now();
    h.torrent(id).connect_peer(from);
    let mut errors = Vec::new();
    // LT_INTEROP_TRACE=1 prints progress about every 200 ms, to see where a transfer stalls.
    let trace = std::env::var_os("LT_INTEROP_TRACE").is_some();
    let mut first_data = None;
    let finished = wait(&h, DOWNLOAD_LIMIT, |a| match a {
        Alert::TorrentFinished { id: i } if *i == id => Some(()),
        Alert::TorrentError { error, .. } => {
            errors.push(error.to_string());
            None
        }
        _ => {
            let st = status(a, id)?;
            if st.total_done > 0 && first_data.is_none() {
                first_data = Some(t0.elapsed());
            }
            if trace {
                eprintln!(
                    "  {:>6} ms  {:>9} bytes  {:>7.2} MB/s  peers {}",
                    t0.elapsed().as_millis(),
                    st.total_done,
                    st.download_payload_rate as f64 / 1e6,
                    st.num_peers
                );
            }
            None
        }
    });
    let elapsed = t0.elapsed();
    let st = wait(&h, Duration::from_secs(5), |a| status(a, id)).ok_or("no final status")?;
    session.shutdown();
    if finished.is_none() {
        return Err(format!(
            "did not finish within {DOWNLOAD_LIMIT:?}: {} of {} bytes, {} peers{}",
            st.total_done,
            st.total,
            st.num_peers,
            if errors.is_empty() { String::new() } else { format!(", errors: {errors:?}") }
        ));
    }
    Ok(Transfer { elapsed, first_data, status: st })
}

fn same_files(f: &Fixture, dir: &Path) -> Result<(), String> {
    for name in &f.files {
        let want = std::fs::read(f.work.join("seed").join(NAME).join(name)).map_err(|e| e.to_string())?;
        let got = std::fs::read(dir.join(NAME).join(name)).map_err(|e| format!("{name}: {e}"))?;
        if got != want {
            return Err(format!("{name} differs from the original"));
        }
    }
    Ok(())
}

fn peer_id_text(id: &[u8; 20]) -> String {
    id[..8].iter().map(|&b| if b.is_ascii_graphic() { b as char } else { '.' }).collect()
}

// --- Driver -------------------------------------------------------------------------------------

fn run_one(client: Client, f: &Fixture, opts: &Options) -> Result<(String, Transfer), String> {
    let mut seed = start_seed(client, f, opts)?;
    let result = wait_until_seeding(&mut seed, f).and_then(|peer_id| {
        let dir = f.work.join(format!("leech-{}", client.key()));
        let t = download(f, seed.addr, &dir)?;
        same_files(f, &dir)?;
        Ok((peer_id_text(&peer_id), t))
    });
    stop_seed(seed, opts);
    result
}

pub fn run(opts: &Options) -> ExitCode {
    let work = std::env::temp_dir().join(format!("{NAME}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let f = match std::fs::create_dir_all(&work).and_then(|_| fixture(&work, opts.mb)) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot write the test torrent: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("torrent_bytes={}", f.total);
    println!("torrent_pieces={}", f.num_pieces);
    let mut summary = Vec::new();
    let mut failed = false;
    for &client in &opts.clients {
        let k = client.key();
        match run_one(client, &f, opts) {
            Ok((peer_id, t)) => {
                let st = &t.status;
                println!("{k}_result=ok");
                println!("{k}_peer_id={peer_id}");
                println!("{k}_download_ms={:.0}", t.elapsed.as_secs_f64() * 1e3);
                if let Some(d) = t.first_data {
                    println!("{k}_first_data_ms={}", d.as_millis());
                }
                println!("{k}_payload_bytes={}", st.total_payload_download);
                println!("{k}_failed_bytes={}", st.total_failed_bytes);
                println!("{k}_redundant_bytes={}", st.total_redundant_bytes);
                let first = t.first_data.map_or("-".into(), |d| format!("{:.1} s", d.as_secs_f64()));
                summary.push(format!(
                    "{k:<13} ok   {peer_id}  {:>6.2} s (first data after {first}), failed {} B, redundant {} B",
                    t.elapsed.as_secs_f64(),
                    st.total_failed_bytes,
                    st.total_redundant_bytes
                ));
            }
            Err(e) => {
                failed = true;
                println!("{k}_result=failed");
                summary.push(format!("{k:<13} FAIL {e}"));
            }
        }
    }
    println!();
    println!("PaTorrent downloading {} MiB ({} pieces) from:", f.total >> 20, f.num_pieces);
    for line in &summary {
        println!("  {line}");
    }
    if opts.keep || failed {
        println!("work directory kept: {}", work.display());
    } else {
        let _ = std::fs::remove_dir_all(&work);
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}
