// Ported from libtorrent-rasterbar 2.0.11: src/torrent.cpp (web_seed_t, add_web_seed,
// remove_web_seed, web_seeds, maybe_connect_web_seeds, connect_to_url_seed, on_name_lookup,
// connect_web_seed, retry_web_seed, remove_web_seed_conn, the web seed branches of piece_failed
// and ban_peer), src/web_peer_connection.cpp (on_connected, disconnect, received_invalid_data,
// the torrent side of handle_error, handle_redirect and disable), src/web_connection_base.cpp
// (timeout, on_connected), src/http_seed_connection.cpp (on_connected, disconnect, the torrent
// side of its redirects), src/ip_helpers.cpp (is_local, is_global), src/parse_url.cpp (is_idna)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: web seed connections live in a slab of their own beside the BitTorrent peers and are
// driven by mio events like tracker connections; their HTTP side is the sans-IO
// `proto::web_peer`. A web seed keeps its own trust points (libtorrent: a `torrent_peer` outside
// the peer list). A web seed blocked by the IP filter waits `urlseed_wait_retry` before the
// next try (libtorrent retries, and posts the alert, every second); a 503 or 429 answer does
// not mark the file as missing on that server (libtorrent stops asking it for that file); an
// empty file a server lacks does not take the piece it sits in away from that server. Through a
// proxy (libtorrent `connect_to_url_seed`'s proxy branches): an HTTP proxy is asked for plain
// `http://` seeds directly; otherwise a tunnel is made to the seed, by name when the proxy
// resolves names. A seed reached by name has a placeholder address (`100::/64`) for blame.

//! Web seeds (BEP 19 `url-list` and BEP 17 `httpseeds`): HTTP servers that hold the torrent's
//! files and act as seeds.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use mio::net::TcpStream;
use mio::{Interest, Token};

use super::proxy::Tunnel;
use super::smart_ban::Trust;
use super::torrent::Torrent;
use super::{Engine, Slab};
use crate::alert::{Alert, BlockReason};
use crate::disk::BLOCK_SIZE;
use crate::metainfo::WebSeedKind;
use crate::metainfo::file_storage::file_piece_range_inclusive;
use crate::net::resolver::Resolved;
use crate::net::tls::{self, TlsStream};
use crate::params::TorrentFlags;
use crate::proto::peer_wire::{self, msg};
use crate::proto::proxy::Target;
use crate::proto::web_peer::{Restart, WebEvent, WebPeer};
use crate::status::{PeerFlags, PeerInfo, PeerSource};
use crate::types::TorrentId;
use crate::util::bitfield::Bitfield;
use crate::util::stat::Stat;
use crate::util::string::parse_url_components;

/// Reactor tokens of web seed connections.
pub(crate) const WEB_TOKEN_BASE: usize = 0x6000_0000;
/// Resolver cookies of web seed lookups (the DHT's carry bit 63).
pub(crate) const WEB_COOKIE: u64 = 1 << 62;
const READ_CHUNK: usize = 64 * 1024;

/// One web seed of a torrent (libtorrent `web_seed_t`).
#[derive(Clone, Debug)]
pub(crate) struct WebSeedEntry {
    /// Stable within the torrent; connections and lookups name their seed by it.
    pub id: u32,
    pub url: String,
    pub kind: WebSeedKind,
    pub auth: String,
    pub extra_headers: Vec<(String, String)>,
    /// No reconnect before this.
    pub retry: Option<Instant>,
    /// The resolved addresses; a failed connect drops the first.
    pub endpoints: Vec<SocketAddr>,
    pub resolving: bool,
    /// Removed while resolving or connected: dropped once that is over.
    pub removed: bool,
    /// The server may have something we want (false once all its files went elsewhere).
    pub interesting: bool,
    /// Created by a redirect: not saved in resume data.
    pub ephemeral: bool,
    /// Created by a redirect from a global address with SSRF mitigation on: it may not
    /// resolve to a local address.
    pub no_local_ips: bool,
    /// Not used again this session (but kept in resume data).
    pub disabled: bool,
    pub supports_keepalive: bool,
    pub restart: Option<Restart>,
    /// File index → the path it was redirected to on this server.
    pub redirects: HashMap<usize, String>,
    /// The files this server has; empty means all of them.
    pub have_files: Bitfield,
    pub trust: Trust,
    pub banned: bool,
    /// The connection's slot, while connected.
    pub conn: Option<usize>,
    /// The address of the last connection, for blame.
    pub addr: Option<SocketAddr>,
}

/// A torrent's web seeds.
#[derive(Clone, Debug, Default)]
pub(crate) struct WebSeeds {
    pub list: Vec<WebSeedEntry>,
    next_id: u32,
}

impl WebSeeds {
    /// libtorrent `torrent::add_web_seed`: duplicates are not added (an ephemeral one is
    /// promoted to a permanent one). Returns the entry's index.
    pub fn add(
        &mut self,
        url: &str,
        kind: WebSeedKind,
        auth: &str,
        extra_headers: &[(String, String)],
        ephemeral: bool,
        no_local_ips: bool,
    ) -> usize {
        if let Some(i) = self.list.iter().position(|w| w.url == url && w.kind == kind && !w.removed) {
            if self.list[i].ephemeral && !ephemeral {
                self.list[i].ephemeral = false;
            }
            return i;
        }
        self.next_id += 1;
        self.list.push(WebSeedEntry {
            id: self.next_id,
            url: url.to_owned(),
            kind,
            auth: auth.to_owned(),
            extra_headers: extra_headers.to_vec(),
            retry: None,
            endpoints: Vec::new(),
            resolving: false,
            removed: false,
            interesting: true,
            ephemeral,
            no_local_ips,
            disabled: false,
            supports_keepalive: true,
            restart: None,
            redirects: HashMap::new(),
            have_files: Bitfield::default(),
            trust: Trust::default(),
            banned: false,
            conn: None,
            addr: None,
        });
        self.list.len() - 1
    }

    /// Adds non-empty, trimmed URLs.
    pub fn add_urls<'a>(&mut self, urls: impl IntoIterator<Item = &'a String>, kind: WebSeedKind) {
        for u in urls {
            let u = u.trim();
            if !u.is_empty() {
                self.add(u, kind, "", &[], false, false);
            }
        }
    }

    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.list.iter().position(|w| w.id == id)
    }

    /// libtorrent `torrent::web_seeds(type)`: neither banned nor removed.
    pub fn urls(&self, kind: WebSeedKind) -> Vec<String> {
        self.list
            .iter()
            .filter(|w| w.kind == kind && !w.banned && !w.removed)
            .map(|w| w.url.clone())
            .collect()
    }

    /// The seeds saved in resume data (libtorrent `write_resume_data`: not ephemeral ones).
    pub fn resume_urls(&self, kind: WebSeedKind) -> Vec<String> {
        self.list
            .iter()
            .filter(|w| w.kind == kind && !w.ephemeral && !w.removed)
            .map(|w| w.url.clone())
            .collect()
    }
}

/// libtorrent `aux::is_local`: private, link-local, loopback and CGNAT addresses.
pub(crate) fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let ip = u32::from(a);
            (ip & 0xff00_0000) == 0x0a00_0000
                || (ip & 0xfff0_0000) == 0xac10_0000
                || (ip & 0xffff_0000) == 0xc0a8_0000
                || (ip & 0xffff_0000) == 0xa9fe_0000
                || (ip & 0xff00_0000) == 0x7f00_0000
                || (ip & 0xffc0_0000) == 0x6440_0000
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_local(IpAddr::V4(v4));
            }
            let s = a.segments();
            a.is_loopback()
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || (s[0] & 0xffc0) == 0xfec0 // site local
                || (s[0] & 0xff0f) == 0xff02 // multicast link local
                || (s[0] & 0xff0f) == 0xff05 // multicast site local
                || (a.octets()[0] & 0xfe) == 0xfc // unique local
        }
    }
}

