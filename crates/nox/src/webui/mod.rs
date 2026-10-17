//! The Web UI server (PLAN.md Phase 6, D18): an embedded HTTP/1.1 server with the native push
//! API (`/pa/ws`, a WebSocket carrying the session protocol of `docs/WEBUI.md`) and, as a
//! compatibility shim (D10), a subset of the qBittorrent WebAPI v2 written from qBittorrent's
//! published WebAPI documentation only.
//!
//! Threads: one acceptor, one thread per connection (at most [`MAX_CONNECTIONS`]); a WebSocket
//! keeps its connection thread. They never touch the engine directly. The owner of the
//! session's alert queue (nox's main loop) hands every batch of alerts to [`WebUi::on_alerts`],
//! which keeps a cache of torrent status, session stats and replies to `post_*` requests and
//! counts a status tick per batch; requests read that cache and post commands through the
//! [`SessionHandle`], WebSocket threads send what changed since their last tick.

mod api;
pub mod auth;
mod fetch;
pub mod http;
pub mod json;
pub mod native;
mod server;
pub mod torrent;
pub mod ws;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use patorrent_core::{
    Alert, AnnounceEntry, Bitfield, FileEntry, PeerInfo, SessionHandle, SessionStats, Settings, TorrentId,
    TorrentStatus,
};

use auth::{BanList, PasswordHash};
use json::Json;

/// Most connections served at once; more are answered 503 and closed.
pub const MAX_CONNECTIONS: usize = 64;
/// Most logged-in sessions kept; the least recently used is dropped beyond this.
pub const MAX_SESSIONS: usize = 256;
/// How long a request waits for the engine to answer a `post_*` call.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// The name of the state file in the state directory.
pub const STATE_FILE: &str = "webui.json";
/// Log lines kept for the Logger tab (the alert loop's lines, session-wide and per torrent).
pub const LOG_LINES: usize = 500;
/// Speed samples kept per watched torrent: 10 minutes at the engine's one status tick a second.
pub const SPEED_SAMPLES: usize = 600;
/// A speed ring no client looked at for this many ticks is dropped.
const SPEED_IDLE_TICKS: u64 = 600;

/// How to start the Web UI.
#[derive(Clone, Debug)]
pub struct Config {
    pub addr: SocketAddr,
    pub username: String,
    /// A new password (from the command line): hashed and, with a state directory, stored as
    /// the hash. `None` keeps the stored one; with none stored, a random temporary password is
    /// made and shown once ([`WebUi::temporary_password`]).
    pub password: Option<String>,
    pub state_dir: Option<PathBuf>,
    /// The default save path for added torrents.
    pub save_path: String,
    /// The settings the session was started with; the Web UI changes them through
    /// `apply_settings`.
    pub settings: Settings,
    pub pbkdf2_iterations: u32,
    pub limits: http::Limits,
}

impl Config {
    pub fn new(addr: SocketAddr, settings: Settings, save_path: impl Into<String>) -> Config {
        Config {
            addr,
            username: "admin".into(),
            password: None,
            state_dir: None,
            save_path: save_path.into(),
            settings,
            pbkdf2_iterations: auth::DEFAULT_ITERATIONS,
            limits: http::Limits::default(),
        }
    }
}

