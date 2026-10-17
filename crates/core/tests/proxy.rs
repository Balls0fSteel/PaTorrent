//! Loopback tests for proxies, after libtorrent's test_socks5.cpp, test_web_seed_socks*.cpp and
//! the proxy runs of its simulator: tiny SOCKS4, SOCKS5 (with username / password and UDP
//! ASSOCIATE) and HTTP (CONNECT and plain proxy requests) servers in the test stand between the
//! session and a seed, trackers, a web seed and a DHT node. Cases: downloads through each kind
//! of proxy with no TCP listener and no direct connection; an HTTP proxy asked for a tracker and
//! a web seed by absolute URL; a tracker host name the SOCKS5 proxy resolves; a wrong password;
//! a UDP tracker, a DHT node and a uTP download through the SOCKS5 UDP associate.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;
use patorrent_core::{
    AddTorrentParams, Alert, ProxyOperation, ProxyType, Session, SessionHandle, SessionParams, Settings,
    TorrentFlags, TorrentId, TorrentSource, TorrentState, TorrentStatus,
};

const PIECE: usize = 32 * 1024;

// --- Torrents and sessions ---------------------------------------------------------------------

struct Fixture {
    torrent: Vec<u8>,
    data: Vec<u8>,
}

const NAME: &str = "proxy-test.bin";

/// A single-file torrent with an optional tracker and web seed.
fn fixture(announce: Option<&str>, url_seed: Option<&str>) -> Fixture {
    let mut x = 0x2545_f491u32;
    let data: Vec<u8> = (0..700_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect();
    let mut pieces = Vec::new();
    for c in data.chunks(PIECE) {
        pieces.extend_from_slice(&Sha1::digest(c).0);
    }
    let mut info = Entry::new_dict();
    *info.key("length") = Entry::Int(data.len() as i64);
    *info.key("name") = Entry::Bytes(NAME.as_bytes().to_vec());
    *info.key("piece length") = Entry::Int(PIECE as i64);
    *info.key("pieces") = Entry::Bytes(pieces);
    let mut t = Entry::new_dict();
    if let Some(a) = announce {
        *t.key("announce") = Entry::Bytes(a.as_bytes().to_vec());
    }
    if let Some(u) = url_seed {
        *t.key("url-list") = Entry::Bytes(u.as_bytes().to_vec());
    }
    *t.key("info") = info;
    Fixture { torrent: t.bencode(), data }
}

fn temp_dir(name: &str) -> PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u8(1);
    let d =
        std::env::temp_dir().join(format!("lt-proxy-{name}-{}-{}", std::process::id(), h.finish() as u32));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn settings() -> Settings {
    Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        tick_interval: 100,
        aio_threads: 2,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        ..Settings::default()
    }
}

/// Settings that send everything through `proxy`.
fn proxied(kind: ProxyType, proxy: SocketAddr, user: &str, pass: &str) -> Settings {
    Settings {
        proxy_type: kind,
        proxy_hostname: proxy.ip().to_string(),
        proxy_port: proxy.port(),
        proxy_username: user.into(),
        proxy_password: pass.into(),
        ..settings()
    }
}

struct Watch {
    h: SessionHandle,
    backlog: VecDeque<Alert>,
    seen: Vec<String>,
}

impl Watch {
    fn new(h: SessionHandle) -> Watch {
        Watch { h, backlog: VecDeque::new(), seen: Vec::new() }
    }
}