/// libtorrent `aux::is_global`.
pub(crate) fn is_global(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V6(a) => match a.to_ipv4_mapped() {
            Some(v4) => is_global(IpAddr::V4(v4)),
            None => (a.octets()[0] & 0xe0) == 0x20,
        },
        IpAddr::V4(a) => !(a.is_multicast() || a.is_unspecified() || is_local(ip)),
    }
}

/// libtorrent `is_idna`: a label starting with `xn--`.
fn is_idna(host: &str) -> bool {
    host.split('.').any(|l| l.len() >= 4 && l[..2].eq_ignore_ascii_case("xn") && &l[2..4] == "--")
}

/// How a connection ended (libtorrent's disconnect severities, plus the two causes that
/// change the seed's state).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Close {
    Normal,
    /// `failure` or `peer_error`: no reconnect before `urlseed_wait_retry`.
    Failure,
    /// The connect failed: the next attempt uses the next address.
    ConnectFailed,
    /// libtorrent `uninteresting_upload_peer`: the server has nothing we want.
    Uninteresting,
}

/// A connection to a web seed: plain TCP, or TLS over TCP for `https://`.
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

/// One web seed connection (libtorrent `web_peer_connection` / `http_seed_connection`).
pub(crate) struct WebConn {
    torrent: TorrentId,
    seed: u32,
    addr: SocketAddr,
    https: bool,
    stream: Stream,
    connecting: bool,
    /// The proxy handshake, after the TCP connect and before anything else.
    tunnel: Option<Tunnel>,
    proto: WebPeer,
    /// The pieces this server has, unless `all`.
    have: Bitfield,
    all: bool,
    /// Its pieces are counted in the picker's availability.
    counted: bool,
    am_interested: bool,
    connected_at: Instant,
    last_receive: Instant,
    stat: Stat,
    read_blocked: bool,
    /// Block counts of the HTTP request runs not fully answered yet, oldest first.
    runs: std::collections::VecDeque<usize>,
}

/// The session's web seed connections and lookups.
#[derive(Default)]
pub(crate) struct WebConns {
    conns: Slab<WebConn>,
    /// Resolver cookie → (torrent, seed, port).
    waiting: HashMap<u64, (TorrentId, u32, u16)>,
    next_cookie: u64,
    buf: Vec<u8>,
}

impl WebConns {
    pub fn len(&self) -> usize {
        self.conns.len()
    }

    /// Connected web seeds of a torrent, and how many of them have every piece.
    pub fn counts(&self, id: TorrentId) -> (u32, u32) {
        let mut n = (0, 0);
        for (_, c) in self.conns.iter().filter(|(_, c)| c.torrent == id && !c.connecting) {
            n.0 += 1;
            if c.all {
                n.1 += 1;
            }
        }
        n
    }

    fn of_torrent(&self, id: TorrentId) -> Vec<usize> {
        self.conns.iter().filter(|(_, c)| c.torrent == id).map(|(s, _)| s).collect()
    }

    pub fn any_read_blocked(&self) -> bool {
        self.conns.iter().any(|(_, c)| c.read_blocked)
    }
}

fn later(retry: &mut Option<Instant>, at: Instant) {
    *retry = Some(retry.map_or(at, |r| r.max(at)));
}

impl Engine {
    /// Disjoint borrows of a web seed connection and its torrent.
    fn web_and_torrent(&mut self, slot: usize) -> Option<(&mut WebConn, Option<&mut Torrent>)> {
        let Engine { web, slots, .. } = self;
        let c = web.conns.get_mut(slot)?;
        let id = c.torrent;
        let t = slots
            .get_mut(id.index() as usize)
            .filter(|s| s.generation == id.generation())
            .and_then(|s| s.torrent.as_deref_mut());
        Some((c, t))
    }

    fn url_seed_alert(&mut self, id: TorrentId, url: &str, message: impl Into<String>) {
        let message = message.into();
        self.alert(Alert::Log { id: Some(id), message: format!("web seed {url}: {message}") });
        self.alert(Alert::UrlSeed { id, url: url.to_owned(), message });
    }

    // --- The torrent API ----------------------------------------------------------------------

    /// libtorrent `torrent_handle::add_url_seed` / `add_http_seed`.
    pub(super) fn add_web_seed_cmd(&mut self, id: TorrentId, url: String, kind: WebSeedKind) {
        let Some(t) = self.torrent(id) else { return };
        let url = url.trim();
        if url.is_empty() {
            return;
        }
        t.web_seeds.add(url, kind, "", &[], false, false);
        t.resume_dirty = true;
    }

    /// libtorrent `torrent::remove_web_seed`: a seed being resolved goes once the lookup is
    /// back; a connected one is disconnected.
    pub(super) fn remove_web_seed_cmd(&mut self, id: TorrentId, url: &str, kind: WebSeedKind) {
        let Some(t) = self.torrent(id) else { return };
        let Some(i) = t.web_seeds.list.iter().position(|w| w.url == url && w.kind == kind && !w.removed)
        else {
            return;
        };
        t.resume_dirty = true;
        let w = &mut t.web_seeds.list[i];
        if w.resolving {
            w.removed = true;
        } else if let Some(slot) = w.conn {
            w.removed = true;
            self.remove_web_conn(slot, "web seed removed", Close::Normal);
        } else {
            t.web_seeds.list.remove(i);
        }
    }

    pub(super) fn post_web_seeds(&mut self, id: TorrentId) {
        let Some(t) = self.torrent_ref(id) else { return };
        let url_seeds = t.web_seeds.urls(WebSeedKind::UrlSeed);
        let http_seeds = t.web_seeds.urls(WebSeedKind::HttpSeed);
        self.alert(Alert::WebSeeds { id, url_seeds, http_seeds });
    }

    // --- Connecting ---------------------------------------------------------------------------

    /// libtorrent `torrent::maybe_connect_web_seeds`: while the torrent downloads, up to
    /// `max_web_seed_connections` usable seeds (0: 100) are connected.
    pub(super) fn maybe_connect_web_seeds(&mut self, id: TorrentId, now: Instant) {
        let Some(t) = self.torrent_ref(id) else { return };
        if t.web_seeds.list.is_empty()
            || t.picker.is_none()
            || t.checking
            || t.is_finished()
            || !self.is_running(t)
            || self.connections_full(id)
        {
            return;
        }
        let max = self.settings.max_web_seed_connections;
        let mut limit = if max <= 0 { 100 } else { max as usize };
        let mut connect = Vec::new();
        for w in &t.web_seeds.list {
            if limit == 0 {
                break;
            }
            if w.disabled || w.removed || w.retry.is_some_and(|r| r > now) || !w.interesting {
                continue;
            }
            limit -= 1;
            if w.conn.is_some() || w.resolving {
                continue;
            }
            connect.push(w.id);
        }
        for seed in connect {
            self.connect_to_url_seed(id, seed, now);
        }
    }

