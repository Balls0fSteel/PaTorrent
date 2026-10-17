// Ported from libtorrent-rasterbar 2.0.11: src/tracker_manager.cpp,
// src/http_tracker_connection.cpp, src/udp_tracker_connection.cpp, src/announce_entry.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: request state machines driven by mio events and the session tick instead of asio
// callbacks. UDP trackers are reached from one ephemeral socket per address family (libtorrent
// shares the listen socket); connection ids are cached per endpoint for 60 s, as BEP 15 allows.
// HTTPS trackers go through the OS TLS layer (net::tls) between the socket and the HTTP bytes.
// Through a proxy (libtorrent `http_connection` with `proxy_settings`, `udp_tracker_connection`
// on a proxied `udp_socket`): HTTP requests wait while the proxy's name resolves; UDP tracker
// datagrams are queued for the engine to send through the SOCKS5 associate on its listen socket.

//! Tracker requests and per-torrent tracker state.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mio::net::{TcpStream, UdpSocket};
use mio::{Interest, Registry, Token};

use super::Slab;
use super::proxy::{ProxyConfig, Tunnel};
use crate::net::ip_filter::IpFilter;
use crate::net::resolver::{Resolved, Resolver};
use crate::net::tls::{self, TlsStream};
use crate::proto::http::{self, ParseError, Url};
use crate::proto::proxy::Target;
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
    /// Tracker addresses this filter blocks are not contacted (libtorrent
    /// `tracker_request::filter`).
    pub filter: Option<Arc<IpFilter>>,
}

impl TrackerJob {
    /// Drops the addresses the job's filter blocks (libtorrent `on_filter` /
    /// `udp_tracker_connection::start_announce`).
    fn filter_ips(&self, ips: &mut Vec<IpAddr>) {
        if let Some(f) = &self.filter {
            ips.retain(|ip| !f.is_blocked(*ip));
        }
    }
}

/// libtorrent `errors::banned_by_ip_filter`.
const BANNED_BY_IP_FILTER: &str = "banned by IP filter";

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
    /// A target the proxy resolves (or, for a plain HTTP proxy request, names in the URL).
    name: Option<Target>,
    addrs: Vec<SocketAddr>,
    /// The proxy handshake, once connected to the proxy.
    tunnel: Option<Tunnel>,
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
    /// The tracker's address, sent to directly.
    ep: Option<SocketAddr>,
    /// Where datagrams go: the address, or through a proxy, maybe a host name.
    target: Option<Target>,
    stage: UdpStage,
    attempts: u32,
    deadline: Instant,
    connection_id: u64,
}

enum Waiting {
    Http(usize),
    Udp(u32),
}

/// The proxy trackers go through.
struct TrackerProxy {
    cfg: ProxyConfig,
    /// Unknown while its name resolves.
    addr: Option<SocketAddr>,
    /// Its name failed to resolve.
    error: Option<String>,
}