fn wait<T>(w: &mut Watch, secs: u64, what: &str, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut alerts = Vec::new();
    loop {
        while let Some(a) = w.backlog.pop_front() {
            if !matches!(a, Alert::StateUpdate(_)) {
                w.seen.push(a.message());
            }
            if let Some(v) = f(&a) {
                return v;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        w.h.post_torrent_updates();
        std::thread::sleep(Duration::from_millis(30));
        w.h.pop_alerts(&mut alerts);
        w.backlog.extend(alerts.drain(..));
    }
    panic!("timed out waiting for {what}; alerts: {:#?}", w.seen);
}

fn add(w: &mut Watch, f: &Fixture, dir: &Path) -> TorrentId {
    let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    p.flags = TorrentFlags::empty();
    w.h.add_torrent(p);
    wait(w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        Alert::AddTorrentFailed { error, .. } => panic!("add failed: {error}"),
        _ => None,
    })
}

fn status_of(a: &Alert, id: TorrentId) -> Option<TorrentStatus> {
    match a {
        Alert::StateUpdate(v) => v.iter().find(|s| s.id == id).cloned(),
        _ => None,
    }
}

fn finished(w: &mut Watch, id: TorrentId, secs: u64) {
    wait(w, secs, "finished", |a| matches!(a, Alert::TorrentFinished { id: i } if *i == id).then_some(()));
}

/// A session seeding the fixture on 127.0.0.1, with incoming uTP. Its listen port.
fn seeder(f: &Fixture) -> (Session, PathBuf, u16) {
    let dir = temp_dir("seed");
    std::fs::write(dir.join(NAME), &f.data).unwrap();
    let s = Session::start(SessionParams { settings: settings(), state_dir: None }).unwrap();
    let mut w = Watch::new(s.handle());
    let port = wait(&mut w, 5, "listen", |a| match a {
        Alert::ListenSucceeded { address, .. } => Some(address.port()),
        _ => None,
    });
    let id = add(&mut w, f, &dir);
    wait(&mut w, 20, "seeding", |a| status_of(a, id).filter(|s| s.state == TorrentState::Seeding));
    (s, dir, port)
}

/// A proxied session: the alerts until its listen sockets are up must show no TCP listener.
fn leech(s: Settings) -> (Session, Watch) {
    let session = Session::start(SessionParams { settings: s, state_dir: None }).unwrap();
    let mut w = Watch::new(session.handle());
    wait(&mut w, 5, "UDP listen socket", |a| match a {
        Alert::ListenSucceeded { protocol, .. } if format!("{protocol:?}") == "Udp" => Some(()),
        Alert::ListenSucceeded { address, .. } => panic!("a TCP listener at {address} while proxied"),
        _ => None,
    });
    (session, w)
}

fn check_download(f: &Fixture, dir: &Path) {
    let got = std::fs::read(dir.join(NAME)).unwrap();
    assert!(got == f.data, "the downloaded file differs");
}

// --- The proxy server --------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Socks4,
    Socks5,
    Http,
}

/// A SOCKS4 / SOCKS5 / HTTP proxy on 127.0.0.1. `log` records what it was asked for:
/// `CONNECT target`, `GET absolute-url`, `ASSOCIATE`, `UDP target` per relayed datagram,
/// `AUTH ok|failed`.
struct Proxy {
    addr: SocketAddr,
    log: Arc<Mutex<Vec<String>>>,
    /// The UDP relay of the last associate.
    relay: Arc<Mutex<Option<SocketAddr>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

impl Proxy {
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn saw(&self, prefix: &str) -> bool {
        self.log().iter().any(|l| l.starts_with(prefix))
    }
}

fn proxy(kind: Kind, auth: Option<(&str, &str)>) -> Proxy {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let relay = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let auth = auth.map(|(u, p)| (u.to_owned(), p.to_owned()));
    let (log2, relay2, stop2) = (log.clone(), relay.clone(), stop.clone());
    std::thread::spawn(move || {
        for c in l.incoming() {
            if stop2.load(Ordering::SeqCst) {
                break;
            }
            let Ok(c) = c else { continue };
            let (log, relay, auth) = (log2.clone(), relay2.clone(), auth.clone());
            std::thread::spawn(move || {
                let _ = c.set_read_timeout(Some(Duration::from_secs(60)));
                let _ = match kind {
                    Kind::Socks4 => socks4(c, &log),
                    Kind::Socks5 => socks5(c, &log, &relay, auth.as_ref()),
                    Kind::Http => http(c, &log, auth.as_ref()),
                };
            });
        }
    });
    Proxy { addr, log, relay, stop }
}

fn note(log: &Mutex<Vec<String>>, s: String) {
    log.lock().unwrap().push(s);
}

fn read_exact(c: &mut TcpStream, n: usize) -> std::io::Result<Vec<u8>> {
    let mut b = vec![0; n];
    c.read_exact(&mut b)?;
    Ok(b)
}

/// IPv4 first, like the session's resolver, so `localhost` is the test servers' 127.0.0.1.
fn resolve(host: &str, port: u16) -> SocketAddr {
    let mut v: Vec<SocketAddr> = (host, port).to_socket_addrs().unwrap().collect();
    v.sort_by_key(|a| a.is_ipv6());
    v[0]
}

/// Copies both ways until either side closes.
fn pipe(a: TcpStream, b: TcpStream) {
    let (mut a2, mut b2) = (a.try_clone().unwrap(), b.try_clone().unwrap());
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut b2, &mut a2);
        let _ = a2.shutdown(Shutdown::Both);
    });
    let (mut a, mut b) = (a, b);
    let _ = std::io::copy(&mut a, &mut b);
    let _ = b.shutdown(Shutdown::Both);
    let _ = t.join();
}