    /// The torrent's `max_connections` or the session's `connections_limit` is reached
    /// (web seed connections count toward both).
    fn connections_full(&self, id: TorrentId) -> bool {
        let Some(t) = self.torrent_ref(id) else { return true };
        let limit = self.settings.connections_limit;
        let num_peers = t.peers.len() + self.web.of_torrent(id).len();
        t.max_connections.is_some_and(|m| num_peers >= m as usize)
            || (limit >= 0 && self.peers.len() + self.web.len() >= limit as usize)
    }

    /// libtorrent `torrent::connect_to_url_seed`: checks the URL, then resolves the host.
    fn connect_to_url_seed(&mut self, id: TorrentId, seed: u32, now: Instant) {
        let allow_idna = self.settings.allow_idna;
        let port_filter = self.filters.port.clone();
        let Some(t) = self.torrent(id) else { return };
        let Some(i) = t.web_seeds.index_of(seed) else { return };
        let w = &mut t.web_seeds.list[i];
        let url = w.url.clone();
        let parts = match parse_url_components(&url) {
            Ok(p) => p,
            Err(_) => {
                // never try it again
                t.web_seeds.list.remove(i);
                return self.url_seed_alert(id, &url, "invalid URL");
            }
        };
        let proto = parts.protocol.to_ascii_lowercase();
        let port = parts.port.unwrap_or(if proto == "http" { 80 } else { 443 });
        let (problem, remove) = if !allow_idna && is_idna(&parts.host) {
            (Some("blocked by IDNA ban"), false)
        } else if w.banned {
            (Some("peer banned"), false)
        } else if proto != "http" && proto != "https" {
            (Some("unsupported URL protocol"), false)
        } else if parts.host.is_empty() {
            (Some("invalid hostname"), true)
        } else if port == 0 {
            (Some("invalid port"), true)
        } else if port_filter.is_blocked(port) {
            (Some("port blocked by port-filter"), false)
        } else {
            (None, false)
        };
        if let Some(message) = problem {
            if remove {
                t.web_seeds.list.remove(i);
            } else {
                // never try it again this session
                t.web_seeds.list[i].disabled = true;
            }
            return self.url_seed_alert(id, &url, message);
        }
        if let Some(&addr) = w.endpoints.first() {
            return self.connect_web_seed(id, seed, addr, None, now);
        }
        // through a proxy that is asked for the URL or resolves names, nothing is resolved here
        if let Some(cfg) = self.proxy.for_peers() {
            let target = Target::new(&parts.host, port);
            if cfg.plain_http(proto == "https") || cfg.resolves_names() || matches!(target, Target::Addr(_)) {
                return match target {
                    Target::Addr(a) => self.connect_web_seed(id, seed, a, None, now),
                    name => self.connect_web_seed(
                        id,
                        seed,
                        Self::proxied_placeholder(seed, port),
                        Some(name),
                        now,
                    ),
                };
            }
        }
        let Some(t) = self.torrent(id) else { return };
        let Some(i) = t.web_seeds.index_of(seed) else { return };
        let w = &mut t.web_seeds.list[i];
        w.resolving = true;
        self.web.next_cookie += 1;
        let cookie = WEB_COOKIE | self.web.next_cookie;
        self.web.waiting.insert(cookie, (id, seed, port));
        if let Some(ips) = self.resolver.resolve(&parts.host, cookie) {
            self.web.waiting.remove(&cookie);
            self.on_web_name_lookup(id, seed, port, Ok(ips), now);
        }
    }

    /// A host name lookup for a web seed is back.
    pub(super) fn on_web_resolved(&mut self, r: Resolved) {
        let Some((id, seed, port)) = self.web.waiting.remove(&r.cookie) else { return };
        let result = r.result.map_err(|e| format!("{}: {e}", r.host));
        self.on_web_name_lookup(id, seed, port, result, Instant::now());
    }

    /// libtorrent `torrent::on_name_lookup`.
    fn on_web_name_lookup(
        &mut self,
        id: TorrentId,
        seed: u32,
        port: u16,
        result: Result<Vec<IpAddr>, String>,
        now: Instant,
    ) {
        let lookup_retry = Duration::from_secs(self.settings.web_seed_name_lookup_retry as u64);
        let Some(t) = self.torrent(id) else { return };
        let Some(i) = t.web_seeds.index_of(seed) else { return };
        let active = t.is_active();
        let w = &mut t.web_seeds.list[i];
        w.resolving = false;
        if w.removed {
            t.web_seeds.list.remove(i);
            return;
        }
        if !active {
            return;
        }
        let url = w.url.clone();
        let ips = match result {
            Ok(ips) if !ips.is_empty() => ips,
            Ok(_) => Vec::new(),
            Err(e) => {
                // unavailable, retry in `web_seed_name_lookup_retry` seconds
                w.retry = Some(now + lookup_retry);
                return self.url_seed_alert(id, &url, e);
            }
        };
        if ips.is_empty() {
            w.retry = Some(now + lookup_retry);
            return self.url_seed_alert(id, &url, "host not found");
        }
        let no_local = w.no_local_ips;
        w.endpoints = ips
            .into_iter()
            .filter(|ip| !no_local || is_global(*ip))
            .map(|ip| SocketAddr::new(ip, port))
            .collect();
        let Some(&addr) = w.endpoints.first() else {
            // the name lookup failed for the http host. Don't try this host again
            w.disabled = true;
            return self.url_seed_alert(id, &url, "banned by IP filter");
        };
        if self.connections_full(id) {
            return;
        }
        self.connect_web_seed(id, seed, addr, None, now);
    }

