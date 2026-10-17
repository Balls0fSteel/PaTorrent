// Ported from libtorrent-rasterbar 2.0.11: src/tracker_manager.cpp,
// src/http_tracker_connection.cpp, src/udp_tracker_connection.cpp, src/announce_entry.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: request state machines driven by mio events and the session tick instead of asio
// callbacks. UDP trackers are reached from one ephemeral socket per address family (libtorrent
// shares the listen socket); connection ids are cached per endpoint for 60 s, as BEP 15 allows.
// HTTPS trackers go through the OS TLS layer (net::tls) between the socket and the HTTP bytes.

//! Tracker requests and per-torrent tracker state.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use mio::net::{TcpStream, UdpSocket};
use mio::{Interest, Registry, Token};

use super::Slab;
use crate::net::resolver::{Resolved, Resolver};
use crate::net::tls::{self, TlsStream};
use crate::proto::http::{self, ParseError, Url};
use crate::proto::tracker::{
    self, AnnounceRequest, AnnounceResponse, Event, ScrapeEntry, UdpResponse, announce_url, scrape_url,
};
use crate::status::{AnnounceEntry, TrackerStatus};
use crate::types::{Sha1Hash, TorrentId};

pub(crate) const HTTP_TOKEN_BASE: usize = 0x2000_0000;
pub(crate) const UDP4_TOKEN: Token = Token(9);
pub(crate) const UDP6_TOKEN: Token = Token(10);

const UDP_CONNECTION_ID_LIFETIME: Duration = Duration::from_secs(60);
const UDP_ATTEMPTS: u32 = 3;
const UDP_FIRST_TIMEOUT: Duration = Duration::from_secs(5);

// --- Per-torrent tracker state (libtorrent announce_entry / announce_endpoint) ----------------

#[derive(Clone, Debug)]
pub(crate) struct TrackerEntry {
    pub url: String,
    pub tier: u32,
    pub fails: u32,
    pub updating: bool,
    pub next_announce: Option<Instant>,
    /// Announces before this are refused by the tracker (`min interval`).
    pub min_announce: Option<Instant>,
    pub start_sent: bool,
    pub complete_sent: bool,
    pub working: bool,
    pub message: String,
    pub scrape: Option<ScrapeEntry>,
    pub tracker_id: String,
}

impl TrackerEntry {
    pub fn new(url: String, tier: u32) -> TrackerEntry {
        TrackerEntry {
            url,
            tier,
            fails: 0,
            updating: false,
            next_announce: None,
            min_announce: None,
            start_sent: false,
            complete_sent: false,
            working: false,
            message: String::new(),
            scrape: None,
            tracker_id: String::new(),
        }
    }

    /// libtorrent `announce_endpoint::failed` with the default `tracker_backoff` (250 %):
    /// 5 s, 17 s, 36 s, ... capped at an hour.
    pub fn failed(&mut self, message: String, now: Instant) {
        self.fails += 1;
        self.working = false;
        self.updating = false;
        self.message = message;
        let delay = (5.0 + 5.0 * (self.fails as f64).powi(2) * 2.5 / 1.0).min(3600.0);
        self.next_announce = Some(now + Duration::from_secs_f64(delay));
    }

    pub fn reset(&mut self) {
        self.next_announce = None;
        self.min_announce = None;
        self.updating = false;
        self.start_sent = false;
        self.fails = 0;
    }

    pub fn to_status(&self, now: Instant) -> AnnounceEntry {
        AnnounceEntry {
            url: self.url.clone(),
            tier: self.tier,
            status: if self.updating {
                TrackerStatus::Updating
            } else if self.fails > 0 {
                TrackerStatus::Error
            } else if self.working {
                TrackerStatus::Working
            } else {
                TrackerStatus::NotContacted
            },
            message: self.message.clone(),
            scrape_complete: self.scrape.map(|s| s.complete),
            scrape_incomplete: self.scrape.map(|s| s.incomplete),
            scrape_downloaded: self.scrape.map(|s| s.downloaded),
            next_announce: self.next_announce.map(|t| t.saturating_duration_since(now).as_secs() as u32),
            fails: self.fails,
        }
    }
}