fn socks4(mut c: TcpStream, log: &Mutex<Vec<String>>) -> std::io::Result<()> {
    let head = read_exact(&mut c, 8)?;
    assert_eq!(head[..2], [4, 1], "SOCKS4 CONNECT");
    let port = u16::from_be_bytes([head[2], head[3]]);
    let ip = Ipv4Addr::new(head[4], head[5], head[6], head[7]);
    let mut user = Vec::new();
    loop {
        let b = read_exact(&mut c, 1)?[0];
        if b == 0 {
            break;
        }
        user.push(b);
    }
    let target = SocketAddr::new(ip.into(), port);
    note(log, format!("CONNECT {target} user={}", String::from_utf8_lossy(&user)));
    match TcpStream::connect(target) {
        Ok(t) => {
            c.write_all(&[0, 90, 0, 0, 0, 0, 0, 0])?;
            pipe(c, t);
        }
        Err(_) => c.write_all(&[0, 91, 0, 0, 0, 0, 0, 0])?,
    }
    Ok(())
}

fn socks5(
    mut c: TcpStream,
    log: &Mutex<Vec<String>>,
    relay: &Mutex<Option<SocketAddr>>,
    auth: Option<&(String, String)>,
) -> std::io::Result<()> {
    let head = read_exact(&mut c, 2)?;
    assert_eq!(head[0], 5);
    let methods = read_exact(&mut c, head[1] as usize)?;
    match auth {
        None => c.write_all(&[5, 0])?,
        Some((user, pass)) => {
            assert!(methods.contains(&2), "username / password offered: {methods:?}");
            c.write_all(&[5, 2])?;
            let v = read_exact(&mut c, 2)?;
            assert_eq!(v[0], 1);
            let u = read_exact(&mut c, v[1] as usize)?;
            let plen = read_exact(&mut c, 1)?[0];
            let p = read_exact(&mut c, plen as usize)?;
            if u != user.as_bytes() || p != pass.as_bytes() {
                note(log, "AUTH failed".into());
                return c.write_all(&[1, 1]);
            }
            note(log, "AUTH ok".into());
            c.write_all(&[1, 0])?;
        }
    }
    let req = read_exact(&mut c, 4)?;
    assert_eq!(req[0], 5);
    let target = match req[3] {
        1 => {
            let a = read_exact(&mut c, 6)?;
            let ip = Ipv4Addr::new(a[0], a[1], a[2], a[3]);
            format!("{ip}:{}", u16::from_be_bytes([a[4], a[5]]))
        }
        4 => {
            let a = read_exact(&mut c, 18)?;
            let ip: [u8; 16] = a[..16].try_into().unwrap();
            SocketAddr::new(IpAddr::from(ip), u16::from_be_bytes([a[16], a[17]])).to_string()
        }
        3 => {
            let n = read_exact(&mut c, 1)?[0] as usize;
            let a = read_exact(&mut c, n + 2)?;
            format!("{}:{}", String::from_utf8_lossy(&a[..n]), u16::from_be_bytes([a[n], a[n + 1]]))
        }
        t => panic!("address type {t}"),
    };
    match req[1] {
        1 => {
            note(log, format!("CONNECT {target}"));
            let (host, port) = target.rsplit_once(':').unwrap();
            let host = host.trim_start_matches('[').trim_end_matches(']');
            match TcpStream::connect(resolve(host, port.parse().unwrap())) {
                Ok(t) => {
                    c.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])?;
                    pipe(c, t);
                }
                Err(_) => c.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0])?,
            }
        }
        3 => {
            note(log, format!("ASSOCIATE {target}"));
            let u = UdpSocket::bind("127.0.0.1:0")?;
            let r = u.local_addr()?;
            *relay.lock().unwrap() = Some(r);
            let mut reply = vec![5, 0, 0, 1, 127, 0, 0, 1];
            reply.extend_from_slice(&r.port().to_be_bytes());
            c.write_all(&reply)?;
            udp_relay(c, u, log);
        }
        cmd => panic!("SOCKS5 command {cmd}"),
    }
    Ok(())
}