    /// libtorrent `torrent::connect_web_seed`. `name` is the target a proxy resolves; `addr` is
    /// then a placeholder.
    fn connect_web_seed(
        &mut self,
        id: TorrentId,
        seed: u32,
        addr: SocketAddr,
        name: Option<Target>,
        now: Instant,
    ) {
        let wait = Duration::from_secs(self.settings.urlseed_wait_retry as u64);
        let ssrf = self.settings.ssrf_mitigation && name.is_none();
        let (user_agent, always_ua) =
            (self.settings.user_agent.clone(), self.settings.always_send_user_agent);
        let blocked = name.is_none() && self.filters.ip.is_blocked(addr.ip());
        let via = match self.peer_proxy() {
            None => None,
            Some(Ok(p)) => Some(p),
            Some(Err(error)) => {
                // the proxy's name is not resolved (yet)
                let Some(t) = self.torrent(id) else { return };
                let Some(i) = t.web_seeds.index_of(seed) else { return };
                let retry = if error.is_some() { wait } else { Duration::from_secs(1) };
                later(&mut t.web_seeds.list[i].retry, now + retry);
                return;
            }
        };
        let running = self.torrent_ref(id).is_some_and(|t| self.is_running(t));
        let Some(t) = self.torrent(id) else { return };
        let Some(i) = t.web_seeds.index_of(seed) else { return };
        if blocked && t.flags.contains(TorrentFlags::APPLY_IP_FILTER) {
            later(&mut t.web_seeds.list[i].retry, now + wait);
            return self.alert(Alert::PeerBlocked { id: Some(id), addr, reason: BlockReason::IpFilter });
        }
        let w = &mut t.web_seeds.list[i];
        w.addr = Some(addr);
        if !running || t.flags.contains(TorrentFlags::UPLOAD_MODE) {
            return;
        }
        // this web seed may have redirected all files to other URLs, leaving it having no file
        // left, and there's no longer any point in connecting to it
        if !w.have_files.is_empty() && w.have_files.none_set() {
            return;
        }
        let Some(ti) = t.ti.clone() else { return };
        let url = w.url.clone();
        let Ok(parts) = parse_url_components(&url) else { return };
        // The SSRF mitigation for web seeds is that any HTTP server on the local network may
        // not use any query string parameters
        if ssrf && is_local(addr.ip()) && parts.path.contains('?') {
            // never try it again
            w.disabled = true;
            self.url_seed_alert(id, &url, "blocked by SSRF mitigation");
            let reason = BlockReason::SsrfMitigation;
            return self.alert(Alert::PeerBlocked { id: Some(id), addr, reason });
        }
        let ih = t.info_hashes.best_v1_compatible();
        let proto =
            WebPeer::new(&url, w.kind, ih, &w.auth, &w.extra_headers, ti.files(), &user_agent, always_ua);
        let mut proto = match proto {
            Ok(p) => p,
            Err(_) => {
                let t = self.torrent(id).expect("exists");
                t.web_seeds.list[i].disabled = true;
                return self.url_seed_alert(id, &url, "invalid URL");
            }
        };
        let https = parts.protocol.eq_ignore_ascii_case("https");
        let mut tunnel = None;
        let mut connect_to = addr;
        if let Some((cfg, proxy_addr)) = &via {
            connect_to = *proxy_addr;
            if cfg.plain_http(https) {
                proto.set_http_proxy(cfg.creds());
            } else {
                match cfg.tunnel(&name.clone().unwrap_or(Target::Addr(addr))) {
                    Ok(hs) => tunnel = Some(Tunnel::new(hs)),
                    Err(e) => {
                        let t = self.torrent(id).expect("exists");
                        t.web_seeds.list[i].disabled = true;
                        return self.url_seed_alert(id, &url, format!("proxy: {e}"));
                    }
                }
            }
        }
        let sock = match TcpStream::connect(connect_to) {
            Ok(s) => s,
            Err(e) => {
                let t = self.torrent(id).expect("exists");
                let w = &mut t.web_seeds.list[i];
                if !w.endpoints.is_empty() {
                    w.endpoints.remove(0);
                }
                later(&mut w.retry, now + wait);
                return self.url_seed_alert(id, &url, format!("connect failed: {e}"));
            }
        };
        let _ = sock.set_nodelay(true);
        let stream = if https {
            match tls::client(&parts.host) {
                Ok(s) => Stream::Tls(Box::new(TlsStream::new(sock, s))),
                Err(e) => {
                    let t = self.torrent(id).expect("exists");
                    t.web_seeds.list[i].disabled = true;
                    return self.url_seed_alert(id, &url, format!("TLS: {e}"));
                }
            }
        } else {
            Stream::Plain(sock)
        };
        // nothing is requested before the connection is up
        proto.out().clear();
        let slot = self.web.conns.insert(WebConn {
            torrent: id,
            seed,
            addr,
            https,
            stream,
            connecting: true,
            tunnel,
            proto,
            have: Bitfield::default(),
            all: false,
            counted: false,
            am_interested: false,
            connected_at: now,
            last_receive: now,
            stat: Stat::default(),
            read_blocked: false,
            runs: Default::default(),
        });
        let Engine { web, poll, .. } = self;
        let c = web.conns.get_mut(slot).expect("inserted");
        let token = Token(WEB_TOKEN_BASE + slot);
        if let Err(e) =
            poll.registry().register(c.stream.tcp(), token, Interest::READABLE | Interest::WRITABLE)
        {
            self.web.conns.remove(slot);
            return self.url_seed_alert(id, &url, format!("register failed: {e}"));
        }
        let t = self.torrent(id).expect("exists");
        t.web_seeds.list[i].conn = Some(slot);
        t.status_dirty = true;
        self.alert(Alert::Log {
            id: Some(id),
            message: format!("web seed connection started: [{addr}] {url}"),
        });
    }

    // --- Connection events --------------------------------------------------------------------

    pub(super) fn on_web_event(&mut self, token: Token) {
        let slot = token.0 - WEB_TOKEN_BASE;
        let Some(c) = self.web.conns.get_mut(slot) else { return };
        if c.connecting {
            let tcp = c.stream.tcp();
            match (tcp.take_error(), tcp.peer_addr()) {
                (Ok(None), Ok(_)) => {}
                (Ok(None), Err(e)) if e.kind() == io::ErrorKind::NotConnected => return,
                (Ok(Some(e)), _) | (Err(e), _) | (_, Err(e)) => {
                    return self.remove_web_conn(slot, &format!("connect failed: {e}"), Close::ConnectFailed);
                }
            }
            // through a proxy: its handshake on the bare socket, before TLS or a request
            if let Some(t) = c.tunnel.as_mut() {
                match t.drive(c.stream.tcp()) {
                    Ok(None) => return,
                    Ok(Some(_)) => c.tunnel = None,
                    Err(e) => {
                        let (id, url) = (c.torrent, c.proto.url().to_owned());
                        self.proxy_connection_failed(&e);
                        self.url_seed_alert(id, &url, e.to_string());
                        return self.remove_web_conn(
                            slot,
                            &format!("connect failed: {e}"),
                            Close::ConnectFailed,
                        );
                    }
                }
            }
            self.web_on_connected(slot);
        }
        self.web_flush(slot);
        self.web_read(slot);
    }

    /// libtorrent `web_peer_connection::on_connected` (and `http_seed_connection`'s): the
    /// server has the pieces of the files it is known to have, pad files included.
    fn web_on_connected(&mut self, slot: usize) {
        let now = Instant::now();
        let Some((c, Some(t))) = self.web_and_torrent(slot) else {
            return self.remove_web_conn(slot, "torrent gone", Close::Normal);
        };
        c.connecting = false;
        c.connected_at = now;
        c.last_receive = now;
        let (Some(ti), Some(i)) = (t.ti.clone(), t.web_seeds.index_of(c.seed)) else {
            return self.remove_web_conn(slot, "torrent stopped", Close::Normal);
        };
        let w = &mut t.web_seeds.list[i];
        if w.have_files.is_empty() || w.kind == WebSeedKind::HttpSeed {
            c.all = true;
        } else {
            // start with all pieces, and clear those of files we don't have (pieces spanning
            // several files we have all stay)
            let fs = ti.files();
            let mut have = Bitfield::new(ti.num_pieces() as usize, true);
            for f in 0..fs.num_files() {
                // an empty file has nothing to serve (libtorrent clears the piece it sits in)
                if w.have_files.get(f) || fs.pad_file_at(f) || fs.file_size(f) == 0 {
                    continue;
                }
                let (first, end) = file_piece_range_inclusive(fs, f);
                for p in first..end.min(ti.num_pieces()) {
                    have.set(p as usize, false);
                }
            }
            if have.none_set() {
                w.interesting = false;
                return self.remove_web_conn(slot, "have no pieces, not interesting", Close::Uninteresting);
            }
            c.have = have;
        }
        if let Some(picker) = t.picker.as_mut() {
            if c.all {
                picker.inc_seed();
            } else {
                picker.inc_refcount_bitfield(&c.have);
            }
            c.counted = true;
            c.am_interested = picker.is_interesting(&c.have, c.all);
        }
        t.status_dirty = true;
        if !c.am_interested {
            return self.remove_web_conn(slot, "uninteresting upload peer", Close::Uninteresting);
        }
        self.web_request_blocks(slot);
    }