/// libtorrent's announce policy: which trackers to announce to now. Trackers are sorted by
/// tier. In each tier the first tracker that is not failing is used (all of them with
/// `all_trackers`); with `all_tiers` every tier gets one, otherwise only the first tier that has
/// a usable tracker.
pub(crate) fn trackers_to_announce(
    trackers: &[TrackerEntry],
    now: Instant,
    all_tiers: bool,
    all_trackers: bool,
) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < trackers.len() {
        let tier = trackers[i].tier;
        let end = trackers[i..].iter().position(|t| t.tier != tier).map_or(trackers.len(), |n| i + n);
        let in_tier = i..end;
        i = end;
        let mut chosen = Vec::new();
        if all_trackers {
            chosen.extend(in_tier.clone());
        } else {
            // First non-failing tracker; if every one fails, the one that may retry soonest.
            let pick = in_tier
                .clone()
                .find(|&t| trackers[t].fails == 0)
                .or_else(|| in_tier.clone().min_by_key(|&t| trackers[t].next_announce.unwrap_or(now)));
            chosen.extend(pick);
        }
        let usable = chosen.iter().any(|&t| trackers[t].fails == 0);
        for t in chosen {
            let e = &trackers[t];
            if !e.updating && e.next_announce.is_none_or(|n| n <= now) {
                out.push(t);
            }
        }
        if !all_tiers && usable {
            break;
        }
    }
    out
}

// --- Requests ---------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) enum JobKind {
    Announce(AnnounceRequest),
    Scrape(Sha1Hash),
}

#[derive(Clone, Debug)]
pub(crate) struct TrackerJob {
    pub torrent: TorrentId,
    /// Index in the torrent's tracker list; `None` for fire-and-forget (stopped) announces.
    pub tracker: Option<usize>,
    pub url: String,
    pub kind: JobKind,
}

#[derive(Debug)]
pub(crate) enum TrackerOutcome {
    /// The reply and the tracker address it came from (libtorrent `tracker_response`'s
    /// `tracker_ip`), to whom the reply's `external ip` vote is attributed.
    Announce(AnnounceResponse, Option<IpAddr>),
    Scrape(ScrapeEntry),
    Failed(String),
}