/// Relays datagrams for one associate until its TCP connection closes. The client is whoever
/// sends the first datagram; everyone else is a target.
fn udp_relay(mut control: TcpStream, u: UdpSocket, log: &Mutex<Vec<String>>) {
    u.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    control.set_nonblocking(true).unwrap();
    let mut client: Option<SocketAddr> = None;
    let mut buf = [0u8; 65536];
    loop {
        let mut b = [0u8; 1];
        match control.read(&mut b) {
            Ok(_) => return,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return,
        }
        let Ok((n, from)) = u.recv_from(&mut buf) else { continue };
        if client.is_none() || client == Some(from) {
            client = Some(from);
            let p = &buf[..n];
            if n < 10 || p[2] != 0 {
                continue;
            }
            let (to, data) = match p[3] {
                1 => {
                    let ip = Ipv4Addr::new(p[4], p[5], p[6], p[7]);
                    (SocketAddr::new(ip.into(), u16::from_be_bytes([p[8], p[9]])), &p[10..])
                }
                3 => {
                    let l = p[4] as usize;
                    let host = String::from_utf8_lossy(&p[5..5 + l]).into_owned();
                    let port = u16::from_be_bytes([p[5 + l], p[6 + l]]);
                    (resolve(&host, port), &p[7 + l..])
                }
                _ => continue,
            };
            note(log, format!("UDP {to}"));
            let _ = u.send_to(data, to);
        } else if let (Some(c), SocketAddr::V4(f)) = (client, from) {
            let mut out = vec![0, 0, 0, 1];
            out.extend_from_slice(&f.ip().octets());
            out.extend_from_slice(&f.port().to_be_bytes());
            out.extend_from_slice(&buf[..n]);
            let _ = u.send_to(&out, c);
        }
    }
}

/// Reads one request head (through the blank line).
fn read_head(c: &mut TcpStream) -> std::io::Result<String> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let b = read_exact(c, 1)?;
        head.extend_from_slice(&b);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