pub(crate) struct TrackerManager {
    http: Slab<HttpConn>,
    udp4: Option<UdpSocket>,
    udp6: Option<UdpSocket>,
    txns: HashMap<u32, UdpTxn>,
    connection_ids: HashMap<Target, (u64, Instant)>,
    proxy: Option<TrackerProxy>,
    /// HTTP requests waiting for the proxy's address.
    parked: Vec<TrackerJob>,
    /// UDP trackers go through the proxy: datagrams wait in `proxied_out` for the engine.
    udp_proxied: bool,
    /// ... but the proxy carries no UDP.
    udp_refused: bool,
    proxied_out: Vec<(Target, Vec<u8>)>,
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
            proxy: None,
            parked: Vec::new(),
            udp_proxied: false,
            udp_refused: false,
            proxied_out: Vec::new(),
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
        self.http.len() + self.txns.len() + self.parked.len()
    }

    /// The proxy for tracker connections changed or its name resolved. Requests that waited for
    /// it go on (or fail, if its name did not resolve).
    pub fn set_proxy(
        &mut self,
        cfg: Option<ProxyConfig>,
        addr: Option<SocketAddr>,
        error: Option<String>,
        resolver: &Resolver,
        registry: &Registry,
        now: Instant,
    ) {
        self.proxy = cfg.map(|cfg| TrackerProxy { cfg, addr, error });
        for job in std::mem::take(&mut self.parked) {
            self.start(job, resolver, registry, now);
        }
    }

    /// UDP trackers go through the proxy (`refused`: which carries no UDP).
    pub fn set_udp_proxied(&mut self, proxied: bool, refused: bool) {
        self.udp_proxied = proxied;
        self.udp_refused = refused;
        if !proxied {
            self.proxied_out.clear();
        }
    }

    /// Datagrams to send through the proxy.
    pub fn take_proxied_out(&mut self) -> Vec<(Target, Vec<u8>)> {
        std::mem::take(&mut self.proxied_out)
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
            if let Some(p) = &self.proxy {
                if let Some(e) = &p.error {
                    let e = format!("proxy: {e}");
                    return self.fail(job, e);
                }
                if p.addr.is_none() {
                    self.parked.push(job);
                    return;
                }
            }
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
        // through a proxy: an HTTP proxy is asked for plain http:// URLs directly; otherwise a
        // tunnel to the target, named or resolved here first
        let mut request = http::get_request(&url, &self.user_agent);
        let mut name = None;
        if let Some(p) = &self.proxy {
            let t = Target::new(&url.host, url.port);
            if p.cfg.plain_http(url.scheme == "https") {
                request = http::proxy_get_request(&url, &self.user_agent, p.cfg.creds());
                name = Some(t);
            } else if matches!(t, Target::Addr(_)) || p.cfg.resolves_names() {
                name = Some(t);
            }
        }
        if let Some(Target::Addr(a)) = &name {
            if job.filter.as_ref().is_some_and(|f| f.is_blocked(a.ip())) {
                return self.fail(job, BANNED_BY_IP_FILTER);
            }
        }
        let host = url.host.clone();
        let named = name.is_some();
        let slot = self.http.insert(HttpConn {
            job,
            url,
            name,
            tunnel: None,
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
        if named {
            return self.http_connect_next(slot, registry);
        }
        let cookie = self.cookie(Waiting::Http(slot));
        if let Some(ips) = resolver.resolve(&host, cookie) {
            self.waiting.remove(&cookie);
            self.http_resolved(slot, ips, registry);
        }
    }

    fn http_resolved(&mut self, slot: usize, mut ips: Vec<IpAddr>, registry: &Registry) {
        let Some(c) = self.http.get_mut(slot) else { return };
        let any = !ips.is_empty();
        c.job.filter_ips(&mut ips);
        if any && ips.is_empty() {
            let c = self.http.remove(slot).expect("present");
            return self.fail(c.job, BANNED_BY_IP_FILTER);
        }
        let port = c.url.port;
        c.addrs = ips.into_iter().rev().map(|ip| SocketAddr::new(ip, port)).collect();
        self.http_connect_next(slot, registry);
    }

    fn http_connect_next(&mut self, slot: usize, registry: &Registry) {
        let Some(c) = self.http.get_mut(slot) else { return };
        if let Some(mut s) = c.stream.take() {
            let _ = registry.deregister(s.tcp());
        }
        let proxy = self.proxy.as_ref().map(|p| (&p.cfg, p.addr));
        loop {
            // the address to connect to, and what to ask a proxy for
            let (addr, target) = match (c.name.take(), proxy) {
                (Some(t), Some((_, Some(proxy)))) => (proxy, t),
                (None, Some((_, Some(proxy)))) => match c.addrs.pop() {
                    Some(a) => (proxy, Target::Addr(a)),
                    None => break,
                },
                (None, None) => match c.addrs.pop() {
                    Some(a) => (a, Target::Addr(a)),
                    None => break,
                },
                _ => break,
            };
            c.tunnel = None;
            if let Some((cfg, _)) = proxy {
                if !cfg.plain_http(c.url.scheme == "https") {
                    match cfg.tunnel(&target) {
                        Ok(hs) => c.tunnel = Some(Tunnel::new(hs)),
                        Err(e) => {
                            let c = self.http.remove(slot).expect("present");
                            return self.fail(c.job, format!("proxy: {e}"));
                        }
                    }
                }
            }
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
                    // a tracker reached through a proxy sees the proxy's address, not ours:
                    // its `external ip` is no vote
                    c.ip = if proxy.is_some() { None } else { Some(addr.ip()) };
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
        // through a proxy: its handshake on the bare socket, before TLS or the request
        if let Some(t) = c.tunnel.as_mut() {
            match t.drive(s.tcp()) {
                Ok(None) => return,
                Ok(Some(_)) => c.tunnel = None,
                Err(e) => return self.http_done(slot, Err(e.to_string()), resolver, registry, now),
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
        if self.udp_proxied && self.udp_refused {
            // libtorrent `udp_socket::send`: `permission_denied` without a SOCKS5 associate
            return self.fail(job, "Permission denied (UDP trackers need a SOCKS5 proxy)");
        }
        // a SOCKS5 proxy with `proxy_hostnames` resolves the name (libtorrent
        // `udp_tracker_connection::start`)
        let by_proxy = self.udp_proxied
            && self.proxy.as_ref().is_some_and(|p| p.cfg.kind.is_socks5() && p.cfg.hostnames)
            && matches!(Target::new(&host, port), Target::Host(..));
        let id = self.new_txn_id();
        self.txns.insert(
            id,
            UdpTxn {
                job,
                port,
                ep: None,
                target: by_proxy.then(|| Target::Host(host.clone(), port)),
                stage: UdpStage::Resolving,
                attempts: 0,
                deadline: now + self.completion_timeout,
                connection_id: 0,
            },
        );
        if by_proxy {
            return self.udp_send(id, now);
        }
        let cookie = self.cookie(Waiting::Udp(id));
        if let Some(ips) = resolver.resolve(&host, cookie) {
            self.waiting.remove(&cookie);
            self.udp_resolved(id, ips, now);
        }
    }

    fn udp_resolved(&mut self, id: u32, mut ips: Vec<IpAddr>, now: Instant) {
        let have6 = self.udp6.is_some();
        let Some(t) = self.txns.get_mut(&id) else { return };
        let any = !ips.is_empty();
        t.job.filter_ips(&mut ips);
        if any && ips.is_empty() {
            let t = self.txns.remove(&id).expect("present");
            return self.fail(t.job, BANNED_BY_IP_FILTER);
        }
        let ip = ips.iter().find(|ip| ip.is_ipv4()).or_else(|| ips.iter().find(|_| have6)).copied();
        let ip = if self.udp_proxied { ips.first().copied() } else { ip };
        let Some(ip) = ip else {
            let t = self.txns.remove(&id).expect("present");
            return self.fail(t.job, "no usable tracker address");
        };
        let ep = SocketAddr::new(ip, t.port);
        t.target = Some(Target::Addr(ep));
        // replies through the proxy are matched by transaction id and source address alone
        t.ep = (!self.udp_proxied).then_some(ep);
        self.udp_send(id, now);
    }

    /// Sends the next packet of transaction `id`: a connect, or the request if we hold a fresh
    /// connection id for the endpoint.
    fn udp_send(&mut self, id: u32, now: Instant) {
        let Some(mut t) = self.txns.remove(&id) else { return };
        let target = t.target.clone().expect("resolved");
        if t.attempts >= UDP_ATTEMPTS {
            return self.fail(t.job, "timed out");
        }
        let new_id = self.new_txn_id();
        let cached = self
            .connection_ids
            .get(&target)
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
        if self.udp_proxied {
            // the engine sends it through the proxy; until the associate is up it is lost
            self.proxied_out.push((target, packet));
        } else {
            // a transaction started through a proxy that has been turned off since goes direct
            // if it knows the tracker's address
            let ep = match (t.ep, &target) {
                (Some(ep), _) => ep,
                (None, Target::Addr(a)) => *t.ep.insert(*a),
                (None, Target::Host(..)) => return self.fail(t.job, "the proxy was turned off"),
            };
            let sock = if ep.is_ipv4() { self.udp4.as_ref() } else { self.udp6.as_ref() };
            let Some(sock) = sock else {
                return self.fail(t.job, "no UDP socket for the tracker's address family");
            };
            // Send errors (unreachable networks) are treated like a lost packet.
            let _ = sock.send_to(&packet, ep);
        }
        t.deadline = now + UDP_FIRST_TIMEOUT * 2u32.pow(t.attempts);
        t.attempts += 1;
        self.txns.insert(new_id, t);
    }

    pub fn on_udp_readable(&mut self, v6: bool, now: Instant) {
        let mut buf = std::mem::take(&mut self.udp_buf);
        loop {
            let sock = if v6 { self.udp6.as_ref() } else { self.udp4.as_ref() };
            let Some(sock) = sock else { break };
            let (n, from) = match sock.recv_from(&mut buf) {
                Ok(r) => r,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                // Windows reports ICMP port-unreachable as a receive error; ignore it.
                Err(_) => continue,
            };
            if !self.udp_proxied {
                self.on_udp_packet(Some(from), &buf[..n], now);
            }
        }
        self.udp_buf = buf;
    }

    /// A datagram the proxy relayed from `from` (`None`: from a host name). Whether it was a
    /// UDP tracker's reply.
    pub fn on_proxied_packet(&mut self, from: Option<SocketAddr>, data: &[u8], now: Instant) -> bool {
        self.udp_proxied && self.on_udp_packet(from, data, now)
    }

    fn on_udp_packet(&mut self, from: Option<SocketAddr>, data: &[u8], now: Instant) -> bool {
        {
            let v6 = from.is_some_and(|f| f.is_ipv6());
            let Some(resp) = tracker::udp_decode(data, v6) else { return false };
            let txn = match &resp {
                UdpResponse::Connect { txn, .. }
                | UdpResponse::Announce { txn, .. }
                | UdpResponse::Scrape { txn, .. }
                | UdpResponse::Error { txn, .. } => *txn,
            };
            let Some(t) = self.txns.get(&txn) else { return false };
            let Some(ep) = t.target.clone() else { return false };
            match (&ep, from) {
                (Target::Addr(a), Some(f)) if *a != f => return false,
                (Target::Addr(_), None) => return false,
                _ => {}
            }
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
        true
    }

    pub fn on_resolved(&mut self, r: Resolved, registry: &Registry, now: Instant) {
        let Some(w) = self.waiting.remove(&r.cookie) else { return };
        match (w, r.result) {
            (Waiting::Http(slot), Ok(ips)) => self.http_resolved(slot, ips, registry),
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