    /// Writes queued requests until the socket would block.
    fn web_flush(&mut self, slot: usize) {
        let Some(c) = self.web.conns.get_mut(slot) else { return };
        if c.connecting {
            return;
        }
        let mut sent = 0;
        let mut failed = None;
        // A TLS handshake runs inside `write`, so try even with nothing queued.
        loop {
            let out = c.proto.out();
            let r = if out.is_empty() {
                c.stream.flush().map(|()| 0)
            } else {
                c.stream.write(out).inspect(|&n| {
                    out.drain(..n);
                })
            };
            match r {
                Ok(0) => break,
                Ok(n) => sent += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                    // libtorrent: HTTP 1.0 servers close their read end after one request; keep
                    // reading what they send, but send nothing more
                    c.proto.out().clear();
                    c.proto.choked = true;
                    break;
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        c.stat.sent(0, sent as u64);
        let id = c.torrent;
        self.stat.sent(0, sent as u64);
        if let Some(t) = self.torrent(id) {
            t.stat.get_or_insert_with(Box::default).sent(0, sent as u64);
        }
        if let Some(e) = failed {
            self.remove_web_conn(slot, &format!("write error: {e}"), Close::Failure);
        }
    }

    /// Reads and parses what the server sent, within the download quota.
    fn web_read(&mut self, slot: usize) {
        let now = Instant::now();
        let mut buf = std::mem::take(&mut self.web.buf);
        buf.resize(READ_CHUNK, 0);
        loop {
            let backlogged = self.disk_backlogged();
            let session_quota = self.download_bucket.available();
            let Some((c, t)) = self.web_and_torrent(slot) else { break };
            if c.connecting {
                break;
            }
            let Some(t) = t else {
                self.remove_web_conn(slot, "torrent gone", Close::Normal);
                break;
            };
            let Some(ti) = t.ti.clone() else {
                self.remove_web_conn(slot, "torrent stopped", Close::Normal);
                break;
            };
            let quota = if backlogged { 0 } else { session_quota.min(t.download_limit.available()) };
            if quota == 0 {
                c.read_blocked = true;
                break;
            }
            c.read_blocked = false;
            let max = quota.min(READ_CHUNK);
            match c.stream.read(&mut buf[..max]) {
                Ok(0) => {
                    self.remove_web_conn(slot, "connection closed by the server", Close::Normal);
                    break;
                }
                Ok(n) => {
                    c.last_receive = now;
                    let payload = c.proto.feed(ti.files(), &buf[..n]);
                    let protocol = n as u64 - payload;
                    c.stat.received(payload, protocol);
                    t.stat.get_or_insert_with(Box::default).received(payload, protocol);
                    t.download_limit.consume(n);
                    self.download_bucket.consume(n);
                    self.stat.received(payload, protocol);
                    self.web_handle_events(slot);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.remove_web_conn(slot, &format!("read error: {e}"), Close::Failure);
                    break;
                }
            }
        }
        self.web.buf = buf;
        if self.web.conns.get(slot).is_some() {
            self.web_request_blocks(slot);
            self.web_flush(slot);
        }
    }

    /// Re-reads connections that waited for download quota or the disk.
    pub(super) fn web_unblock_reads(&mut self) {
        let blocked: Vec<usize> =
            self.web.conns.iter().filter(|(_, c)| c.read_blocked).map(|(s, _)| s).collect();
        for slot in blocked {
            self.web_read(slot);
        }
    }

    /// Acts on what the HTTP side made of the responses.
    fn web_handle_events(&mut self, slot: usize) {
        let Some(c) = self.web.conns.get_mut(slot) else { return };
        let events = c.proto.take_events();
        for e in events {
            if self.web.conns.get(slot).is_none() {
                return;
            }
            match e {
                WebEvent::Block { piece, start, data } => self.web_on_block(slot, piece, start, data),
                WebEvent::NoKeepAlive => {
                    if let Some((c, Some(t))) = self.web_and_torrent(slot) {
                        if let Some(i) = t.web_seeds.index_of(c.seed) {
                            t.web_seeds.list[i].supports_keepalive = false;
                        }
                    }
                }
                WebEvent::HttpError { status, message, retry_after, file } => {
                    self.web_http_error(slot, status, &message, retry_after, file)
                }
                WebEvent::Redirect { base, path, file } => self.web_redirect(slot, &base, &path, file),
                WebEvent::Disable(message) => self.web_disable(slot, message),
                WebEvent::Fail(message) => {
                    let (id, url) = {
                        let c = self.web.conns.get(slot).expect("checked");
                        (c.torrent, c.proto.url().to_owned())
                    };
                    self.url_seed_alert(id, &url, message);
                    self.remove_web_conn(slot, message, Close::Failure);
                }
            }
        }
    }

    /// libtorrent `peer_connection::incoming_piece` for a web seed's block.
    fn web_on_block(&mut self, slot: usize, piece: u32, start: u32, data: Vec<u8>) {
        let Some((c, Some(t))) = self.web_and_torrent(slot) else { return };
        if let Some(n) = c.runs.front_mut() {
            *n = n.saturating_sub(1);
            if *n == 0 {
                c.runs.pop_front();
            }
        }
        let len = data.len() as u64;
        let block = start / BLOCK_SIZE;
        let expected = (piece < t.num_pieces)
            .then(|| (t.piece_size(piece) as u32).saturating_sub(start).min(BLOCK_SIZE));
        let addr = c.addr;
        let Some(picker) =
            t.picker.as_mut().filter(|_| start % BLOCK_SIZE == 0 && expected == Some(len as u32))
        else {
            t.redundant_bytes += len;
            return;
        };
        if !picker.mark_as_writing(piece, block, addr) {
            t.redundant_bytes += len;
            return;
        }
        let Some(storage) = t.storage.clone() else { return };
        t.downloaded_in_session = true;
        t.status_dirty = true;
        let others = t.peers.clone();
        self.disk.async_write(&storage, piece, start, data);
        // end-game: cancel the same block at the BitTorrent peers we asked
        for s in others {
            let Some(o) = self.peers.get_mut(s) else { continue };
            if let Some(i) = o.requests.iter().position(|r| r.piece == piece && r.block == block) {
                o.requests.remove(i);
                peer_wire::encode_block_msg(o.out(), msg::CANCEL, piece, start, len as u32);
                self.to_flush.push(s);
            }
        }
    }

    /// libtorrent `request_a_block` + `send_block_requests` with `prefer_contiguous_blocks`
    /// and `request_large_blocks`: the rate-based queue (at least a piece's worth) is filled
    /// with contiguous runs of blocks, each run one HTTP request of at most
    /// `urlseed_max_request_bytes`; at most `urlseed_pipeline_size` requests are in flight (one
    /// when the server closes connections after each response).
    fn web_request_blocks(&mut self, slot: usize) {
        let s = &self.settings;
        let (queue_time, max_queue, pipeline, max_request) = (
            s.request_queue_time.max(1) as u64,
            s.max_out_request_queue.max(1) as usize,
            s.urlseed_pipeline_size.max(1) as usize,
            s.urlseed_max_request_bytes.max(BLOCK_SIZE as i32) as u64,
        );
        let Some((c, Some(t))) = self.web_and_torrent(slot) else { return };
        if c.connecting || c.proto.choked || c.proto.is_done() || !c.am_interested || t.checking {
            return;
        }
        let Torrent { picker, web_seeds, ti, .. } = t;
        let (Some(picker), Some(ti)) = (picker.as_mut(), ti.clone()) else { return };
        let Some(i) = web_seeds.index_of(c.seed) else { return };
        let w = &mut web_seeds.list[i];
        let max_runs = if w.supports_keepalive { pipeline } else { 1 };
        if c.runs.len() >= max_runs {
            return;
        }
        let piece_len = ti.piece_length() as u64;
        let blocks_per_piece = piece_len.div_ceil(BLOCK_SIZE as u64) as usize;
        let min_queue = blocks_per_piece.max(4);
        let rate = c.stat.download_payload.rate() as u64;
        let desired =
            ((rate * queue_time / BLOCK_SIZE as u64) as usize).clamp(min_queue, max_queue.max(min_queue));
        let outstanding = c.proto.num_requests();
        if outstanding >= desired {
            return;
        }
        let http_seed = w.kind == WebSeedKind::HttpSeed;
        // libtorrent: requests of at least a piece and 1 MiB (4 MiB without keep-alive), up to
        // `urlseed_max_request_bytes`; a BEP 17 seed takes one piece per request
        let contiguous = if http_seed {
            1
        } else {
            let min_size = (if w.supports_keepalive { 1u64 } else { 4 } << 20).max(piece_len);
            (min_size.max(max_request) / piece_len).max(1) as u32
        };
        let proto = &c.proto;
        let mut picks = picker.pick_contiguous(
            &c.have,
            c.all,
            desired - outstanding,
            contiguous,
            c.addr,
            w.trust.on_parole,
            |p, b| proto.requests().any(|r| r.piece == p && r.start == b * BLOCK_SIZE),
        );
        if picks.is_empty() {
            return;
        }
        picks.sort_unstable();
        // (piece, start, length, blocks) of each run
        let mut runs: Vec<(u32, u32, u64, usize)> = Vec::new();
        let mut next: Option<(u32, u32)> = None;
        for &(p, b) in &picks {
            let len = (ti.piece_size(p) as u64 - b as u64 * BLOCK_SIZE as u64).min(BLOCK_SIZE as u64);
            match runs.last_mut() {
                Some(run)
                    if next == Some((p, b)) && run.2 + len <= max_request && (!http_seed || run.0 == p) =>
                {
                    run.2 += len;
                    run.3 += 1;
                }
                _ => runs.push((p, b * BLOCK_SIZE, len, 1)),
            }
            next = Some(if b + 1 < picker.blocks_in_piece(p) { (p, b + 1) } else { (p + 1, 0) });
        }
        let room = max_runs - c.runs.len();
        for (n, &(piece, start, length, blocks)) in runs.iter().enumerate() {
            if n < room {
                c.proto.write_request(ti.files(), piece, start, length as u32, &w.redirects, &mut w.restart);
                c.runs.push_back(blocks);
            } else {
                let first = start / BLOCK_SIZE;
                let mut left = blocks;
                let (mut p, mut b) = (piece, first);
                while left > 0 {
                    picker.abort_download(p, b);
                    left -= 1;
                    (p, b) = if b + 1 < picker.blocks_in_piece(p) { (p, b + 1) } else { (p + 1, 0) };
                }
            }
        }
        // pad files of the new requests may be answered already
        self.web_handle_events(slot);
        self.web_flush(slot);
    }

    // --- Responses that end the connection ----------------------------------------------------

    /// libtorrent `handle_error` and the missing-file mark of `on_receive`.
    fn web_http_error(
        &mut self,
        slot: usize,
        status: i32,
        message: &str,
        retry_after: Option<u32>,
        file: Option<usize>,
    ) {
        let now = Instant::now();
        let wait = self.settings.urlseed_wait_retry;
        let Some((c, Some(t))) = self.web_and_torrent(slot) else { return };
        let (id, url) = (c.torrent, c.proto.url().to_owned());
        let num_files = t.ti.as_ref().map_or(0, |ti| ti.num_files());
        if let Some(i) = t.web_seeds.index_of(c.seed) {
            let w = &mut t.web_seeds.list[i];
            // temporarily unavailable is not "missing" (libtorrent marks the file either way)
            let temporary = matches!(status, 503 | 429);
            if let (false, Some(f)) = (temporary, file) {
                if w.have_files.is_empty() {
                    w.have_files.resize(num_files, true);
                }
                if f < w.have_files.len() {
                    w.have_files.set(f, false);
                }
            }
            // libtorrent `retry_web_seed`
            if !w.removed && !w.disabled {
                w.retry = Some(now + Duration::from_secs(retry_after.unwrap_or(wait) as u64));
            }
        }
        self.url_seed_alert(id, &url, format!("{status} {message}"));
        self.remove_web_conn(slot, &format!("HTTP {status} {message}"), Close::Failure);
    }

    /// libtorrent `handle_redirect` (`http_seed_connection`'s for BEP 17): the location
    /// becomes an ephemeral web seed. For a multi-file torrent it has this one file (more may
    /// follow), which this server is then known not to have; a single file or a BEP 17 seed
    /// moves as a whole.
    fn web_redirect(&mut self, slot: usize, base: &str, path: &str, file: Option<usize>) {
        let ssrf = self.settings.ssrf_mitigation;
        let Some((c, Some(t))) = self.web_and_torrent(slot) else { return };
        let num_files = t.ti.as_ref().map_or(0, |ti| ti.num_files());
        // when SSRF mitigation is enabled, a web seed on the internet is not allowed to
        // redirect to a server on the local network
        let no_local = ssrf && is_global(c.addr.ip());
        let Some(i) = t.web_seeds.index_of(c.seed) else { return };
        let (kind, auth, headers) = {
            let w = &t.web_seeds.list[i];
            (w.kind, w.auth.clone(), w.extra_headers.clone())
        };
        let id = c.torrent;
        let message = format!("redirected to {base}{path}");
        match (kind, file) {
            (WebSeedKind::HttpSeed, _) => {
                t.web_seeds.add(base, WebSeedKind::HttpSeed, "", &[], true, no_local);
                return self.web_disable(slot, "redirecting");
            }
            (WebSeedKind::UrlSeed, Some(f)) => {
                let before = t.web_seeds.list.len();
                let ni = t.web_seeds.add(base, kind, &auth, &headers, true, no_local);
                let new = &mut t.web_seeds.list[ni];
                // the new web seed only has this file for now; one that already had every
                // file keeps them
                if ni >= before || !new.have_files.is_empty() {
                    new.have_files.resize(num_files, false);
                }
                new.redirects.insert(f, path.to_owned());
                let mut learned = None;
                if f < new.have_files.len() && !new.have_files.get(f) {
                    new.have_files.set(f, true);
                    // we just learned about another file this web server has
                    new.interesting = true;
                    learned = new.conn;
                }
                // we don't have this file on this server. Don't ask for it again
                let w = &mut t.web_seeds.list[i];
                if w.have_files.is_empty() {
                    w.have_files.resize(num_files, true);
                }
                if f < w.have_files.len() {
                    w.have_files.set(f, false);
                }
                t.resume_dirty = true;
                if let Some(other) = learned {
                    self.web_incoming_have_file(other, f);
                }
            }
            (WebSeedKind::UrlSeed, None) => {
                t.web_seeds.add(base, kind, &auth, &headers, true, no_local);
                // this web seed doesn't have any files. Don't try to request from it again
                // this session
                let w = &mut t.web_seeds.list[i];
                w.have_files = Bitfield::new(num_files, false);
            }
        }
        self.alert(Alert::Log { id: Some(id), message: format!("web seed {}: {message}", base) });
        self.remove_web_conn(slot, "redirecting", Close::Normal);
    }

    /// A connected web seed turns out to have one more file (libtorrent `incoming_have` for
    /// the file's pieces).
    fn web_incoming_have_file(&mut self, slot: usize, file: usize) {
        let Some((c, Some(t))) = self.web_and_torrent(slot) else { return };
        if c.all || c.connecting {
            return;
        }
        let (Some(ti), Some(picker)) = (t.ti.clone(), t.picker.as_mut()) else { return };
        let (first, end) = file_piece_range_inclusive(ti.files(), file);
        for p in first..end.min(ti.num_pieces()) {
            if !c.have.get(p as usize) {
                c.have.set(p as usize, true);
                if c.counted {
                    picker.inc_refcount(p);
                }
            }
        }
        c.am_interested = picker.is_interesting(&c.have, c.all);
    }

    /// libtorrent `web_peer_connection::disable`: never try this server again this session; an
    /// ephemeral one is forgotten.
    fn web_disable(&mut self, slot: usize, message: &str) {
        let Some((c, Some(t))) = self.web_and_torrent(slot) else { return };
        let (id, url) = (c.torrent, c.proto.url().to_owned());
        if let Some(i) = t.web_seeds.index_of(c.seed) {
            let w = &mut t.web_seeds.list[i];
            w.disabled = true;
            if w.ephemeral {
                w.removed = true;
            }
        }
        self.url_seed_alert(id, &url, message);
        self.remove_web_conn(slot, message, Close::Failure);
    }

    /// libtorrent `web_peer_connection::disconnect` and `peer_connection::disconnect`: the
    /// picker forgets the server's pieces and requests, a partly received block is kept for
    /// the next connection, and the seed waits before reconnecting after a failure.
    fn remove_web_conn(&mut self, slot: usize, reason: &str, close: Close) {
        let wait = Duration::from_secs(self.settings.urlseed_wait_retry as u64);
        let Some(mut c) = self.web.conns.remove(slot) else { return };
        let _ = self.poll.registry().deregister(c.stream.tcp());
        let now = Instant::now();
        let Some(t) = self.torrent(c.torrent) else { return };
        if let Some(picker) = t.picker.as_mut() {
            if c.counted {
                if c.all {
                    picker.dec_seed();
                } else {
                    picker.dec_refcount_bitfield(&c.have);
                }
            }
            for r in c.proto.requests() {
                picker.abort_download(r.piece, r.start / BLOCK_SIZE);
            }
        }
        let restart = c.proto.take_restart();
        t.status_dirty = true;
        let mut url = c.proto.url().to_owned();
        if let Some(i) = t.web_seeds.index_of(c.seed) {
            let w = &mut t.web_seeds.list[i];
            url = w.url.clone();
            w.conn = None;
            let mut wasted = 0;
            if let Some(r) = restart {
                // replacing a different restart block wastes it
                wasted = w.restart.replace(r).map_or(0, |old| old.data.len() as u64);
            }
            match close {
                Close::ConnectFailed => {
                    // the next attempt uses the next address
                    if !w.endpoints.is_empty() {
                        w.endpoints.remove(0);
                    }
                    later(&mut w.retry, now + wait);
                }
                Close::Uninteresting => {
                    // an ephemeral seed (from a redirect) we have no use for is not tried again;
                    // a permanent one implies all its files failed or moved elsewhere
                    if w.ephemeral {
                        w.interesting = false;
                    }
                    later(&mut w.retry, now + wait);
                }
                Close::Failure => later(&mut w.retry, now + wait),
                // a server without keep-alive closed after its response: reconnect at once
                Close::Normal => {}
            }
            if w.removed {
                t.web_seeds.list.remove(i);
            }
            t.redundant_bytes += wasted;
        }
        let id = c.torrent;
        self.alert(Alert::Log { id: Some(id), message: format!("web seed {url} [{}]: {reason}", c.addr) });
    }

    /// Closes a torrent's web seed connections (the torrent stopped or finished).
    pub(super) fn close_web_seeds(&mut self, id: TorrentId, reason: &str) {
        for slot in self.web.of_torrent(id) {
            self.remove_web_conn(slot, reason, Close::Normal);
        }
    }

    /// Once per tick: timeouts (libtorrent `web_connection_base::timeout` is
    /// `urlseed_timeout`), redundant connections, interest, and new requests; then new
    /// connections for the running torrents.
    pub(super) fn web_tick(&mut self, now: Instant, dt: f64, running: &[TorrentId]) {
        let connect_timeout = Duration::from_secs(self.settings.peer_connect_timeout as u64);
        let timeout = Duration::from_secs(self.settings.urlseed_timeout.max(1) as u64);
        let slots: Vec<usize> = self.web.conns.iter().map(|(s, _)| s).collect();
        for slot in slots {
            let Some((c, t)) = self.web_and_torrent(slot) else { continue };
            c.stat.tick(dt);
            if c.connecting {
                if now.duration_since(c.connected_at) > connect_timeout {
                    self.remove_web_conn(slot, "connect timed out", Close::ConnectFailed);
                }
                continue;
            }
            let Some(t) = t.filter(|t| t.is_active() && t.picker.is_some()) else {
                self.remove_web_conn(slot, "torrent stopped", Close::Normal);
                continue;
            };
            // libtorrent `disconnect_if_redundant`: we are done downloading
            if t.is_finished() {
                self.remove_web_conn(slot, "torrent finished", Close::Normal);
                continue;
            }
            if now.duration_since(c.last_receive) > timeout {
                self.remove_web_conn(slot, "timed out", Close::Failure);
                continue;
            }
            if !t.checking {
                let interested = t.picker.as_ref().is_some_and(|p| p.is_interesting(&c.have, c.all));
                c.am_interested = interested;
                if !interested {
                    self.remove_web_conn(slot, "uninteresting upload peer", Close::Uninteresting);
                    continue;
                }
                self.web_request_blocks(slot);
            }
        }
        for &id in running {
            self.maybe_connect_web_seeds(id, now);
        }
    }

    // --- Blame --------------------------------------------------------------------------------

    /// The web seed that sent data from `addr`: the one connected from it, else the last one
    /// that was.
    fn web_seed_at(t: &Torrent, addr: SocketAddr) -> Option<usize> {
        let l = &t.web_seeds.list;
        l.iter()
            .position(|w| w.addr == Some(addr) && w.conn.is_some())
            .or_else(|| l.iter().position(|w| w.addr == Some(addr)))
    }

    /// The web seed branch of libtorrent `torrent::piece_failed`: the server lost trust, and
    /// `received_invalid_data` decides whether being the only sender gets it banned. Returns
    /// false if `addr` is not a web seed.
    pub(super) fn web_seed_hash_failed(
        &mut self,
        id: TorrentId,
        piece: u32,
        addr: SocketAddr,
        known_bad_peer: bool,
        use_parole_mode: bool,
    ) -> bool {
        let ban_web_seeds = self.settings.ban_web_seeds;
        let Some(t) = self.torrent(id) else { return false };
        let Some(i) = Self::web_seed_at(t, addr) else { return false };
        let seed = t.web_seeds.list[i].id;
        let allow_disconnect = !known_bad_peer || self.web_received_invalid_data(id, seed, piece);
        let Some(t) = self.torrent(id) else { return true };
        let Some(i) = t.web_seeds.index_of(seed) else { return true };
        let ban = t.web_seeds.list[i].trust.failed(known_bad_peer && allow_disconnect, use_parole_mode);
        // libtorrent `torrent::ban_peer` refuses web seeds when `ban_web_seeds` is off
        if ban && ban_web_seeds {
            self.ban_web_seed(id, seed, "too many corrupt pieces");
        }
        true
    }

    /// libtorrent `web_peer_connection::received_invalid_data` for the only sender of a bad
    /// piece: a single-file torrent may ban the server at once; otherwise the server is taken
    /// to hold another version of the file (a piece inside one file) or of the piece, and
    /// stops claiming it. It may be banned once it claims nothing.
    fn web_received_invalid_data(&mut self, id: TorrentId, seed: u32, piece: u32) -> bool {
        let Some(t) = self.torrent(id) else { return true };
        let Some(slot) = t.web_seeds.index_of(seed).and_then(|i| t.web_seeds.list[i].conn) else {
            return true;
        };
        let Some(ti) = t.ti.clone() else { return true };
        let fs = ti.files();
        if fs.num_files() == 1 {
            return true;
        }
        let files = fs.map_block(piece, 0, ti.piece_size(piece) as u64);
        let pieces: Vec<u32> = if files.len() == 1 {
            let (first, end) = file_piece_range_inclusive(fs, files[0].file_index);
            (first..end.min(ti.num_pieces())).collect()
        } else {
            vec![piece]
        };
        let Some((c, Some(t))) = self.web_and_torrent(slot) else { return true };
        let Some(picker) = t.picker.as_mut() else { return true };
        if c.all {
            // no longer a seed: count it as a peer with every piece, then take some away
            c.all = false;
            c.have = Bitfield::new(ti.num_pieces() as usize, true);
            if c.counted {
                picker.dec_seed();
                picker.inc_refcount_bitfield(&c.have);
            }
        }
        for p in pieces {
            if c.have.get(p as usize) {
                c.have.set(p as usize, false);
                if c.counted {
                    picker.dec_refcount(p);
                }
            }
        }
        c.have.none_set()
    }

    /// libtorrent `piece_passed` for web seeds: the senders regain trust and leave parole.
    pub(super) fn web_seeds_passed(&mut self, id: TorrentId, senders: &HashSet<SocketAddr>) {
        let Some(t) = self.torrent(id) else { return };
        for w in &mut t.web_seeds.list {
            if w.addr.is_some_and(|a| senders.contains(&a)) {
                w.trust.passed();
            }
        }
    }

    /// libtorrent `torrent::ban_peer` for a web seed: the connection is closed, and the seed is
    /// never connected again (it disappears from the web seed list).
    fn ban_web_seed(&mut self, id: TorrentId, seed: u32, reason: &str) {
        let Some(t) = self.torrent(id) else { return };
        let Some(i) = t.web_seeds.index_of(seed) else { return };
        let w = &mut t.web_seeds.list[i];
        if w.banned {
            return;
        }
        w.banned = true;
        let (url, addr, conn) = (w.url.clone(), w.addr, w.conn);
        t.resume_dirty = true;
        if let Some(slot) = conn {
            self.remove_web_conn(slot, &format!("banned: {reason}"), Close::Failure);
        }
        self.alert(Alert::Log { id: Some(id), message: format!("banning web seed {url}: {reason}") });
        if let Some(addr) = addr {
            self.alert(Alert::PeerBanned { id, addr, peer_id: [0; 20] });
        }
    }

    // --- Status -------------------------------------------------------------------------------

    /// libtorrent `get_peer_info` for web seed connections (`get_specific_peer_info`).
    pub(super) fn web_peer_infos(&self, id: TorrentId) -> Vec<PeerInfo> {
        let Some(t) = self.torrent_ref(id) else { return Vec::new() };
        let num_pieces = t.num_pieces.max(1) as u64;
        let mut out = Vec::new();
        for (_, c) in self.web.conns.iter().filter(|(_, c)| c.torrent == id) {
            let seed = t.web_seeds.index_of(c.seed).map(|i| &t.web_seeds.list[i]);
            let trust = seed.map_or(Trust::default(), |w| w.trust);
            let mut flags = PeerFlags::empty();
            flags.set(PeerFlags::INTERESTING, c.am_interested);
            flags.set(PeerFlags::CHOKED, true);
            flags.set(PeerFlags::REMOTE_CHOKED, c.proto.choked);
            flags.set(PeerFlags::OUTGOING_CONNECTION, true);
            flags.set(PeerFlags::CONNECTING, c.connecting);
            flags.set(PeerFlags::HANDSHAKE, !c.connecting && c.proto.server().is_empty());
            flags.set(PeerFlags::SEED, c.all);
            flags.set(PeerFlags::UPLOAD_ONLY, true);
            flags.set(PeerFlags::ON_PAROLE, trust.on_parole);
            flags.set(PeerFlags::SSL_SOCKET, c.https);
            let have = if c.all { num_pieces } else { c.have.count() as u64 };
            out.push(PeerInfo {
                ip: c.addr,
                client: c.proto.server().to_owned(),
                flags,
                source: PeerSource::empty(),
                progress_ppm: (have * 1_000_000 / num_pieces) as u32,
                down_speed: c.stat.download_payload.rate(),
                up_speed: 0,
                total_download: c.stat.download_payload.total,
                total_upload: 0,
                download_queue_length: c.proto.num_requests() as u32,
                upload_queue_length: 0,
                num_hashfails: trust.hashfails as u32,
                is_web_seed: true,
                url: seed.map(|w| w.url.clone()),
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_global_addresses() {
        // after libtorrent test_enum_net.cpp's is_local / is_global cases
        for (a, local) in [
            ("192.168.0.1", true),
            ("10.1.1.56", true),
            ("172.16.4.3", true),
            ("172.32.4.3", false),
            ("127.0.0.1", true),
            ("100.64.0.1", true),
            ("169.254.1.1", true),
            ("8.8.8.8", false),
            ("::1", true),
            ("fe80::1", true),
            ("fd00::1", true),
            ("2001:db8::1", false),
            ("::ffff:10.0.0.1", true),
        ] {
            assert_eq!(is_local(a.parse().unwrap()), local, "{a}");
        }
        for (a, global) in [
            ("8.8.8.8", true),
            ("10.0.0.1", false),
            ("0.0.0.0", false),
            ("224.0.0.1", false),
            ("2001:db8::1", true),
            ("fd00::1", false),
            ("::1", false),
        ] {
            assert_eq!(is_global(a.parse().unwrap()), global, "{a}");
        }
        assert!(is_idna("xn--bcher-kva.example"));
        assert!(is_idna("www.XN--bcher-kva.example"));
        assert!(!is_idna("example.com"));
        assert!(!is_idna("xn-.com"));
    }

    #[test]
    fn web_seed_list() {
        let mut l = WebSeeds::default();
        let a = l.add("http://a/", WebSeedKind::UrlSeed, "", &[], true, false);
        assert_eq!(l.add("http://a/", WebSeedKind::UrlSeed, "", &[], false, false), a, "no duplicates");
        assert!(!l.list[a].ephemeral, "promoted to permanent");
        let b = l.add("http://b/", WebSeedKind::UrlSeed, "", &[], true, false);
        l.add_urls(&["  ".to_owned(), " http://h/s ".to_owned()], WebSeedKind::HttpSeed);
        assert_eq!(l.urls(WebSeedKind::UrlSeed), ["http://a/", "http://b/"]);
        assert_eq!(l.urls(WebSeedKind::HttpSeed), ["http://h/s"]);
        assert_eq!(l.resume_urls(WebSeedKind::UrlSeed), ["http://a/"], "ephemeral ones are not saved");
        l.list[b].banned = true;
        assert_eq!(l.urls(WebSeedKind::UrlSeed), ["http://a/"]);
        let id = l.list[a].id;
        assert_eq!(l.index_of(id), Some(a));
        assert_ne!(l.list[a].id, l.list[b].id);
    }
}