/// Parses `[ADDR:]PORT` (`8080`, `0.0.0.0:8080`, `[::1]:8080`); the address defaults to
/// 127.0.0.1.
pub fn parse_addr(s: &str) -> Result<SocketAddr, String> {
    if let Ok(port) = s.parse::<u16>() {
        return Ok(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    s.parse().map_err(|_| format!("--webui {s}: expected [ADDR:]PORT"))
}

/// Preferences that live in the Web UI rather than in the engine's [`Settings`].
#[derive(Clone, Debug)]
pub(crate) struct Prefs {
    pub save_path: String,
    /// Bytes per second, 0 = unlimited: the normal and the alternative global limits.
    pub dl_limit: u32,
    pub up_limit: u32,
    pub alt_dl_limit: u32,
    pub alt_up_limit: u32,
    pub alt_enabled: bool,
    pub start_paused: bool,
    pub bypass_local_auth: bool,
    pub csrf_protection: bool,
    pub host_header_validation: bool,
    /// `;`-separated host names accepted in `Host` besides IP addresses and `localhost`
    /// (`*` wildcards allowed).
    pub domain_list: String,
    pub max_auth_fail_count: u32,
    pub ban_duration: u64,
    pub session_timeout: u64,
}

/// One logged-in client.
pub(crate) struct ClientSession {
    pub last_used: Instant,
    pub maindata: Option<api::Snapshot>,
    pub peers: Option<api::PeersSnapshot>,
}

/// A cached torrent.
pub(crate) struct Entry {
    pub status: TorrentStatus,
    pub hash: String,
    /// Bumped on every change, so `sync/maindata` diffs only changed torrents.
    pub version: u64,
    pub last_activity: i64,
}

/// What a `post_*` call answered.
#[derive(Clone)]
pub(crate) enum Reply {
    Files(Arc<[FileEntry]>),
    Progress(Vec<u64>),
    Trackers(Vec<AnnounceEntry>),
    /// Our pieces, the availability per piece, the pieces being downloaded.
    Pieces(Bitfield, Vec<u32>, Vec<u32>),
    Peers(Vec<PeerInfo>),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum ReplyKind {
    Files,
    Progress,
    Trackers,
    Pieces,
    Peers,
}

/// One Logger line: the alert's time, the torrent it concerns (none for session-wide ones) and
/// the text, as `patorrent-nox` prints it.
pub(crate) struct LogLine {
    pub seq: u64,
    pub time: i64,
    pub torrent: Option<TorrentId>,
    pub text: String,
}

/// The Speed tab's history of one torrent, sampled once per status tick while some client
/// looks at it ([`State::watch_speed`]), and for [`SPEED_IDLE_TICKS`] after.
pub(crate) struct SpeedRing {
    /// (download, upload) payload bytes per second, oldest first.
    pub samples: VecDeque<(u32, u32)>,
    /// Samples pushed so far; the newest has this sequence number.
    pub pushed: u64,
    /// The tick a client last asked for it.
    pub last_seen: u64,
}

/// Torrent metadata the engine status does not carry, for torrents added through the API.
#[derive(Clone, Debug, Default)]
pub(crate) struct Meta {
    pub comment: String,
    pub created_by: String,
    pub creation_date: Option<i64>,
    pub private: bool,
}

pub(crate) struct State {
    pub torrents: HashMap<TorrentId, Entry>,
    pub by_hash: HashMap<String, TorrentId>,
    pub next_version: u64,
    pub stats: SessionStats,
    pub settings: Settings,
    pub prefs: Prefs,
    pub username: String,
    pub password: PasswordHash,
    /// Category name -> save path.
    pub categories: BTreeMap<String, String>,
    pub categories_version: u64,
    /// Torrent hash -> category.
    pub torrent_categories: HashMap<String, String>,
    pub meta: HashMap<String, Meta>,
    /// Preferences set through `setPreferences`, re-applied on the next start.
    pub stored_prefs: Json,
    replies: HashMap<(TorrentId, ReplyKind), (u64, Reply)>,
    serial: u64,
    pub sessions: HashMap<String, ClientSession>,
    pub bans: BanList,
    pub shutdown: bool,
    /// Status ticks so far: one per alert batch that carried a state update or session stats.
    /// WebSocket threads send a `tick` when it moved past the one they last sent.
    pub tick: u64,
    /// Torrents removed so far, so a client's list only scans for removals when there were any.
    pub removals: u64,
    /// The last [`LOG_LINES`] alert lines, oldest first.
    pub log: VecDeque<LogLine>,
    log_seq: u64,
    /// Speed history per torrent some client is (or was lately) looking at.
    pub speed: HashMap<TorrentId, SpeedRing>,
}

pub(crate) struct Shared {
    pub handle: SessionHandle,
    pub state: Mutex<State>,
    replied: Condvar,
    pub limits: http::Limits,
    pub iterations: u32,
    pub state_file: Option<PathBuf>,
}

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Shared {
    pub fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking request thread must not take the whole Web UI down with it.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Posts a request with `post` and waits for the engine's answer.
    pub fn query(&self, id: TorrentId, kind: ReplyKind, post: impl FnOnce(&SessionHandle)) -> Option<Reply> {
        let start = self.lock().serial;
        post(&self.handle);
        let deadline = Instant::now() + REPLY_TIMEOUT;
        let mut st = self.lock();
        loop {
            if let Some((serial, r)) = st.replies.get(&(id, kind)) {
                if *serial > start {
                    return Some(r.clone());
                }
            }
            let now = Instant::now();
            if now >= deadline || !st.torrents.contains_key(&id) {
                return None;
            }
            st = self.replied.wait_timeout(st, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Writes the state file (credentials as a hash, stored preferences, categories).
    pub fn save(&self, st: &State) {
        let Some(path) = &self.state_file else { return };
        let mut cats = Json::obj();
        for (name, path) in &st.categories {
            cats.set(name, path.as_str());
        }
        let mut tc = Json::obj();
        let mut assigned: Vec<_> = st.torrent_categories.iter().collect();
        assigned.sort();
        for (hash, name) in assigned {
            tc.set(hash, name.as_str());
        }
        let doc = Json::obj()
            .with("username", st.username.as_str())
            .with("password", st.password.to_string())
            .with("preferences", st.stored_prefs.clone())
            .with("categories", cats)
            .with("torrent_categories", tc);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Write then rename, so a crash never leaves half a file.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, doc.to_json()).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// Applies the global limits for the current alternative-speed mode.
    pub fn apply_limits(&self, st: &mut State) {
        let (dl, up) = if st.prefs.alt_enabled {
            (st.prefs.alt_dl_limit, st.prefs.alt_up_limit)
        } else {
            (st.prefs.dl_limit, st.prefs.up_limit)
        };
        st.settings.download_rate_limit = dl;
        st.settings.upload_rate_limit = up;
        self.handle.apply_settings(st.settings.clone());
    }
}

/// The running Web UI. Dropping it stops the server.
pub struct WebUi {
    shared: Arc<Shared>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
    temporary_password: Option<String>,
}

impl WebUi {
    /// Binds `config.addr` and starts serving.
    pub fn start(handle: SessionHandle, config: Config) -> std::io::Result<WebUi> {
        let state_file = config.state_dir.as_ref().map(|d| d.join(STATE_FILE));
        let stored = state_file
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| json::parse(&s).ok())
            .unwrap_or_else(Json::obj);

        let mut temporary_password = None;
        let password = match (&config.password, stored.get("password").and_then(Json::as_str)) {
            (Some(p), _) => PasswordHash::new(p, config.pbkdf2_iterations),
            (None, Some(h)) if PasswordHash::parse(h).is_some() => PasswordHash::parse(h).expect("checked"),
            _ => {
                let p = auth::random_hex(6);
                let h = PasswordHash::new(&p, config.pbkdf2_iterations);
                temporary_password = Some(p);
                h
            }
        };
        // A stored user name is the one set through setPreferences; the command line wins when
        // it gives a password too.
        let username = match stored.get("username").and_then(Json::as_str) {
            Some(u) if config.password.is_none() && temporary_password.is_none() => u.to_owned(),
            _ => config.username.clone(),
        };
        let mut categories = BTreeMap::new();
        for (name, path) in stored.get("categories").map(Json::entries).unwrap_or_default() {
            categories.insert(name.clone(), path.as_str().unwrap_or_default().to_owned());
        }
        let mut torrent_categories = HashMap::new();
        for (hash, name) in stored.get("torrent_categories").map(Json::entries).unwrap_or_default() {
            if let Some(name) = name.as_str() {
                torrent_categories.insert(hash.clone(), name.to_owned());
            }
        }
        let settings = config.settings.clone();
        let prefs = Prefs {
            save_path: config.save_path.clone(),
            dl_limit: settings.download_rate_limit,
            up_limit: settings.upload_rate_limit,
            alt_dl_limit: 10 * 1024,
            alt_up_limit: 10 * 1024,
            alt_enabled: false,
            start_paused: false,
            bypass_local_auth: false,
            csrf_protection: true,
            host_header_validation: true,
            domain_list: String::new(),
            max_auth_fail_count: 5,
            ban_duration: 3600,
            session_timeout: 3600,
        };
        let listener = std::net::TcpListener::bind(config.addr)?;
        let addr = listener.local_addr()?;
        let stored_prefs = match stored.get("preferences") {
            Some(p @ Json::Obj(_)) => p.clone(),
            _ => Json::obj(),
        };
        let shared = Arc::new(Shared {
            handle,
            state: Mutex::new(State {
                torrents: HashMap::new(),
                by_hash: HashMap::new(),
                next_version: 1,
                stats: SessionStats::default(),
                settings,
                prefs,
                username,
                password,
                categories,
                categories_version: 1,
                torrent_categories,
                meta: HashMap::new(),
                stored_prefs: Json::obj(),
                replies: HashMap::new(),
                serial: 0,
                sessions: HashMap::new(),
                bans: BanList::default(),
                shutdown: false,
                tick: 0,
                removals: 0,
                log: VecDeque::new(),
                log_seq: 0,
                speed: HashMap::new(),
            }),
            replied: Condvar::new(),
            limits: config.limits,
            iterations: config.pbkdf2_iterations,
            state_file,
        });
        {
            let mut st = shared.lock();
            if !stored_prefs.entries().is_empty() {
                api::apply_preferences(&shared, &mut st, &stored_prefs);
            }
            // keeps the stored file current (a new password hash, a first start)
            shared.save(&st);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let acceptor = server::spawn(listener, shared.clone(), stop.clone())?;
        Ok(WebUi { shared, addr, stop, acceptor: Some(acceptor), temporary_password })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The random password made because none was given or stored.
    pub fn temporary_password(&self) -> Option<&str> {
        self.temporary_password.as_deref()
    }

    /// True once a client called `app/shutdown`.
    pub fn shutdown_requested(&self) -> bool {
        self.shared.lock().shutdown
    }

    /// Feeds a batch of alerts popped from the session.
    pub fn on_alerts(&self, alerts: &[Alert]) {
        let mut st = self.shared.lock();
        let mut replied = false;
        let mut ticked = false;
        let now = unix_now();
        for a in alerts {
            match a {
                Alert::StateUpdate(v) => {
                    for s in v {
                        st.update(s.clone(), now);
                    }
                    ticked = true;
                }
                Alert::TorrentAdded { id, info_hashes, name } => {
                    if !st.torrents.contains_key(id) {
                        let s = TorrentStatus {
                            id: *id,
                            info_hashes: *info_hashes,
                            name: name.clone(),
                            ..TorrentStatus::default()
                        };
                        st.update(s, now);
                    }
                }
                Alert::TorrentRemoved { id, .. } => {
                    if let Some(e) = st.torrents.remove(id) {
                        st.by_hash.remove(&e.hash);
                        st.meta.remove(&e.hash);
                        st.removals += 1;
                        if st.torrent_categories.remove(&e.hash).is_some() {
                            self.shared.save(&st);
                        }
                    }
                    st.replies.retain(|(t, _), _| t != id);
                }
                Alert::SessionStats(s) => {
                    st.stats = *s;
                    ticked = true;
                }
                Alert::FileList { id, files } => {
                    st.reply(*id, ReplyKind::Files, Reply::Files(files.clone()));
                    replied = true;
                }
                Alert::FileProgress { id, done } => {
                    st.reply(*id, ReplyKind::Progress, Reply::Progress(done.clone()));
                    replied = true;
                }
                Alert::TrackerList { id, trackers } => {
                    st.reply(*id, ReplyKind::Trackers, Reply::Trackers(trackers.clone()));
                    replied = true;
                }
                Alert::PieceInfo { id, have, availability, downloading } => {
                    let r = Reply::Pieces(have.clone(), availability.clone(), downloading.clone());
                    st.reply(*id, ReplyKind::Pieces, r);
                    replied = true;
                }
                Alert::PeerInfo { id, peers } => {
                    st.reply(*id, ReplyKind::Peers, Reply::Peers(peers.clone()));
                    replied = true;
                }
                _ => {}
            }
            if logged(a) {
                st.log_line(a, now);
            }
        }
        if ticked {
            st.tick += 1;
            st.sample_speeds();
        }
        drop(st);
        if replied {
            self.shared.replied.notify_all();
        }
    }

    /// Stops accepting connections (also done on drop). Open connections end on their own
    /// after their current request.
    pub fn stop(mut self) {
        self.stop_now();
    }

    fn stop_now(&mut self) {
        if let Some(t) = self.acceptor.take() {
            self.stop.store(true, Ordering::SeqCst);
            server::wake(self.addr);
            let _ = t.join();
        }
    }
}

impl Drop for WebUi {
    fn drop(&mut self) {
        self.stop_now();
    }
}

/// Whether the Logger tab shows an alert: everything but the replies to `post_*` requests
/// (the Win32 Logger's rule) and the port mappers' step-by-step chatter.
fn logged(a: &Alert) -> bool {
    !matches!(
        a,
        Alert::StateUpdate(_)
            | Alert::SessionStats(_)
            | Alert::PeerInfo { .. }
            | Alert::FileList { .. }
            | Alert::FileProgress { .. }
            | Alert::TrackerList { .. }
            | Alert::PieceInfo { .. }
            | Alert::WebSeeds { .. }
            | Alert::PortmapLog { .. }
    )
}

impl State {
    /// Appends an alert's line to the Logger ring: `name: message` for a torrent's alert.
    fn log_line(&mut self, a: &Alert, now: i64) {
        let torrent = a.torrent();
        let message = a.message();
        let text = match torrent.and_then(|id| self.torrents.get(&id)) {
            // some messages (added, failed to add) name the torrent themselves
            Some(e) if !e.status.name.is_empty() && !message.starts_with(&e.status.name) => {
                format!("{}: {message}", e.status.name)
            }
            _ => message,
        };
        self.push_log(torrent, text, now);
    }

    pub(crate) fn push_log(&mut self, torrent: Option<TorrentId>, text: String, time: i64) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log_seq += 1;
        self.log.push_back(LogLine { seq: self.log_seq, time, torrent, text });
    }

    /// Starts (or keeps) the speed history of a torrent a client is looking at.
    pub(crate) fn watch_speed(&mut self, id: TorrentId) -> &SpeedRing {
        let tick = self.tick;
        let ring = self.speed.entry(id).or_insert_with(|| SpeedRing {
            samples: VecDeque::with_capacity(SPEED_SAMPLES),
            pushed: 0,
            last_seen: tick,
        });
        ring.last_seen = tick;
        ring
    }

    /// Once per status tick: one sample per watched torrent, and the rings nobody asked for
    /// in [`SPEED_IDLE_TICKS`] (or whose torrent is gone) dropped.
    fn sample_speeds(&mut self) {
        let tick = self.tick;
        let torrents = &self.torrents;
        self.speed.retain(|id, ring| {
            let Some(e) = torrents.get(id) else { return false };
            if tick.saturating_sub(ring.last_seen) > SPEED_IDLE_TICKS {
                return false;
            }
            if ring.samples.len() == SPEED_SAMPLES {
                ring.samples.pop_front();
            }
            ring.samples.push_back((e.status.download_payload_rate, e.status.upload_payload_rate));
            ring.pushed += 1;
            true
        });
    }

    fn update(&mut self, s: TorrentStatus, now: i64) {
        let version = self.next_version;
        self.next_version += 1;
        let hash = torrent::hash_of(&s.info_hashes);
        let active = s.download_payload_rate > 0 || s.upload_payload_rate > 0;
        match self.torrents.get_mut(&s.id) {
            Some(e) => {
                if active {
                    e.last_activity = now;
                }
                if e.hash != hash {
                    self.by_hash.remove(&e.hash);
                    e.hash = hash.clone();
                }
                e.status = s;
                e.version = version;
                self.by_hash.insert(hash, e.status.id);
            }
            None => {
                let id = s.id;
                let last_activity = if active { now } else { 0 };
                self.by_hash.insert(hash.clone(), id);
                self.torrents.insert(id, Entry { status: s, hash, version, last_activity });
            }
        }
    }

    /// The cached answer to a `post_*` request, if the engine answered one.
    pub(crate) fn cached(&self, id: TorrentId, kind: ReplyKind) -> Option<&Reply> {
        self.replies.get(&(id, kind)).map(|(_, r)| r)
    }

    pub(crate) fn reply(&mut self, id: TorrentId, kind: ReplyKind, r: Reply) {
        self.serial += 1;
        self.replies.insert((id, kind), (self.serial, r));
        if self.replies.len() > 256 {
            let keep_after = self.serial.saturating_sub(64);
            self.replies.retain(|_, (s, _)| *s > keep_after);
        }
    }

    /// Bumps a torrent's version after a change the engine does not report (its category).
    pub fn touch(&mut self, hash: &str) {
        if let Some(e) = self.by_hash.get(hash).and_then(|id| self.torrents.get_mut(id)) {
            e.version = self.next_version;
            self.next_version += 1;
        }
    }

    pub fn is_loopback_bypass(&self, ip: IpAddr) -> bool {
        self.prefs.bypass_local_auth && ip.is_loopback()
    }

    /// An empty cache for unit tests of the protocol code (no server, no engine).
    #[cfg(test)]
    pub(crate) fn for_tests() -> State {
        State {
            torrents: HashMap::new(),
            by_hash: HashMap::new(),
            next_version: 1,
            stats: SessionStats::default(),
            settings: Settings::default(),
            prefs: Prefs {
                save_path: String::new(),
                dl_limit: 0,
                up_limit: 0,
                alt_dl_limit: 0,
                alt_up_limit: 0,
                alt_enabled: false,
                start_paused: false,
                bypass_local_auth: false,
                csrf_protection: true,
                host_header_validation: true,
                domain_list: String::new(),
                max_auth_fail_count: 5,
                ban_duration: 3600,
                session_timeout: 3600,
            },
            username: "admin".into(),
            password: PasswordHash::new("x", 1),
            categories: BTreeMap::new(),
            categories_version: 1,
            torrent_categories: HashMap::new(),
            meta: HashMap::new(),
            stored_prefs: Json::obj(),
            replies: HashMap::new(),
            serial: 0,
            sessions: HashMap::new(),
            bans: BanList::default(),
            shutdown: false,
            tick: 0,
            removals: 0,
            log: VecDeque::new(),
            log_seq: 0,
            speed: HashMap::new(),
        }
    }
}