fn http(mut c: TcpStream, log: &Mutex<Vec<String>>, auth: Option<&(String, String)>) -> std::io::Result<()> {
    let head = read_head(&mut c)?;
    let line = head.lines().next().unwrap_or_default().to_owned();
    if let Some((u, p)) = auth {
        let want = format!("Proxy-Authorization: Basic {}", base64(format!("{u}:{p}").as_bytes()));
        if !head.lines().any(|l| l == want) {
            note(log, format!("AUTH failed {line}"));
            return c.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n");
        }
    }
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    if method == "CONNECT" {
        note(log, format!("CONNECT {target}"));
        let (host, port) = target.rsplit_once(':').unwrap();
        let host = host.trim_start_matches('[').trim_end_matches(']');
        match TcpStream::connect(resolve(host, port.parse().unwrap())) {
            Ok(t) => {
                c.write_all(b"HTTP/1.0 200 Connection established\r\n\r\n")?;
                pipe(c, t);
            }
            Err(_) => c.write_all(b"HTTP/1.0 502 Bad Gateway\r\n\r\n")?,
        }
        return Ok(());
    }
    // a plain proxy request: the origin gets each request with an origin-form target, and its
    // answers flow back as they come
    let origin_of = |t: &str| {
        let rest = t.strip_prefix("http://").expect("an absolute http:// URL");
        let (hostport, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        (hostport.to_owned(), if path.is_empty() { "/".to_owned() } else { path.to_owned() })
    };
    let (hostport, _) = origin_of(target);
    let (host, port) = hostport.rsplit_once(':').unwrap_or((&hostport, "80"));
    let mut origin = TcpStream::connect(resolve(host, port.parse().unwrap()))?;
    let (mut from_origin, mut to_client) = (origin.try_clone()?, c.try_clone()?);
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut from_origin, &mut to_client);
        let _ = to_client.shutdown(Shutdown::Both);
    });
    let mut head = head;
    loop {
        let line = head.lines().next().unwrap_or_default().to_owned();
        let target = line.split(' ').nth(1).unwrap_or_default();
        note(log, format!("GET {target}"));
        let (_, path) = origin_of(target);
        let mut out = format!("GET {path} HTTP/1.1\r\n");
        for l in head.lines().skip(1).filter(|l| !l.is_empty()) {
            if !l.starts_with("Proxy-") {
                out.push_str(l);
                out.push_str("\r\n");
            }
        }
        out.push_str("\r\n");
        origin.write_all(out.as_bytes())?;
        head = match read_head(&mut c) {
            Ok(h) => h,
            Err(_) => break,
        };
    }
    let _ = origin.shutdown(Shutdown::Both);
    Ok(())
}

fn base64(s: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in s.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// --- Trackers and a web seed -------------------------------------------------------------------

/// An HTTP tracker: answers each announce with `peers` and records the query strings.
fn http_tracker(peers: Vec<SocketAddr>) -> (u16, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let queries = Arc::new(Mutex::new(Vec::new()));
    let q2 = queries.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let Ok(head) = read_head(&mut c) else { continue };
            let line = head.lines().next().unwrap_or_default();
            q2.lock().unwrap().push(line.to_owned());
            let mut compact = Vec::new();
            for p in &peers {
                let SocketAddr::V4(a) = p else { continue };
                compact.extend_from_slice(&a.ip().octets());
                compact.extend_from_slice(&a.port().to_be_bytes());
            }
            let mut body = format!("d8:intervali1800e5:peers{}:", compact.len()).into_bytes();
            body.extend_from_slice(&compact);
            body.push(b'e');
            let _ = c.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                    .as_bytes(),
            );
            let _ = c.write_all(&body);
        }
    });
    (port, queries)
}

/// A BEP 15 tracker answering with `peers`; it records where its datagrams came from.
fn udp_tracker(peers: Vec<SocketAddr>) -> (u16, Arc<Mutex<Vec<SocketAddr>>>) {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = s.local_addr().unwrap().port();
    let sources = Arc::new(Mutex::new(Vec::new()));
    let s2 = sources.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        loop {
            let Ok((n, from)) = s.recv_from(&mut buf) else { break };
            s2.lock().unwrap().push(from);
            if n < 16 {
                continue;
            }
            let action = u32::from_be_bytes(buf[8..12].try_into().unwrap());
            let mut out = Vec::new();
            if action == 0 {
                out.extend_from_slice(&0u32.to_be_bytes());
                out.extend_from_slice(&buf[12..16]);
                out.extend_from_slice(&0x1234_5678_9abc_def0u64.to_be_bytes());
            } else if action == 1 && n >= 98 {
                out.extend_from_slice(&1u32.to_be_bytes());
                out.extend_from_slice(&buf[12..16]);
                out.extend_from_slice(&1800u32.to_be_bytes());
                out.extend_from_slice(&0u32.to_be_bytes());
                out.extend_from_slice(&(peers.len() as u32).to_be_bytes());
                for p in &peers {
                    let SocketAddr::V4(a) = p else { continue };
                    out.extend_from_slice(&a.ip().octets());
                    out.extend_from_slice(&a.port().to_be_bytes());
                }
            } else {
                continue;
            }
            let _ = s.send_to(&out, from);
        }
    });
    (port, sources)
}

