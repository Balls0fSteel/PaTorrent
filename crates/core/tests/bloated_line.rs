//! Keep ping low against a real bloated line (D49, `docs/LOWPING.md`). `tools/bloated-line/line.sh`
//! builds the line in Linux network namespaces (a home PC, a modem whose upload is a slow link
//! with a deep buffer, an ISP router whose download to the modem is another, and the internet
//! with the peers behind it) and runs this test twice: the swarm on the internet side and the
//! client at home. Ignored: it needs that network.
//!
//! The environment says what to do: `LINE_ROLE` (`swarm` or `client`), `LINE_DIR` (`upload`: the
//! client seeds to the swarm's leeches; `download`: the swarm's seeds send to the client),
//! `LINE_PROTO` (`tcp`, `utp` or `both`), `LINE_PEERS` (swarm sessions), `LINE_MB` (the torrent's
//! size), `LINE_SWARM` and `LINE_HOME` (the two sides' addresses), `LINE_PING` (the router the
//! client pings on its own, to show the queue) and `LINE_PHASES` (the client's run: `off:60,on:180`
//! is a minute without keep ping low, then three with it).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, Session, SessionHandle, SessionParams, Settings, TorrentFlags, TorrentId,
    TorrentSource, TorrentState,
};

const PIECE: usize = 256 * 1024;
const PORT: u16 = 7000;

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