/// A tracker connection: plain TCP, or TLS over TCP for `https://`.
enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl Stream {
    fn tcp(&mut self) -> &mut TcpStream {
        match self {
            Stream::Plain(s) => s,
            Stream::Tls(s) => s.get_mut(),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

struct HttpConn {
    job: TrackerJob,
    url: Url,
    addrs: Vec<SocketAddr>,
    /// The address being connected to.
    ip: Option<IpAddr>,
    stream: Option<Stream>,
    connected: bool,
    request: Vec<u8>,
    written: usize,
    buf: Vec<u8>,
    deadline: Instant,
    redirects: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UdpStage {
    Resolving,
    Connecting,
    Requesting,
}

struct UdpTxn {
    job: TrackerJob,
    port: u16,
    ep: Option<SocketAddr>,
    stage: UdpStage,
    attempts: u32,
    deadline: Instant,
    connection_id: u64,
}

enum Waiting {
    Http(usize),
    Udp(u32),
}

pub(crate) struct TrackerManager {
    http: Slab<HttpConn>,
    udp4: Option<UdpSocket>,
    udp6: Option<UdpSocket>,
    txns: HashMap<u32, UdpTxn>,
    connection_ids: HashMap<SocketAddr, (u64, Instant)>,
    waiting: HashMap<u64, Waiting>,
    next_cookie: u64,
    results: Vec<(TrackerJob, TrackerOutcome)>,
    user_agent: String,
    completion_timeout: Duration,
    udp_buf: Vec<u8>,
}

impl TrackerManager {
    pub fn new(registry: &Registry, user_agent: String) -> TrackerManager {
        let bind = |addr: &str, token: Token| {
            let mut s = UdpSocket::bind(addr.parse().expect("valid")).ok()?;
            registry.register(&mut s, token, Interest::READABLE).ok()?;
            Some(s)
        };
        TrackerManager {
            http: Slab::default(),
            udp4: bind("0.0.0.0:0", UDP4_TOKEN),
            udp6: bind("[::]:0", UDP6_TOKEN),
            txns: HashMap::new(),
            connection_ids: HashMap::new(),
            waiting: HashMap::new(),
            next_cookie: 1,
            results: Vec::new(),
            user_agent,
            completion_timeout: Duration::from_secs(30),
            udp_buf: vec![0; 64 * 1024],
        }
    }

    pub fn set_options(&mut self, user_agent: &str, completion_timeout: u32) {
        self.user_agent = user_agent.to_owned();
        self.completion_timeout = Duration::from_secs(completion_timeout.max(1) as u64);
    }

    /// Requests in flight.
    pub fn pending(&self) -> usize {
        self.http.len() + self.txns.len()
    }

    pub fn take_results(&mut self) -> Vec<(TrackerJob, TrackerOutcome)> {
        std::mem::take(&mut self.results)
    }

    fn fail(&mut self, job: TrackerJob, msg: impl Into<String>) {
        self.results.push((job, TrackerOutcome::Failed(msg.into())));
    }

    pub fn start(&mut self, job: TrackerJob, resolver: &Resolver, registry: &Registry, now: Instant) {
        let lower = job.url.to_ascii_lowercase();
        if lower.starts_with("udp://") {
            self.start_udp(job, resolver, now);
        } else if lower.starts_with("http://") || lower.starts_with("https://") {
            let url = match &job.kind {
                JobKind::Announce(r) => Some(announce_url(&job.url, r)),
                JobKind::Scrape(ih) => scrape_url(&job.url, &[*ih]),
            };
            match url.as_deref().and_then(Url::parse) {
                Some(url) => self.start_http(job, url, 0, resolver, registry, now),
                None => self.fail(job, "invalid tracker URL"),
            }
        } else {
            self.fail(job, "unsupported tracker protocol")
        }
    }

    fn cookie(&mut self, w: Waiting) -> u64 {
        let c = self.next_cookie;
        self.next_cookie += 1;
        self.waiting.insert(c, w);
        c
    }

    // --- HTTP -------------------------------------------------------------------------------

    fn start_http(
        &mut self,
        job: TrackerJob,
        url: Url,
        redirects: u32,
        resolver: &Resolver,
        registry: &Registry,
        now: Instant,
    ) {
        let request = http::get_request(&url, &self.user_agent);
        let host = url.host.clone();
        let port = url.port;
        let slot = self.http.insert(HttpConn {
            job,
            url,
            addrs: Vec::new(),
            ip: None,
            stream: None,
            connected: false,
            request,
            written: 0,
            buf: Vec::new(),
            deadline: now + self.completion_timeout,
            redirects,
        });
        let cookie = self.cookie(Waiting::Http(slot));
        if let Some(ips) = resolver.resolve(&host, cookie) {
            self.waiting.remove(&cookie);
            let c = self.http.get_mut(slot).expect("just inserted");
            c.addrs = ips.into_iter().rev().map(|ip| SocketAddr::new(ip, port)).collect();
            self.http_connect_next(slot, registry);
        }
    }

    fn http_connect_next(&mut self, slot: usize, registry: &Registry) {
        let Some(c) = self.http.get_mut(slot) else { return };
        if let Some(mut s) = c.stream.take() {
            let _ = registry.deregister(s.tcp());
        }
        while let Some(addr) = c.addrs.pop() {
            if let Ok(mut s) = TcpStream::connect(addr) {
                if registry
                    .register(&mut s, Token(HTTP_TOKEN_BASE + slot), Interest::READABLE | Interest::WRITABLE)
                    .is_ok()
                {
                    // A fresh TLS session per address: the ClientHello waits in it until the
                    // TCP connection is up.
                    c.stream = Some(if c.url.scheme == "https" {
                        match tls::client(&c.url.host) {
                            Ok(t) => Stream::Tls(Box::new(TlsStream::new(s, t))),
                            Err(e) => {
                                let _ = registry.deregister(&mut s);
                                let c = self.http.remove(slot).expect("present");
                                return self.fail(c.job, e.to_string());
                            }
                        }
                    } else {
                        Stream::Plain(s)
                    });
                    c.ip = Some(addr.ip());
                    c.connected = false;
                    c.written = 0;
                    c.buf.clear();
                    return;
                }
            }
        }
        let c = self.http.remove(slot).expect("present");
        self.fail(c.job, "failed to connect to tracker");
    }

    pub fn on_http_event(&mut self, token: Token, resolver: &Resolver, registry: &Registry, now: Instant) {
        let slot = token.0 - HTTP_TOKEN_BASE;
        let Some(c) = self.http.get_mut(slot) else { return };
        let Some(s) = c.stream.as_mut() else { return };
        if !c.connected {
            let tcp = s.tcp();
            match (tcp.take_error(), tcp.peer_addr()) {
                (Ok(None), Ok(_)) => c.connected = true,
                (Ok(None), Err(e)) if e.kind() == io::ErrorKind::NotConnected => return,
                _ => return self.http_connect_next(slot, registry),
            }
        }
        while c.written < c.request.len() {
            match s.write(&c.request[c.written..]) {
                Ok(n) => c.written += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return self.http_done(slot, Err(e.to_string()), resolver, registry, now),
            }
        }
        let mut eof = false;
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match s.read(&mut chunk) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => {
                    c.buf.extend_from_slice(&chunk[..n]);
                    if c.buf.len() > http::MAX_RESPONSE {
                        return self.http_done(
                            slot,
                            Err("tracker response too large".into()),
                            resolver,
                            registry,
                            now,
                        );
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return self.http_done(slot, Err(e.to_string()), resolver, registry, now),
            }
        }
        match http::parse_response(&c.buf, eof) {
            Ok(resp) => self.http_done(slot, Ok(resp), resolver, registry, now),
            Err(ParseError::Incomplete) => {}
            Err(ParseError::Invalid(m)) => self.http_done(slot, Err(m.into()), resolver, registry, now),
        }
    }

    fn http_done(
        &mut self,
        slot: usize,
        result: Result<http::Response, String>,
        resolver: &Resolver,
        registry: &Registry,
        now: Instant,
    ) {
        let Some(mut c) = self.http.remove(slot) else { return };
        if let Some(mut s) = c.stream.take() {
            let _ = registry.deregister(s.tcp());
        }
        let resp = match result {
            Ok(r) => r,
            Err(e) => return self.fail(c.job, e),
        };
        if resp.is_redirect() {
            if c.redirects >= http::MAX_REDIRECTS {
                return self.fail(c.job, "too many redirects");
            }
            let next = resp
                .header("location")
                .and_then(|l| http::resolve_redirect(&c.url, l))
                .and_then(|u| Url::parse(&u));
            return match next {
                Some(u) => self.start_http(c.job, u, c.redirects + 1, resolver, registry, now),
                None => self.fail(c.job, "invalid redirect"),
            };
        }
        if resp.status != 200 {
            return self.fail(c.job, format!("{} {}", resp.status, resp.message));
        }
        let outcome = match &c.job.kind {
            JobKind::Announce(_) => {
                tracker::parse_announce_response(&resp.body).map(|r| TrackerOutcome::Announce(r, c.ip))
            }
            JobKind::Scrape(ih) => tracker::parse_scrape_response(&resp.body, ih).map(TrackerOutcome::Scrape),
        };
        self.results.push((c.job, outcome.unwrap_or_else(TrackerOutcome::Failed)));
    }

    // --- UDP --------------------------------------------------------------------------------

    fn new_txn_id(&self) -> u32 {
        loop {
            let id = crate::util::random::random_u64() as u32;
            if !self.txns.contains_key(&id) {
                return id;
            }
        }
    }

    fn start_udp(&mut self, job: TrackerJob, resolver: &Resolver, now: Instant) {
        let parts = crate::util::string::parse_url_components(&job.url);
        let (host, port) = match parts {
            Ok(p) if p.port.is_some_and(|p| p != 0) && !p.host.is_empty() => {
                (p.host, p.port.expect("checked"))
            }
            _ => return self.fail(job, "invalid UDP tracker URL"),
        };
        let id = self.new_txn_id();
        self.txns.insert(
            id,
            UdpTxn {
                job,
                port,
                ep: None,
                stage: UdpStage::Resolving,
                attempts: 0,
                deadline: now + self.completion_timeout,
                connection_id: 0,
            },
        );
        let cookie = self.cookie(Waiting::Udp(id));
        if let Some(ips) = resolver.resolve(&host, cookie) {
            self.waiting.remove(&cookie);
            self.udp_resolved(id, ips, now);
        }
    }

    fn udp_resolved(&mut self, id: u32, ips: Vec<IpAddr>, now: Instant) {
        let have6 = self.udp6.is_some();
        let Some(t) = self.txns.get_mut(&id) else { return };
        let ip = ips.iter().find(|ip| ip.is_ipv4()).or_else(|| ips.iter().find(|_| have6)).copied();
        let Some(ip) = ip else {
            let t = self.txns.remove(&id).expect("present");
            return self.fail(t.job, "no usable tracker address");
        };
        t.ep = Some(SocketAddr::new(ip, t.port));
        self.udp_send(id, now);
    }

    /// Sends the next packet of transaction `id`: a connect, or the request if we hold a fresh
    /// connection id for the endpoint.
    fn udp_send(&mut self, id: u32, now: Instant) {
        let Some(mut t) = self.txns.remove(&id) else { return };
        let ep = t.ep.expect("resolved");
        if t.attempts >= UDP_ATTEMPTS {
            return self.fail(t.job, "timed out");
        }
        let new_id = self.new_txn_id();
        let cached = self
            .connection_ids
            .get(&ep)
            .filter(|(_, at)| now.duration_since(*at) < UDP_CONNECTION_ID_LIFETIME);
        let packet = match cached {
            Some(&(cid, _)) => {
                t.stage = UdpStage::Requesting;
                t.connection_id = cid;
                match &t.job.kind {
                    JobKind::Announce(r) => tracker::udp_announce(cid, new_id, r),
                    JobKind::Scrape(ih) => tracker::udp_scrape(cid, new_id, &[*ih]),
                }
            }
            None => {
                t.stage = UdpStage::Connecting;
                tracker::udp_connect(new_id)
            }
        };
        let sock = if ep.is_ipv4() { self.udp4.as_ref() } else { self.udp6.as_ref() };
        let Some(sock) = sock else {
            return self.fail(t.job, "no UDP socket for the tracker's address family");
        };
        // Send errors (unreachable networks) are treated like a lost packet.
        let _ = sock.send_to(&packet, ep);
        t.deadline = now + UDP_FIRST_TIMEOUT * 2u32.pow(t.attempts);
        t.attempts += 1;
        self.txns.insert(new_id, t);
    }

    pub fn on_udp_readable(&mut self, v6: bool, now: Instant) {
        loop {
            let sock = if v6 { self.udp6.as_ref() } else { self.udp4.as_ref() };
            let Some(sock) = sock else { return };
            let (n, from) = match sock.recv_from(&mut self.udp_buf) {
                Ok(r) => r,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                // Windows reports ICMP port-unreachable as a receive error; ignore it.
                Err(_) => continue,
            };
            let Some(resp) = tracker::udp_decode(&self.udp_buf[..n], v6) else { continue };
            let txn = match &resp {
                UdpResponse::Connect { txn, .. }
                | UdpResponse::Announce { txn, .. }
                | UdpResponse::Scrape { txn, .. }
                | UdpResponse::Error { txn, .. } => *txn,
            };
            let Some(t) = self.txns.get(&txn) else { continue };
            if t.ep != Some(from) {
                continue;
            }
            let ep = t.ep.expect("resolved");
            match resp {
                UdpResponse::Connect { connection_id, .. } if t.stage == UdpStage::Connecting => {
                    self.connection_ids.insert(ep, (connection_id, now));
                    let mut t = self.txns.remove(&txn).expect("present");
                    t.attempts = 0;
                    self.txns.insert(txn, t);
                    self.udp_send(txn, now);
                }
                UdpResponse::Announce { interval, leechers, seeders, peers, .. }
                    if t.stage == UdpStage::Requesting =>
                {
                    let t = self.txns.remove(&txn).expect("present");
                    let r = AnnounceResponse {
                        peers,
                        interval: if interval == 0 { tracker::DEFAULT_INTERVAL } else { interval },
                        complete: Some(seeders),
                        incomplete: Some(leechers),
                        ..Default::default()
                    };
                    let ip = t.ep.map(|e| e.ip());
                    self.results.push((t.job, TrackerOutcome::Announce(r, ip)));
                }
                UdpResponse::Scrape { entries, .. } if t.stage == UdpStage::Requesting => {
                    let t = self.txns.remove(&txn).expect("present");
                    match entries.first() {
                        Some(e) => self.results.push((t.job, TrackerOutcome::Scrape(*e))),
                        None => self.fail(t.job, "empty scrape response"),
                    }
                }
                UdpResponse::Error { message, .. } => {
                    let t = self.txns.remove(&txn).expect("present");
                    self.connection_ids.remove(&ep);
                    self.fail(t.job, message);
                }
                _ => {}
            }
        }
    }

    pub fn on_resolved(&mut self, r: Resolved, registry: &Registry, now: Instant) {
        let Some(w) = self.waiting.remove(&r.cookie) else { return };
        match (w, r.result) {
            (Waiting::Http(slot), Ok(ips)) => {
                let Some(c) = self.http.get_mut(slot) else { return };
                let port = c.url.port;
                c.addrs = ips.into_iter().rev().map(|ip| SocketAddr::new(ip, port)).collect();
                self.http_connect_next(slot, registry);
            }
            (Waiting::Http(slot), Err(e)) => {
                if let Some(c) = self.http.remove(slot) {
                    self.fail(c.job, format!("{}: {e}", r.host));
                }
            }
            (Waiting::Udp(id), Ok(ips)) => self.udp_resolved(id, ips, now),
            (Waiting::Udp(id), Err(e)) => {
                if let Some(t) = self.txns.remove(&id) {
                    self.fail(t.job, format!("{}: {e}", r.host));
                }
            }
        }
    }

    /// Timeouts and UDP retransmissions.
    pub fn tick(&mut self, registry: &Registry, now: Instant) {
        let expired: Vec<usize> =
            self.http.iter().filter(|(_, c)| c.deadline <= now).map(|(i, _)| i).collect();
        for slot in expired {
            if let Some(mut c) = self.http.remove(slot) {
                if let Some(mut s) = c.stream.take() {
                    let _ = registry.deregister(s.tcp());
                }
                self.fail(c.job, "timed out");
            }
        }
        let due: Vec<u32> = self.txns.iter().filter(|(_, t)| t.deadline <= now).map(|(&id, _)| id).collect();
        for id in due {
            if self.txns.get(&id).is_some_and(|t| t.stage == UdpStage::Resolving) {
                let t = self.txns.remove(&id).expect("present");
                self.fail(t.job, "timed out resolving the tracker");
            } else {
                self.udp_send(id, now);
            }
        }
        self.connection_ids.retain(|_, (_, at)| now.duration_since(*at) < UDP_CONNECTION_ID_LIFETIME);
    }
}

/// Builds the announce request for a torrent.
#[allow(clippy::too_many_arguments)]
pub(crate) fn make_announce(
    info_hash: Sha1Hash,
    peer_id: [u8; 20],
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    corrupt: u64,
    event: Event,
    key: u32,
    num_want: u32,
    tracker_id: String,
) -> AnnounceRequest {
    AnnounceRequest {
        info_hash,
        peer_id,
        port,
        uploaded,
        downloaded,
        left,
        corrupt,
        event,
        key,
        num_want,
        tracker_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(tiers: &[u32]) -> Vec<TrackerEntry> {
        tiers.iter().enumerate().map(|(i, &t)| TrackerEntry::new(format!("udp://t{i}:1"), t)).collect()
    }

    #[test]
    fn announce_policy() {
        let now = Instant::now();
        let mut t = entries(&[0, 0, 1, 2]);
        assert_eq!(trackers_to_announce(&t, now, false, false), [0]);
        assert_eq!(trackers_to_announce(&t, now, true, false), [0, 2, 3]);
        assert_eq!(trackers_to_announce(&t, now, true, true), [0, 1, 2, 3]);

        // Tracker 0 fails: the next one in its tier takes over.
        t[0].failed("down".into(), now);
        assert_eq!(trackers_to_announce(&t, now, false, false), [1]);
        // Every tracker of tier 0 fails: fall through to tier 1, and retry tier 0 when due.
        t[1].failed("down".into(), now);
        assert_eq!(trackers_to_announce(&t, now, false, false), [2]);
        let later = now + Duration::from_secs(60);
        assert_eq!(trackers_to_announce(&t, later, false, false), [0, 2]);

        t[2].updating = true;
        assert!(!trackers_to_announce(&t, now, true, false).contains(&2));
        t[3].next_announce = Some(now + Duration::from_secs(1));
        assert!(!trackers_to_announce(&t, now, true, false).contains(&3));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let now = Instant::now();
        let mut e = TrackerEntry::new("x".into(), 0);
        let mut prev = Duration::ZERO;
        for _ in 0..30 {
            e.failed("x".into(), now);
            let d = e.next_announce.unwrap() - now;
            assert!(d >= prev && d <= Duration::from_secs(3600));
            prev = d;
        }
        assert_eq!(prev, Duration::from_secs(3600));
        assert_eq!(e.to_status(now).status, TrackerStatus::Error);
    }
}