/// A web server holding the fixture's file, with range requests and keep-alive.
fn web_seed(f: &Fixture) -> (u16, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let (data, hits2) = (Arc::new(f.data.clone()), hits.clone());
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let (data, hits) = (data.clone(), hits2.clone());
            std::thread::spawn(move || {
                while let Ok(head) = read_head(&mut c) {
                    let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
                    assert_eq!(path, format!("/{NAME}"), "an origin-form request");
                    hits.fetch_add(1, Ordering::SeqCst);
                    let range = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Range: bytes="))
                        .and_then(|r| r.split_once('-'))
                        .map(|(a, b)| (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()))
                        .unwrap_or((0, data.len() - 1));
                    let body = &data[range.0..=range.1];
                    let resp = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\n\r\n",
                        range.0,
                        range.1,
                        data.len(),
                        body.len()
                    );
                    if c.write_all(resp.as_bytes()).and_then(|()| c.write_all(body)).is_err() {
                        break;
                    }
                }
            });
        }
    });
    (port, hits)
}

// --- Tests -------------------------------------------------------------------------------------

/// Downloads from a seed the session is told about, through `proxy`, over TCP only.
fn tcp_download(kind: ProxyType, p: &Proxy, user: &str, pass: &str) {
    let f = fixture(None, None);
    let (seed, seed_dir, port) = seeder(&f);
    let seed_addr = SocketAddr::from(([127, 0, 0, 1], port));
    let dir = temp_dir("leech");
    let (leech, mut w) = leech(Settings { enable_outgoing_utp: false, ..proxied(kind, p.addr, user, pass) });
    let id = add(&mut w, &f, &dir);
    leech.handle().torrent(id).connect_peer(seed_addr);
    finished(&mut w, id, 60);
    assert!(p.saw(&format!("CONNECT {seed_addr}")), "the seed was reached through the proxy: {:?}", p.log());
    leech.shutdown();
    seed.shutdown();
    check_download(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn download_through_a_socks5_proxy_with_a_password() {
    let p = proxy(Kind::Socks5, Some(("alice", "s3cret")));
    tcp_download(ProxyType::Socks5Pw, &p, "alice", "s3cret");
    assert!(p.saw("AUTH ok"));
}

#[test]
fn download_through_a_socks4_proxy() {
    let p = proxy(Kind::Socks4, None);
    tcp_download(ProxyType::Socks4, &p, "bob", "");
    assert!(p.log().iter().any(|l| l.ends_with("user=bob")), "{:?}", p.log());
}

#[test]
fn download_through_an_http_connect_proxy() {
    let p = proxy(Kind::Http, Some(("carol", "pw")));
    tcp_download(ProxyType::HttpPw, &p, "carol", "pw");
}

#[test]
fn http_proxy_requests_trackers_and_web_seeds_by_url() {
    let (tport, queries) = http_tracker(Vec::new());
    let (wport, hits) = web_seed(&fixture(None, None));
    let announce = format!("http://127.0.0.1:{tport}/announce");
    let f = fixture(Some(&announce), Some(&format!("http://127.0.0.1:{wport}/{NAME}")));
    let p = proxy(Kind::Http, Some(("dave", "pw")));
    let dir = temp_dir("leech-http");
    let (leech, mut w) = leech(proxied(ProxyType::HttpPw, p.addr, "dave", "pw"));
    let id = add(&mut w, &f, &dir);
    wait(&mut w, 20, "tracker reply", |a| match a {
        Alert::TrackerReply { .. } => Some(()),
        Alert::TrackerError { message, .. } => panic!("tracker error: {message}"),
        _ => None,
    });
    finished(&mut w, id, 60);
    let log = p.log();
    assert!(log.iter().any(|l| l.starts_with(&format!("GET {announce}?"))), "{log:?}");
    assert!(log.iter().any(|l| l == &format!("GET http://127.0.0.1:{wport}/{NAME}")), "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("AUTH failed")), "{log:?}");
    // nothing can connect to us through the proxy: the tracker is told port 1
    let q = queries.lock().unwrap().clone();
    let announces: Vec<&String> = q.iter().filter(|q| q.contains("/announce?")).collect();
    assert!(!announces.is_empty() && announces.iter().all(|q| q.contains("&port=1&")), "{q:?}");
    assert!(hits.load(Ordering::SeqCst) > 0);
    leech.shutdown();
    check_download(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn socks5_proxy_resolves_tracker_host_names() {
    let f = fixture(None, None);
    let (seed, seed_dir, port) = seeder(&f);
    let seed_addr = SocketAddr::from(([127, 0, 0, 1], port));
    let (tport, _) = http_tracker(vec![seed_addr]);
    let p = proxy(Kind::Socks5, None);
    let dir = temp_dir("leech-names");
    let (leech, mut w) =
        leech(Settings { enable_outgoing_utp: false, ..proxied(ProxyType::Socks5, p.addr, "", "") });
    let mut params =
        AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    params.flags = TorrentFlags::empty();
    params.trackers = vec![format!("http://localhost:{tport}/announce")];
    leech.handle().add_torrent(params);
    let id = wait(&mut w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    finished(&mut w, id, 60);
    assert!(p.saw(&format!("CONNECT localhost:{tport}")), "the name went to the proxy: {:?}", p.log());
    assert!(p.saw(&format!("CONNECT {seed_addr}")));
    leech.shutdown();
    seed.shutdown();
    check_download(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn a_wrong_socks5_password_is_reported() {
    let (tport, queries) = http_tracker(Vec::new());
    let f = fixture(Some(&format!("http://127.0.0.1:{tport}/announce")), None);
    let p = proxy(Kind::Socks5, Some(("alice", "right")));
    let dir = temp_dir("leech-auth");
    let (leech, mut w) = leech(Settings {
        enable_outgoing_utp: false,
        ..proxied(ProxyType::Socks5Pw, p.addr, "alice", "wrong")
    });
    let id = add(&mut w, &f, &dir);
    let message = wait(&mut w, 20, "tracker error", |a| match a {
        Alert::TrackerError { message, .. } => Some(message.clone()),
        Alert::TrackerReply { .. } => panic!("the tracker was reached"),
        _ => None,
    });
    assert!(message.contains("SOCKS authentication error"), "{message}");
    // a peer connection fails the same way, and the proxy's refusal is posted once
    leech.handle().torrent(id).connect_peer(SocketAddr::from(([127, 0, 0, 1], 9)));
    let (op, error) = wait(&mut w, 20, "proxy alert", |a| match a {
        Alert::Proxy { operation, error, proxy } => {
            assert_eq!(proxy, &format!("socks5://{}", p.addr));
            Some((*operation, error.clone()))
        }
        _ => None,
    });
    assert_eq!((op, error.as_str()), (ProxyOperation::Handshake, "SOCKS authentication error"));
    assert!(p.saw("AUTH failed"));
    assert!(queries.lock().unwrap().is_empty());
    leech.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_udp_tracker_through_the_socks5_udp_associate() {
    let f = fixture(None, None);
    let (seed, seed_dir, port) = seeder(&f);
    let seed_addr = SocketAddr::from(([127, 0, 0, 1], port));
    let (tport, sources) = udp_tracker(vec![seed_addr]);
    let p = proxy(Kind::Socks5, None);
    let dir = temp_dir("leech-udp");
    let (leech, mut w) =
        leech(Settings { enable_outgoing_utp: false, ..proxied(ProxyType::Socks5, p.addr, "", "") });
    let mut params =
        AddTorrentParams::new(TorrentSource::TorrentFile(f.torrent.clone()), dir.to_string_lossy());
    params.flags = TorrentFlags::empty();
    params.trackers = vec![format!("udp://127.0.0.1:{tport}/announce")];
    leech.handle().add_torrent(params);
    let id = wait(&mut w, 5, "add", |a| match a {
        Alert::TorrentAdded { id, .. } => Some(*id),
        _ => None,
    });
    wait(&mut w, 30, "tracker reply", |a| match a {
        Alert::TrackerReply { num_peers, .. } => Some(*num_peers),
        Alert::TrackerError { message, .. } => panic!("tracker error: {message}"),
        _ => None,
    });
    finished(&mut w, id, 60);
    let relay = p.relay.lock().unwrap().expect("an associate was made");
    let src = sources.lock().unwrap().clone();
    assert!(
        !src.is_empty() && src.iter().all(|s| *s == relay),
        "every datagram came from the relay: {src:?}"
    );
    assert!(p.saw(&format!("UDP 127.0.0.1:{tport}")));
    assert!(p.saw("ASSOCIATE 0.0.0.0:0"), "{:?}", p.log());
    leech.shutdown();
    seed.shutdown();
    check_download(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}

#[test]
fn the_dht_goes_through_the_socks5_udp_associate() {
    let node = UdpSocket::bind("127.0.0.1:0").unwrap();
    node.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let p = proxy(Kind::Socks5, None);
    let s = Settings {
        enable_dht: true,
        dht_bootstrap_nodes: format!("127.0.0.1:{}", node.local_addr().unwrap().port()),
        socks5_udp_send_local_ep: true,
        ..proxied(ProxyType::Socks5, p.addr, "", "")
    };
    let (leech, _w) = leech(s);
    let mut buf = [0u8; 2048];
    let (n, from) = node.recv_from(&mut buf).expect("a DHT query reaches the node");
    assert_eq!(Some(from), *p.relay.lock().unwrap(), "it came from the relay");
    assert!(buf[0] == b'd' && buf[n - 1] == b'e' && buf[..n].windows(3).any(|w| w == b"1:q"));
    // with `socks5_udp_send_local_ep` the associate names our UDP socket
    assert!(p.log().iter().any(|l| l.starts_with("ASSOCIATE 127.0.0.1:")), "{:?}", p.log());
    leech.shutdown();
}

#[test]
fn utp_download_through_the_socks5_udp_associate() {
    let f = fixture(None, None);
    let (seed, seed_dir, port) = seeder(&f);
    let seed_addr = SocketAddr::from(([127, 0, 0, 1], port));
    let p = proxy(Kind::Socks5, None);
    let dir = temp_dir("leech-utp");
    let (leech, mut w) =
        leech(Settings { enable_outgoing_tcp: false, ..proxied(ProxyType::Socks5, p.addr, "", "") });
    wait(&mut w, 10, "UDP associate", |a| match a {
        Alert::Log { message, .. } if message.contains("UDP associate up") => Some(()),
        _ => None,
    });
    let id = add(&mut w, &f, &dir);
    leech.handle().torrent(id).connect_peer(seed_addr);
    finished(&mut w, id, 60);
    assert!(p.saw(&format!("UDP {seed_addr}")), "uTP went through the relay: {:?}", p.log());
    assert!(!p.saw("CONNECT"), "no TCP: {:?}", p.log());
    leech.shutdown();
    seed.shutdown();
    check_download(&f, &dir);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(seed_dir);
}