/// One piece of the torrent's data: the same bytes on both sides without sending them around.
fn piece(i: usize, len: usize) -> Vec<u8> {
    let mut x = (0x9e3779b9u32 ^ (i as u32).wrapping_mul(0x85eb_ca6b)) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// The torrent of one `mb`-mebibyte file; writes the file into `dir` when given.
fn torrent(mb: usize, dir: Option<&Path>) -> Vec<u8> {
    let total = mb * 1024 * 1024;
    let mut out = dir.map(|d| {
        std::fs::create_dir_all(d).unwrap();
        std::io::BufWriter::new(std::fs::File::create(d.join("line.bin")).unwrap())
    });
    let mut pieces = Vec::new();
    for i in 0..total.div_ceil(PIECE) {
        let p = piece(i, PIECE.min(total - i * PIECE));
        pieces.extend_from_slice(&Sha1::digest(&p).0);
        if let Some(w) = out.as_mut() {
            w.write_all(&p).unwrap();
        }
    }
    let mut info = Entry::new_dict();
    *info.key("length") = Entry::Int(total as i64);
    *info.key("name") = Entry::Bytes(b"line.bin".to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    t.bencode()
}

fn settings(addr: IpAddr, port: u16, proto: &str) -> Settings {
    let mut s = Settings {
        listen_interfaces: SocketAddr::new(addr, port).to_string(),
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        ..Settings::default()
    };
    match proto {
        "tcp" => {
            s.enable_incoming_utp = false;
            s.enable_outgoing_utp = false;
        }
        "utp" => {
            s.enable_incoming_tcp = false;
            s.enable_outgoing_tcp = false;
        }
        _ => {}
    }
    s
}

fn session(addr: IpAddr, port: u16, proto: &str) -> Session {
    Session::start(SessionParams {
        settings: settings(addr, port, proto),
        state_dir: None,
        ..Default::default()
    })
    .expect("session starts")
}

fn add(h: &SessionHandle, t: &[u8], dir: &Path) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(t.to_vec()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    h.add_torrent(p);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut alerts = Vec::new();
    while Instant::now() < deadline {
        h.pop_alerts(&mut alerts);
        for a in alerts.drain(..) {
            match a {
                Alert::TorrentAdded { id, .. } => return id,
                Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the torrent was not added");
}

fn wait_state(h: &SessionHandle, id: TorrentId, state: TorrentState) {
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut alerts = Vec::new();
    while Instant::now() < deadline {
        h.post_torrent_updates();
        std::thread::sleep(Duration::from_millis(200));
        h.pop_alerts(&mut alerts);
        for a in alerts.drain(..) {
            if let Alert::StateUpdate(v) = a {
                if v.iter().any(|s| s.id == id && s.state == state) {
                    return;
                }
            }
        }
    }
    panic!("not {state:?} in time");
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bloated-line-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The swarm: `LINE_PEERS` sessions on consecutive ports from [`PORT`], seeds for a download
/// and leeches for an upload, until killed.
fn swarm() {
    let addr: IpAddr = env("LINE_SWARM", "203.0.113.10").parse().unwrap();
    let peers: u16 = env("LINE_PEERS", "4").parse().unwrap();
    let mb: usize = env("LINE_MB", "320").parse().unwrap();
    let seeding = env("LINE_DIR", "upload") == "download";
    let proto = env("LINE_PROTO", "tcp");
    let data = scratch("swarm");
    let t = torrent(mb, seeding.then_some(data.as_path()));
    let mut sessions = Vec::new();
    for i in 0..peers {
        let s = session(addr, PORT + i, &proto);
        let dir = if seeding { data.clone() } else { data.join(i.to_string()) };
        let id = add(&s.handle(), &t, &dir);
        if seeding {
            wait_state(&s.handle(), id, TorrentState::Seeding);
        }
        sessions.push(s);
    }
    println!("swarm ready: {peers} {} on {addr}", if seeding { "seeds" } else { "leeches" });
    loop {
        // the alerts would pile up otherwise
        for s in &sessions {
            s.handle().pop_alerts(&mut Vec::new());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// `ping -i 0.2` to `to` in the background: the round trips of the last second.
fn pinger(to: &str) -> Arc<Mutex<Vec<f64>>> {
    let out = Arc::new(Mutex::new(Vec::new()));
    let mut child = Command::new("ping")
        .args(["-n", "-i", "0.2", "-W", "2", to])
        .stdout(Stdio::piped())
        .spawn()
        .expect("ping runs");
    let stdout = child.stdout.take().unwrap();
    let o = out.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(ms) = line.split("time=").nth(1).and_then(|r| r.split(' ').next()) {
                if let Ok(ms) = ms.parse() {
                    o.lock().unwrap().push(ms);
                }
            }
        }
        let _ = child.wait();
    });
    out
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    (!v.is_empty()).then(|| v[v.len() / 2])
}

fn kb(b: u32) -> String {
    if b == 0 { "-".into() } else { format!("{}", b.div_ceil(1000)) }
}

/// The client: one session at `LINE_HOME`, connected to the swarm, through the phases.
fn client() {
    let home: IpAddr = env("LINE_HOME", "192.168.77.2").parse().unwrap();
    let swarm: IpAddr = env("LINE_SWARM", "203.0.113.10").parse().unwrap();
    let peers: u16 = env("LINE_PEERS", "4").parse().unwrap();
    let mb: usize = env("LINE_MB", "320").parse().unwrap();
    let seeding = env("LINE_DIR", "upload") == "upload";
    let proto = env("LINE_PROTO", "tcp");
    let phases: Vec<(bool, u64)> = env("LINE_PHASES", "off:60,on:180")
        .split(',')
        .map(|p| {
            let (mode, secs) = p.split_once(':').expect("mode:secs");
            (mode == "on", secs.parse().expect("seconds"))
        })
        .collect();
    let dir = scratch("client");
    let t = torrent(mb, seeding.then_some(dir.as_path()));
    let ses = session(home, PORT, &proto);
    let h = ses.handle();
    let id = add(&h, &t, &dir);
    wait_state(&h, id, if seeding { TorrentState::Seeding } else { TorrentState::Downloading });
    let pings = pinger(&env("LINE_PING", "84.1.1.1"));
    for i in 0..peers {
        h.torrent(id).connect_peer(SocketAddr::new(swarm, PORT + i));
    }
    println!("phase,t,up kB/s,down kB/s,cap up,cap down,ping ms,peers,sent kB/s,recv kB/s,wire up,wire down");
    // what the session counts against what leaves the home side (`ip netns exec` mounts this
    // namespace's /sys)
    let wire = |what: &str| -> u64 {
        std::fs::read_to_string(format!("/sys/class/net/h0/statistics/{what}"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    };
    let mut last = (0u64, 0u64, wire("tx_bytes"), wire("rx_bytes"));
    let base = settings(home, PORT, &proto);
    let mut alerts = Vec::new();
    let mut summary = Vec::new();
    let started = Instant::now();
    for (on, secs) in phases {
        h.apply_settings(Settings { auto_rate_limit: on, ..base.clone() });
        let name = if on { "on" } else { "off" };
        let mut rows: Vec<(u32, u32, Option<f64>)> = Vec::new();
        let until = Instant::now() + Duration::from_secs(secs);
        let mut window: VecDeque<f64> = VecDeque::new();
        while Instant::now() < until {
            std::thread::sleep(Duration::from_secs(1));
            h.post_session_stats();
            std::thread::sleep(Duration::from_millis(20));
            h.pop_alerts(&mut alerts);
            let second: Vec<f64> = std::mem::take(&mut *pings.lock().unwrap());
            window.extend(second.iter().copied());
            let ping = median(second);
            for a in alerts.drain(..) {
                match a {
                    Alert::Log { id: None, message } if message.starts_with("Keep ping low: ") => {
                        println!("# {:>4} {message}", started.elapsed().as_secs())
                    }
                    Alert::SessionStats(st) => {
                        rows.push((st.upload_rate, st.download_rate, ping));
                        let now = (st.sent_bytes, st.recv_bytes, wire("tx_bytes"), wire("rx_bytes"));
                        let d = |a: u64, b: u64| (a.saturating_sub(b) / 1000).to_string();
                        let deltas = [d(now.0, last.0), d(now.1, last.1), d(now.2, last.2), d(now.3, last.3)]
                            .join(",");
                        last = now;
                        println!(
                            "{name},{},{},{},{},{},{},{},{deltas}",
                            started.elapsed().as_secs(),
                            kb(st.upload_rate),
                            kb(st.download_rate),
                            kb(st.auto_upload_limit),
                            kb(st.auto_download_limit),
                            ping.map_or("-".into(), |p| format!("{p:.0}")),
                            st.num_peers_connected,
                        );
                    }
                    _ => {}
                }
            }
        }
        // the phase's settled part: its last two thirds
        let settled = &rows[rows.len() / 3..];
        let n = settled.len().max(1) as f64;
        let up = settled.iter().map(|r| r.0 as f64).sum::<f64>() / n / 1000.0;
        let down = settled.iter().map(|r| r.1 as f64).sum::<f64>() / n / 1000.0;
        let mut p: Vec<f64> = settled.iter().filter_map(|r| r.2).collect();
        p.sort_by(f64::total_cmp);
        let at = |q: f64| p.get(((p.len() as f64 - 1.0) * q) as usize).copied().unwrap_or(f64::NAN);
        summary.push(format!(
            "{name:>3} {secs:>4} s: up {up:.0} kB/s, down {down:.0} kB/s, ping median {:.0} ms, 90% {:.0} ms",
            at(0.5),
            at(0.9)
        ));
    }
    println!("--- summary (each phase's last two thirds)");
    for s in summary {
        println!("{s}");
    }
    ses.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
#[ignore = "needs the network namespaces of tools/bloated-line/line.sh"]
fn bloated_line() {
    match env("LINE_ROLE", "").as_str() {
        "swarm" => swarm(),
        "client" => client(),
        // `--ignored` runs elsewhere (CI's slow tests) have no such network
        "" => println!("LINE_ROLE is not set: run tools/bloated-line/line.sh"),
        r => panic!("LINE_ROLE is {r:?}: run tools/bloated-line/line.sh"),
    }
}
