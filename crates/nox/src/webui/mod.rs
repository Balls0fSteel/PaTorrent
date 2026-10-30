//! The Web UI server (PLAN.md Phase 6, D18): an embedded HTTP/1.1 server with the native push
//! API (`/pa/ws`, a WebSocket carrying the session protocol of `docs/WEBUI.md`) and, as a
//! compatibility shim (D10), a subset of the qBittorrent WebAPI v2 written from qBittorrent's
//! published WebAPI documentation only.
//!
//! Threads: one acceptor, one thread per connection (at most [`MAX_CONNECTIONS`]); a WebSocket
//! keeps its connection thread. They never touch the engine directly. The owner of the
//! session's alert queue (nox's main loop, or the desktop client hosting the Web UI over its
//! own engine, [`Host`]) hands every batch of alerts to [`WebUi::on_alerts`], which keeps a
//! cache of torrent status, session stats and replies to `post_*` requests and counts a status
//! tick per batch; requests read that cache and post commands through the [`SessionHandle`],
//! WebSocket threads send what changed since their last tick.
//!
//! Hosted (D40, [`Config::hosted`]): the host owns the preferences, the labels, the pause
//! marks and the automations; the Web UI keeps a mirror of its preferences in
//! `State::options`, writes what a browser changed there and hands the journal to the host
//! ([`Shared::save`]), and takes the host's changes through [`WebUi::options_changed`]. Nothing
//! is written to disk and nothing but a request's own command goes to the engine.

mod api;
pub mod auth;
pub(crate) mod commands;
mod fetch;
pub mod http;
pub mod https;
pub mod json;
pub mod native;
pub mod path_policy;
pub(crate) mod prefs;
mod remote;
pub(crate) mod rss;
mod server;
pub mod torrent;
pub mod ws;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use patorrent_core::util::private_file;
use patorrent_core::util::time::local_utc_offset;
use patorrent_core::{
    Alert, AnnounceEntry, Bitfield, FileEntry, MoveCompleted, PartialPiece, PeerInfo, SessionHandle,
    SessionStats, Settings, TorrentFlags, TorrentId, TorrentStatus,
};
use patorrent_frontend::options::GuiOptions;
use patorrent_frontend::scheduler::{Mode, Schedule, Scheduler};
use patorrent_frontend::scrape::Scrapes;
use patorrent_frontend::seeding::{GoalAction, SeedGoal, SeedingGoal};
use patorrent_frontend::torrent_folders::{self, Poller, TorrentFiles, TorrentFolders};
use patorrent_frontend::{fmt, tr};

use auth::{BanList, PasswordHash};
use json::Json;

/// Most connections served at once; more are answered 503 and closed.
pub const MAX_CONNECTIONS: usize = 64;
/// Most logged-in sessions kept; the least recently used is dropped beyond this.
pub const MAX_SESSIONS: usize = 256;
/// How long a request waits for the engine to answer a `post_*` call.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Requests `Shared::query_many` has out at once: well under what `State::reply` keeps after
/// trimming (the last 64), so other clients' replies cannot push ours out before they are read.
const QUERY_GROUP: usize = 32;
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
    /// `--move-on-complete`: the Directories page's "Move completed downloads to" starts on
    /// with this folder (the stored preferences win over it).
    pub move_completed_dir: Option<String>,
    /// `--ip-filter`: the block list the daemon loaded at start, so the BitTorrent page shows
    /// it and the Web UI does not load it a second time.
    pub ip_filter: Option<PathBuf>,
    /// Serve HTTPS with this certificate instead of plain HTTP.
    pub https: Option<https::Certificate>,
    /// `--webui-save-root`: the folders requests may save, move and read in (none = any
    /// absolute local path; see [`path_policy`]).
    pub save_roots: Vec<PathBuf>,
    /// A desktop client hosting the Web UI over its own engine (D40): see [`Hosted`].
    pub hosted: Option<Hosted>,
}

/// A desktop client that hosts the Web UI over its own engine (D40: the Windows GUI, as
/// µTorrent and qBittorrent serve theirs). It owns the preferences (`patorrent.ini`), the
/// labels, the pause marks and the automations (scheduler, seeding goal, scrapes, `.torrent`
/// folders, RSS, moving completed downloads); the Web UI keeps a mirror of its preferences,
/// shows and drives the same torrents, and reports what a browser changed. The Web UI stores
/// nothing itself then, runs none of its own automations and has no RSS Downloader.
pub trait Host: Send + Sync {
    /// Preferences keys a request changed (`None`: removed), as the host's own keys
    /// (`patorrent_frontend::prefs`): a label or a pause mark as `torrent.<key>.label` /
    /// `torrent.<key>.paused` (`InfoHashes::key_hex`), a Preferences page's `lt.*` and option
    /// keys, a seeding goal, the login as `webui.username` / `webui.password` (the hash).
    /// Called on a request thread with the Web UI's lock held (or from [`WebUi::on_alerts`],
    /// for a removed torrent's keys): queue them for the host's thread and return.
    fn options_changed(&self, changes: Vec<patorrent_frontend::prefs::Change>);
    /// `app/shutdown` (the shim) or `session/shutdown` (the page): the host exits. Called on
    /// a request thread without the lock.
    fn shutdown(&self);
}

/// How a desktop client hosts the Web UI ([`Config::hosted`]).
#[derive(Clone)]
pub struct Hosted {
    pub host: Arc<dyn Host>,
    /// The host's preferences, mirrored: the login (`webui.username`, `webui.password` as a
    /// [`PasswordHash`] string), the labels and pause marks (`torrent.<key>.*`), the engine
    /// settings (`lt.*`) and the options the Preferences pages show. The host hands later
    /// changes to [`WebUi::options_changed`].
    pub options: patorrent_frontend::prefs::Prefs,
}

impl std::fmt::Debug for Hosted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hosted").finish_non_exhaustive()
    }
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
            move_completed_dir: None,
            ip_filter: None,
            https: None,
            save_roots: Vec::new(),
            hosted: None,
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

/// Preferences that live in the Web UI rather than in the engine's [`Settings`]: the frontend's
/// options nox has an equivalent for (`frontend::options::GuiOptions` without the tray and
/// balloon ones), the global limits and the Web UI's own settings. One source of truth for
/// `/pa/prefs` and the shim's `app/preferences`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Prefs {
    /// Where new downloads go (`GuiOptions::download_dir`).
    pub save_path: String,
    /// Bytes per second, 0 = unlimited: the normal and the alternative global limits.
    pub dl_limit: u32,
    pub up_limit: u32,
    pub alt_dl_limit: u32,
    pub alt_up_limit: u32,
    pub alt_enabled: bool,
    /// Add torrents stopped (`GuiOptions::add_stopped`).
    pub start_paused: bool,
    /// Move finished downloads to `completed_dir` (Directories page).
    pub move_completed: bool,
    pub completed_dir: String,
    pub move_only_default: bool,
    /// "Location of .torrents" (Directories page): stored copies, finished jobs, auto-load.
    pub torrent_folders: TorrentFolders,
    /// The page asks before removing torrents.
    pub confirm_delete: bool,
    /// The page shows the Add New Torrent dialog for `.torrent` files (`GET /pa/add`'s
    /// `show_dialog`).
    pub show_add_dialog: bool,
    /// A new listen port at every start.
    pub random_port: bool,
    /// Per-torrent defaults: peer exchange, connected peers and upload slots per new torrent
    /// (0 = no limit).
    pub pex: bool,
    pub max_peers_per_torrent: u32,
    pub upload_slots_per_torrent: u32,
    /// Preferences > Queueing's seeding goal (`GuiOptions::seeding_goal`).
    pub seeding_goal: SeedingGoal,
    /// The IP filter: on, and the block list (empty = `ipfilter.dat` in the state directory).
    pub ip_filter: bool,
    pub ip_filter_path: String,
    /// Preferences > Scheduler (`GuiOptions::schedule`).
    pub schedule: Schedule,
    /// Preferences > General's language (`GuiOptions::language`): the Web UI's, the server's.
    pub language: String,
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

impl Prefs {
    /// The defaults for a daemon saving to `save_path` with `settings`' limits.
    pub fn defaults(save_path: &str, settings: &Settings) -> Prefs {
        let gui = GuiOptions::load(&patorrent_frontend::prefs::Prefs::default(), save_path);
        let mut p = Prefs {
            save_path: save_path.to_owned(),
            dl_limit: settings.download_rate_limit,
            up_limit: settings.upload_rate_limit,
            alt_dl_limit: 10 * 1024,
            alt_up_limit: 10 * 1024,
            alt_enabled: false,
            start_paused: false,
            move_completed: false,
            completed_dir: String::new(),
            move_only_default: true,
            torrent_folders: TorrentFolders::default(),
            confirm_delete: true,
            show_add_dialog: true,
            random_port: false,
            pex: true,
            max_peers_per_torrent: 0,
            upload_slots_per_torrent: 0,
            seeding_goal: SeedingGoal::default(),
            ip_filter: false,
            ip_filter_path: String::new(),
            schedule: Schedule::default(),
            language: String::new(),
            bypass_local_auth: false,
            csrf_protection: true,
            host_header_validation: true,
            domain_list: String::new(),
            max_auth_fail_count: 5,
            ban_duration: 3600,
            session_timeout: 3600,
        };
        p.set_gui(&gui);
        p
    }

    /// The frontend's view of these preferences (the tray and balloon options at their
    /// defaults), for `GuiOptions`' helpers and its preferences keys.
    pub fn gui(&self) -> GuiOptions {
        GuiOptions {
            download_dir: self.save_path.clone(),
            move_completed: self.move_completed,
            completed_dir: self.completed_dir.clone(),
            move_only_default: self.move_only_default,
            torrent_folders: self.torrent_folders.clone(),
            confirm_delete: self.confirm_delete,
            alternate_rows: true,
            check_association: false,
            close_to_tray: true,
            minimize_to_tray: false,
            balloons: true,
            activate_on_add: true,
            add_stopped: self.start_paused,
            show_add_dialog: self.show_add_dialog,
            random_port: self.random_port,
            pex: self.pex,
            max_peers_per_torrent: self.max_peers_per_torrent,
            upload_slots_per_torrent: self.upload_slots_per_torrent,
            seed_ratio: self.seeding_goal.goal.ratio_pct,
            seed_minutes: self.seeding_goal.goal.minutes,
            seed_limit_upload: self.seeding_goal.limit_upload,
            seed_limit_kib: self.seeding_goal.limit_kib,
            ip_filter: self.ip_filter,
            ip_filter_path: self.ip_filter_path.clone(),
            schedule: self.schedule.clone(),
            language: self.language.clone(),
            // nox checks for no updates; the GUIs' own settings keep these
            check_updates: true,
            update_beta: false,
        }
    }

    /// Writes these preferences into `g`, leaving the desktop-only options (tray, balloons,
    /// window activation, updates) as they are: a host's options with the Web UI's changes on them.
    pub fn apply_to(&self, g: &mut GuiOptions) {
        let mine = self.gui();
        *g = GuiOptions {
            alternate_rows: g.alternate_rows,
            check_association: g.check_association,
            close_to_tray: g.close_to_tray,
            minimize_to_tray: g.minimize_to_tray,
            balloons: g.balloons,
            activate_on_add: g.activate_on_add,
            check_updates: g.check_updates,
            update_beta: g.update_beta,
            ..mine
        };
    }

    pub fn set_gui(&mut self, g: &GuiOptions) {
        self.save_path = g.download_dir.clone();
        self.move_completed = g.move_completed;
        self.completed_dir = g.completed_dir.clone();
        self.move_only_default = g.move_only_default;
        self.torrent_folders = g.torrent_folders.clone();
        self.confirm_delete = g.confirm_delete;
        self.start_paused = g.add_stopped;
        self.show_add_dialog = g.show_add_dialog;
        self.random_port = g.random_port;
        self.pex = g.pex;
        self.max_peers_per_torrent = g.max_peers_per_torrent;
        self.upload_slots_per_torrent = g.upload_slots_per_torrent;
        self.seeding_goal = g.seeding_goal();
        self.ip_filter = g.ip_filter;
        self.ip_filter_path = g.ip_filter_path.clone();
        self.schedule = g.schedule.clone();
        self.language = g.language.clone();
    }
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
    /// Our pieces, the availability per piece, the pieces being downloaded with their blocks.
    Pieces(Bitfield, Vec<u32>, Vec<PartialPiece>),
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
    /// `InfoHashes::key_hex` (the host's and the preferences' key, `torrent.<key>.*`) to id;
    /// it differs from [`torrent::hash_of`] for a v2-only torrent.
    pub by_key: HashMap<String, TorrentId>,
    /// A desktop client hosts the Web UI ([`Config::hosted`]): `options` mirrors its
    /// preferences, the labels and pause marks are kept there as its keys too.
    pub hosted: bool,
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
    /// The stored preferences, as the frontend's preferences keys (what the Win32 client keeps
    /// in `patorrent.ini`): the engine settings that differ from the defaults as `lt.<name>`,
    /// the `GuiOptions` keys, the Web UI's own as `webui.<name>`, and the Add dialog's
    /// recent folders (`add.recent_dirs`). Saved as the `options` object of the state file and
    /// re-applied on the next start.
    pub options: patorrent_frontend::prefs::Prefs,
    /// The download folder the daemon was started with: the Directories page's default.
    pub default_save_path: String,
    /// The address the server is bound to (the Web UI page shows it).
    pub listen_addr: SocketAddr,
    /// Bumped when the folder options change, so the owner's `MoveCompleted` follows.
    pub gui_version: u64,
    /// The weekly scheduler's mode in force (Preferences > Scheduler is `prefs.schedule`).
    pub scheduler: Scheduler,
    /// The periodic scrape that gives stopped and queued torrents their swarm counts, run on
    /// the status tick.
    pub scrapes: Scrapes,
    /// Stores and moves the `.torrent` copies (`prefs.torrent_folders`), fed every alert, and
    /// watches the auto-load folder on the status tick.
    pub torrent_files: TorrentFiles,
    pub autoload: Poller,
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
    /// Torrents (by hash) the user paused rather than stopped, so the cells say "Paused"
    /// (`frontend::status`); kept in the state file.
    pub user_paused: HashSet<String>,
    /// Torrents (by hash) added with "Add to top of queue", moved there when the engine
    /// reports them added.
    pub pending_top: commands::PendingTop,
    /// Bumped when the RSS Downloader changed (`rss.rs`); a WebSocket `tick` carries it.
    pub rss_version: u64,
    /// Magnets added through the Add dialog, waiting for their metadata or for the user's
    /// choice (kept in `options`), and a version a `tick` carries as `adds`.
    pub awaiting: patorrent_frontend::add::AwaitingMetadata,
    pub adds_version: u64,
}

pub(crate) struct Shared {
    pub handle: SessionHandle,
    pub state: Mutex<State>,
    replied: Condvar,
    pub limits: http::Limits,
    pub iterations: u32,
    pub state_file: Option<PathBuf>,
    /// Where `ipfilter.dat` is looked for when the IP filter has no path: the state directory,
    /// else the working directory.
    pub settings_dir: PathBuf,
    /// The RSS Downloader; its lock comes before the State lock, never after it.
    pub rss: Mutex<rss::Rss>,
    /// The certificate when serving HTTPS.
    pub tls: Option<patorrent_core::tls::Identity>,
    /// The paths requests may name; fixed at start.
    pub path_policy: path_policy::PathPolicy,
    /// The connected remote sessions' alert queues ([`Shared::fan_out`]).
    pub(crate) remotes: Mutex<Vec<Arc<remote::Client>>>,
    /// The desktop client hosting the Web UI ([`Config::hosted`]), given the preferences
    /// journal at every [`Shared::save`].
    pub(crate) host: Option<Arc<dyn Host>>,
}

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Shared {
    pub fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking request thread must not take the whole Web UI down with it.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn rss(&self) -> MutexGuard<'_, rss::Rss> {
        self.rss.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The desktop client hosting the Web UI, if one does ([`Config::hosted`]).
    pub(crate) fn hosted(&self) -> Option<&Arc<dyn Host>> {
        self.host.as_ref()
    }

    pub(crate) fn is_hosted(&self) -> bool {
        self.host.is_some()
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

    /// [`query`](Self::query) for several torrents (Torrent Properties on a whole selection):
    /// the requests go out in groups of `QUERY_GROUP` and the answers are taken as they come.
    /// The torrents that do not answer within `REPLY_TIMEOUT`, all groups together, are left
    /// out.
    pub fn query_many(
        &self,
        ids: &[TorrentId],
        kind: ReplyKind,
        post: impl Fn(&SessionHandle, TorrentId),
    ) -> HashMap<TorrentId, Reply> {
        let deadline = Instant::now() + REPLY_TIMEOUT;
        let mut out = HashMap::new();
        for group in ids.chunks(QUERY_GROUP) {
            let start = self.lock().serial;
            for &id in group {
                post(&self.handle, id);
            }
            let mut waiting = group.to_vec();
            let mut st = self.lock();
            loop {
                waiting.retain(|id| match st.replies.get(&(*id, kind)) {
                    Some((serial, r)) if *serial > start => {
                        out.insert(*id, r.clone());
                        false
                    }
                    _ => st.torrents.contains_key(id),
                });
                let now = Instant::now();
                if waiting.is_empty() || now >= deadline {
                    break;
                }
                st = self.replied.wait_timeout(st, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        out
    }

    /// Writes the state file (credentials as a hash, stored preferences, categories). Hosted:
    /// hands the preferences journal (what requests changed in `options` since the last call)
    /// to the host instead; nothing is written.
    pub fn save(&self, st: &mut State) {
        if let Some(host) = &self.host {
            let changes = st.options.take_changes();
            if !changes.is_empty() {
                host.options_changed(changes);
            }
            return;
        }
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
        let mut paused: Vec<&String> = st.user_paused.iter().collect();
        paused.sort();
        let paused: Vec<Json> = paused.into_iter().map(|h| Json::Str(h.clone())).collect();
        let mut options = Json::obj();
        for (k, v) in st.options.entries() {
            options.set(k, v);
        }
        let doc = Json::obj()
            .with("username", st.username.as_str())
            .with("password", st.password.to_string())
            .with("options", options)
            .with("categories", cats)
            .with("torrent_categories", tc)
            .with("paused", Json::Arr(paused));
        if let Some(dir) = path.parent() {
            let _ = private_file::create_dir_all(dir);
        }
        // Write then rename, so a crash never leaves half a file; owner-only on Unix (the
        // password hash, and the proxy password, which is plain there).
        let _ = private_file::write(path, doc.to_json().as_bytes());
    }

    /// Applies the global limits for the current alternative-speed mode, through the
    /// scheduler (its Limited hours override them). Hosted: nothing (the host applies its
    /// settings itself; a browser's limits reach it through the journal).
    pub fn apply_limits(&self, st: &mut State) {
        if self.is_hosted() {
            return;
        }
        let (dl, up) = if st.prefs.alt_enabled {
            (st.prefs.alt_dl_limit, st.prefs.alt_up_limit)
        } else {
            (st.prefs.dl_limit, st.prefs.up_limit)
        };
        st.settings.download_rate_limit = dl;
        st.settings.upload_rate_limit = up;
        self.run_scheduler(st, true);
    }

    /// Puts the scheduler's mode for this hour into effect; with `force` the settings are sent
    /// even if the mode stays (they changed). Hosted: nothing (the scheduler is the host's, so
    /// the mode here stays Normal).
    pub(crate) fn run_scheduler(&self, st: &mut State, force: bool) {
        if self.is_hosted() {
            return;
        }
        let now = unix_now();
        let offset = local_utc_offset(now);
        let old = st.scheduler.mode();
        let Some(c) = st.scheduler.update(&st.prefs.schedule, &st.settings, now, offset, force) else {
            return;
        };
        if let Some(s) = c.settings {
            self.handle.apply_settings(s);
        }
        match c.pause_session {
            Some(true) => self.handle.pause(),
            Some(false) => self.handle.resume(),
            None => {}
        }
        if let Some(on) = c.upload_mode {
            for id in st.torrents.keys() {
                let t = self.handle.torrent(*id);
                if on {
                    t.set_flags(TorrentFlags::UPLOAD_MODE);
                } else if !st.awaiting.holds(*id) {
                    // a magnet waiting for the Add dialog stays in upload mode until its OK
                    t.unset_flags(TorrentFlags::UPLOAD_MODE);
                }
            }
        }
        if c.mode != old {
            st.push_log(None, tr!(LogScheduler, mode = c.mode.label()), now);
        }
    }
}

/// The running Web UI. Dropping it stops the server.
pub struct WebUi {
    shared: Arc<Shared>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
    temporary_password: Option<String>,
    https: Option<HttpsInfo>,
}

/// The certificate an HTTPS Web UI presents, for the start-up message.
#[derive(Clone, Debug)]
pub struct HttpsInfo {
    pub fingerprint: patorrent_core::tls::Fingerprint,
    /// Unix seconds.
    pub not_after: i64,
    /// The certificate file (none for an in-memory one).
    pub cert_path: Option<PathBuf>,
    /// The self-signed certificate was made (or renewed) by this start.
    pub generated: bool,
}

impl WebUi {
    /// Binds `config.addr` and starts serving.
    pub fn start(handle: SessionHandle, config: Config) -> std::io::Result<WebUi> {
        // hosted: no state file; the login, labels and pause marks come from the host's
        // preferences (`torrent.<key>.*`, the host's keys)
        let hosted = config.hosted.as_ref();
        let state_file = match hosted {
            Some(_) => None,
            None => config.state_dir.as_ref().map(|d| d.join(STATE_FILE)),
        };
        let stored = state_file
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| json::parse(&s).ok())
            .unwrap_or_else(Json::obj);
        let stored_login = |key: &str| -> Option<String> {
            match hosted {
                Some(h) => h.options.get(&format!("webui.{key}")).map(str::to_owned),
                None => stored.get(key).and_then(Json::as_str).map(str::to_owned),
            }
        };

        let mut temporary_password = None;
        let password = match (&config.password, stored_login("password")) {
            (Some(p), _) => PasswordHash::new(p, config.pbkdf2_iterations),
            (None, Some(h)) if PasswordHash::parse(&h).is_some() => PasswordHash::parse(&h).expect("checked"),
            _ => {
                let p = auth::random_hex(6);
                let h = PasswordHash::new(&p, config.pbkdf2_iterations);
                temporary_password = Some(p);
                h
            }
        };
        // A stored user name is the one set through setPreferences; the command line wins when
        // it gives a password too.
        let username = match stored_login("username") {
            Some(u) if config.password.is_none() && temporary_password.is_none() => u,
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
        let user_paused: HashSet<String> = match stored.get("paused") {
            Some(Json::Arr(v)) => v.iter().filter_map(Json::as_str).map(str::to_owned).collect(),
            _ => HashSet::new(),
        };
        // the stored preferences (the frontend's keys), and the older `recent_dirs` list; hosted,
        // the host's preferences, mirrored (its labels make the label list complete at once)
        let mut options = match hosted {
            Some(h) => {
                let mut o = h.options.clone();
                o.track_changes();
                for (k, v) in o.entries() {
                    if k.starts_with("torrent.") && k.ends_with(".label") && !v.is_empty() {
                        categories.entry(v.to_owned()).or_default();
                    }
                }
                o
            }
            None => patorrent_frontend::prefs::Prefs::default(),
        };
        for (k, v) in stored.get("options").map(Json::entries).unwrap_or_default() {
            if let Some(v) = v.as_str() {
                options.set(k, v);
            }
        }
        if let Some(Json::Arr(v)) = stored.get("recent_dirs") {
            // most recent first in the file; remember_dir puts each first, so go backwards
            for d in v.iter().rev().filter_map(Json::as_str) {
                patorrent_frontend::add::remember_dir(&mut options, d);
            }
        }
        // hosted: the host handles its pending magnets after a restart
        let awaiting = match hosted {
            Some(_) => patorrent_frontend::add::AwaitingMetadata::default(),
            None => patorrent_frontend::add::AwaitingMetadata::load(&options),
        };
        let settings = config.settings.clone();
        let mut prefs = Prefs::defaults(&config.save_path, &settings);
        if let Some(d) = &config.move_completed_dir {
            prefs.move_completed = true;
            prefs.completed_dir = d.clone();
            prefs.move_only_default = false;
        }
        if let Some(p) = &config.ip_filter {
            prefs.ip_filter = true;
            prefs.ip_filter_path = p.to_string_lossy().into_owned();
        }
        // the certificate before the socket: a broken one fails the start without binding
        let loaded = match &config.https {
            Some(source) => Some(https::load(source, unix_now())?),
            None => None,
        };
        let https = loaded.as_ref().map(|l| HttpsInfo {
            fingerprint: l.identity.fingerprint(),
            not_after: l.identity.info().not_after,
            cert_path: l.cert_path.clone(),
            generated: l.generated,
        });
        let listener = std::net::TcpListener::bind(config.addr)?;
        let addr = listener.local_addr()?;
        let settings_dir = config.state_dir.clone().unwrap_or_else(|| PathBuf::from("."));
        let shared = Arc::new(Shared {
            handle,
            state: Mutex::new(State {
                torrents: HashMap::new(),
                by_hash: HashMap::new(),
                by_key: HashMap::new(),
                hosted: hosted.is_some(),
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
                options,
                default_save_path: config.save_path.clone(),
                listen_addr: addr,
                gui_version: 1,
                scheduler: Scheduler::new(),
                scrapes: Scrapes::default(),
                torrent_files: TorrentFiles::new(),
                autoload: Poller::new(),
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
                user_paused,
                pending_top: HashSet::new(),
                rss_version: 1,
                awaiting,
                adds_version: 1,
            }),
            replied: Condvar::new(),
            limits: config.limits,
            iterations: config.pbkdf2_iterations,
            state_file,
            settings_dir,
            // rss.dat beside webui.json, in the config directory; hosted, no downloader
            rss: Mutex::new(rss::Rss::load(
                config
                    .state_dir
                    .as_ref()
                    .filter(|_| hosted.is_none())
                    .map(|d| d.join(patorrent_frontend::rss::store::FILE_NAME)),
            )),
            tls: loaded.map(|l| l.identity),
            path_policy: path_policy::PathPolicy::new(config.save_roots.clone()),
            remotes: Mutex::new(Vec::new()),
            host: hosted.map(|h| h.host.clone()),
        });
        {
            let mut st = shared.lock();
            if st.hosted {
                // the host's settings and options, read back for the Preferences pages; the
                // host applied them itself, and its language is the process's already
                prefs::read_back(&mut st);
                // the mirror shows the login in force (not journaled: the host gave it, or it
                // knows the password it passed and reads `temporary_password`)
                let login = [
                    ("webui.username".to_owned(), Some(st.username.clone())),
                    ("webui.password".to_owned(), Some(st.password.to_string())),
                ];
                st.options.apply_changes(&login);
            } else {
                // what the Web UI's preferences changed before this start, re-applied; the
                // `preferences` object of state files written before slice 6 holds the shim's
                // keys
                prefs::restore(&shared, &mut st, config.ip_filter.as_deref());
                // the Web UI's language (Preferences > General); PATORRENT_LANG over it
                for p in patorrent_frontend::lang::init(&st.prefs.language) {
                    st.push_log(None, p, unix_now());
                }
                if let Some(legacy @ Json::Obj(_)) = stored.get("preferences") {
                    api::apply_preferences(&shared, &mut st, legacy, None);
                }
                // keeps the stored file current (a new password hash, a first start)
                shared.save(&mut st);
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let acceptor = server::spawn(listener, shared.clone(), stop.clone())?;
        Ok(WebUi { shared, addr, stop, acceptor: Some(acceptor), temporary_password, https })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The certificate, when serving HTTPS.
    pub fn https(&self) -> Option<&HttpsInfo> {
        self.https.as_ref()
    }

    /// The page's address: `http://` or `https://`, with a loopback host for a wildcard
    /// listen address.
    pub fn url(&self) -> String {
        let scheme = if self.https.is_some() { "https" } else { "http" };
        let ip = match self.addr.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        format!("{scheme}://{}/", SocketAddr::new(ip, self.addr.port()))
    }

    /// The random password made because none was given or stored.
    pub fn temporary_password(&self) -> Option<&str> {
        self.temporary_password.as_deref()
    }

    /// `run --import`: the imported torrents' labels (by hash; a torrent with one already keeps
    /// it), their µTorrent Paused marks and the Logger lines, saved at once.
    pub fn assign_imported(&self, labels: &[(String, String)], paused: &[String], log: &[String]) {
        let mut st = self.shared.lock();
        for (hash, label) in labels {
            if st.torrent_categories.contains_key(hash) {
                continue;
            }
            st.set_label(hash, label);
        }
        for hash in paused {
            st.set_user_paused(hash, true);
        }
        let now = unix_now();
        for line in log {
            st.push_log(None, line.clone(), now);
        }
        self.shared.save(&mut st);
    }

    /// True once a client called `app/shutdown`.
    pub fn shutdown_requested(&self) -> bool {
        self.shared.lock().shutdown
    }

    /// Reads the IP filter's block list again (the daemon's SIGHUP) when the filter is on: the
    /// BitTorrent page's file, else `ipfilter.dat` in the state directory. A list that does not
    /// load leaves the old rules in force. Returns the line it logged.
    pub fn reload_ip_filter(&self) -> Option<String> {
        let file = {
            let st = self.shared.lock();
            if !st.prefs.ip_filter {
                return None;
            }
            st.prefs.gui().ip_filter_file(&self.shared.settings_dir)
        };
        // read without the State lock (SEC-05)
        let line = match self.shared.handle.load_ip_filter(&file) {
            Ok(n) => tr!(LogIpFilterRules, count = n, path = file.display()),
            Err(e) => tr!(LogIpFilterKept, path = file.display(), error = e),
        };
        self.shared.lock().push_log(None, line.clone(), unix_now());
        Some(line)
    }

    /// Points the owner's "Move completed downloads" helper at the Directories preferences
    /// when they changed since the last call (`seen` is the caller's copy of the version).
    /// Returns whether it changed anything.
    pub fn sync_move_completed(&self, seen: &mut u64, current: &mut Option<MoveCompleted>) -> bool {
        let st = self.shared.lock();
        if st.gui_version == *seen {
            return false;
        }
        *seen = st.gui_version;
        st.prefs.gui().update_move_completed(current);
        true
    }

    /// Hosted ([`Config::hosted`]): the host's preferences changed (its journal,
    /// `Prefs::take_changes`); the mirror follows. A label or a pause mark updates the lists
    /// (clients see it at the next tick), the login replaces the one in force, settings and
    /// option keys refresh what the Preferences pages show. Nothing goes to the engine: the
    /// host applied it.
    pub fn options_changed(&self, changes: &[patorrent_frontend::prefs::Change]) {
        let mut st = self.shared.lock();
        if !st.hosted {
            return;
        }
        st.options.apply_changes(changes);
        let mut other = false;
        for (key, value) in changes {
            if let Some(k) = key.strip_prefix("torrent.") {
                if let Some(k) = k.strip_suffix(".label") {
                    let label = value.as_deref().unwrap_or("");
                    match st.by_key.get(k).and_then(|id| st.torrents.get(id)).map(|e| e.hash.clone()) {
                        Some(hash) => st.label_cached(&hash, label),
                        // not seen yet: the label joins the list now, the torrent takes it
                        // at its first sight
                        None if !label.is_empty() && !st.categories.contains_key(label) => {
                            st.categories.insert(label.to_owned(), String::new());
                            st.categories_version += 1;
                        }
                        None => {}
                    }
                } else if let Some(k) = k.strip_suffix(".paused") {
                    let Some(hash) =
                        st.by_key.get(k).and_then(|id| st.torrents.get(id)).map(|e| e.hash.clone())
                    else {
                        continue;
                    };
                    st.paused_cached(&hash, value.as_deref() == Some("1"));
                }
                // a seeding goal override is read from `options` when needed
                continue;
            }
            match key.as_str() {
                "webui.username" => {
                    if let Some(u) = value.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                        st.username = u.to_owned();
                    }
                }
                "webui.password" => {
                    let Some(h) = value.as_deref().and_then(PasswordHash::parse) else { continue };
                    if h.to_string() != st.password.to_string() {
                        st.password = h;
                        // every login ends: the sessions, their WebSockets, the remotes (SEC-24)
                        st.sessions.clear();
                        self.shared.end_remotes();
                    }
                }
                _ => other = true,
            }
        }
        if other {
            // the Preferences pages show the host's values
            prefs::read_back(&mut st);
        }
    }

    /// Feeds a batch of alerts popped from the session.
    pub fn on_alerts(&self, alerts: &[Alert]) {
        self.shared.fan_out(alerts);
        let mut st = self.shared.lock();
        let mut replied = false;
        let mut ticked = false;
        let now = unix_now();
        // hosted: the automations (`.torrent` folders, seeding goal, scheduler, scrapes,
        // auto-load, RSS) are the host's; the cache, the replies and the Web UI's own adds stay
        let hosted = st.hosted;
        let folders = st.prefs.torrent_folders.clone();
        for a in alerts {
            if !hosted {
                for line in st.torrent_files.on_alert(a, &folders, &self.shared.handle) {
                    st.push_log(None, line, now);
                }
            }
            let left = matches!(a, Alert::TorrentRemoved { id, .. } if st.awaiting.holds(*id));
            let s = &mut *st;
            if s.awaiting.on_alert(a, &self.shared.handle, &mut s.options) || left {
                s.adds_version += 1;
            }
            if left {
                self.shared.save(&mut st);
            }
            match a {
                Alert::StateUpdate(v) => {
                    let goal = st.prefs.seeding_goal;
                    for s in v {
                        // first sight of a restored torrent (they post no `TorrentAdded`) during
                        // Seeding only hours
                        if !hosted
                            && !st.torrents.contains_key(&s.id)
                            && st.scheduler.mode() == Mode::SeedOnly
                            && !s.flags.contains(TorrentFlags::UPLOAD_MODE)
                        {
                            self.shared.handle.torrent(s.id).set_flags(TorrentFlags::UPLOAD_MODE);
                        }
                        st.update(s.clone(), now);
                        // Queueing > Seeding Goal: stop or limit a queued seed past its goal
                        if s.is_finished && !hosted {
                            let own = SeedGoal::load_override(&st.options, &s.info_hashes.key_hex());
                            if let Some(action) = goal.check(s, own) {
                                action.apply(&self.shared.handle.torrent(s.id));
                                let what = match action {
                                    GoalAction::Stop => tr!(SeedingGoalStopped).to_owned(),
                                    GoalAction::LimitUpload(b) => {
                                        tr!(SeedingGoalLimited, rate = fmt::speed_always(b))
                                    }
                                };
                                st.push_log(Some(s.id), tr!(LogSeedingGoal, name = s.name, what), now);
                            }
                        }
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
                    // added by anyone during Seeding only hours (the host's scheduler, hosted)
                    if !hosted && st.scheduler.mode() == Mode::SeedOnly {
                        self.shared.handle.torrent(*id).set_flags(TorrentFlags::UPLOAD_MODE);
                    }
                    // "Add to top of queue" from the Add dialog, now that the id is known
                    if st.pending_top.remove(&torrent::hash_of(info_hashes)) {
                        self.shared.handle.torrent(*id).queue_position(patorrent_core::QueueMove::Top);
                    }
                }
                Alert::TorrentRemoved { id, .. } => {
                    if let Some(e) = st.torrents.remove(id) {
                        st.by_hash.remove(&e.hash);
                        st.meta.remove(&e.hash);
                        st.removals += 1;
                        let key = e.status.info_hashes.key_hex();
                        st.by_key.remove(&key);
                        let had_goal = SeedGoal::load_override(&st.options, &key).is_some();
                        SeedGoal::store_override(&mut st.options, &key, None);
                        let forget = st.torrent_categories.remove(&e.hash).is_some() | had_goal;
                        if hosted {
                            // the host drops its own keys too; removing an absent key is not
                            // journaled, and an empty journal is not handed on
                            st.options.remove(&format!("torrent.{key}.label"));
                            st.options.remove(&format!("torrent.{key}.paused"));
                        }
                        if st.user_paused.remove(&e.hash) | forget | hosted {
                            self.shared.save(&mut st);
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
        let mut found = Vec::new();
        if ticked {
            st.tick += 1;
            st.sample_speeds();
        }
        if ticked && !hosted {
            self.shared.run_scheduler(&mut st, false);
            // no scrapes while the scheduler has the session turned off
            if st.scheduler.mode() != Mode::Off {
                let s = &mut *st;
                s.scrapes.run(s.torrents.values().map(|e| &e.status), Instant::now(), &self.shared.handle);
            }
            found = autoload(&mut st, &self.shared.path_policy, now);
        }
        drop(st);
        if replied {
            self.shared.replied.notify_all();
        }
        for (path, source) in found {
            if let Err(e) = commands::add(&self.shared, source, &commands::AddOptions::default()) {
                let line = tr!(FoldersNotLoaded, path = path.display(), error = e.msg);
                self.shared.lock().push_log(None, line, now);
            }
        }
        if ticked && !hosted {
            rss::tick(&self.shared);
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

/// Preferences > Directories "Automatically load .torrents from": the new files in the folder,
/// to add without the Add New Torrent dialog (they are renamed to `.torrent.loaded`, or
/// deleted, once the engine has them). Files whose torrent is in the session already are
/// finished at once.
///
/// A folder the path policy refuses (one set before `--webui-save-root` was given) is not
/// watched: loaded files may be deleted there (SEC-05).
fn autoload(st: &mut State, policy: &path_policy::PathPolicy, now: i64) -> Vec<(PathBuf, commands::Source)> {
    let Some(dir) = st.prefs.torrent_folders.loads_from().map(PathBuf::from) else { return Vec::new() };
    let paths = match policy.check(&dir.to_string_lossy()) {
        Ok(()) => st.autoload.poll(&dir, Instant::now()),
        Err(why) => {
            st.autoload.refuse(&dir, why);
            Vec::new()
        }
    };
    if let Some(e) = st.autoload.take_error() {
        st.push_log(None, tr!(LogAutoloadError, error = e), now);
    }
    let mut found = Vec::new();
    for path in paths {
        let (bytes, ti) = match torrent_folders::read(&path) {
            Ok(r) => r,
            Err(e) => {
                st.push_log(None, tr!(FoldersNotLoaded, path = path.display(), error = e), now);
                continue;
            }
        };
        if st.by_hash.contains_key(&torrent::hash_of(&ti.info_hashes())) {
            let key = ti.info_hashes().key_hex();
            let line =
                torrent_folders::loaded(ti.name(), &key, &path, st.prefs.torrent_folders.autoload_delete);
            st.push_log(None, tr!(LogAlreadyInList, line), now);
            continue;
        }
        st.torrent_files.loading(&ti, path.clone());
        found.push((path, commands::Source::Torrent(bytes, Box::new(ti))));
    }
    found
}

/// Whether the Logger tab shows an alert: everything but the replies to `post_*` requests
/// (the Win32 Logger's rule), the port mappers' step-by-step chatter and scrape results (one
/// per stopped or queued torrent every half hour, `frontend::scrape`; the counts show in the
/// Seeds and Peers columns).
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
            | Alert::TorrentFile { .. }
            | Alert::PortmapLog { .. }
            | Alert::ScrapeReply { .. }
            | Alert::ScrapeFailed { .. }
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
        let key = s.info_hashes.key_hex();
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
                let old_key = e.status.info_hashes.key_hex();
                if old_key != key {
                    self.by_key.remove(&old_key);
                }
                e.status = s;
                e.version = version;
                self.by_hash.insert(hash, e.status.id);
                self.by_key.insert(key, e.status.id);
            }
            None => {
                let id = s.id;
                let last_activity = if active { now } else { 0 };
                self.by_hash.insert(hash.clone(), id);
                self.by_key.insert(key.clone(), id);
                self.torrents.insert(id, Entry { status: s, hash: hash.clone(), version, last_activity });
                if self.hosted {
                    // first sight: the host's label and pause mark for it
                    let label = self.options.get(&format!("torrent.{key}.label")).map(str::to_owned);
                    if let Some(label) = label.filter(|l| !l.is_empty()) {
                        self.label_cached(&hash, &label);
                    }
                    if self.options.get_bool(&format!("torrent.{key}.paused"), false) {
                        self.user_paused.insert(hash);
                    }
                }
            }
        }
    }

    /// The host's key (`InfoHashes::key_hex`) of a cached torrent.
    pub(crate) fn key_of(&self, hash: &str) -> Option<String> {
        self.by_hash.get(hash).and_then(|id| self.torrents.get(id)).map(|e| e.status.info_hashes.key_hex())
    }

    /// Sets a cached torrent's label in the lists (empty removes it; a new label joins the
    /// label list) and bumps its version. Nothing is stored.
    fn label_cached(&mut self, hash: &str, label: &str) {
        if label.is_empty() {
            self.torrent_categories.remove(hash);
        } else {
            if !self.categories.contains_key(label) {
                self.categories.insert(label.to_owned(), String::new());
                self.categories_version += 1;
            }
            self.torrent_categories.insert(hash.to_owned(), label.to_owned());
        }
        self.touch(hash);
    }

    /// Records (or clears) the user's Pause of a cached torrent and bumps its version. Nothing
    /// is stored. Returns whether it changed.
    fn paused_cached(&mut self, hash: &str, paused: bool) -> bool {
        let changed =
            if paused { self.user_paused.insert(hash.to_owned()) } else { self.user_paused.remove(hash) };
        if changed {
            self.touch(hash);
        }
        changed
    }

    /// Sets a torrent's label (empty removes it) and, hosted, writes it into the mirror as the
    /// host's `torrent.<key>.label` for the next [`Shared::save`]. For a torrent in the cache;
    /// [`State::set_label_keyed`] for one being added.
    pub(crate) fn set_label(&mut self, hash: &str, label: &str) {
        let key = self.key_of(hash);
        self.set_label_keyed(hash, key.as_deref(), label);
    }

    /// [`State::set_label`] with the host's key given (a torrent not in the cache yet).
    pub(crate) fn set_label_keyed(&mut self, hash: &str, key: Option<&str>, label: &str) {
        self.label_cached(hash, label);
        if let (true, Some(key)) = (self.hosted, key) {
            let k = format!("torrent.{key}.label");
            if label.is_empty() {
                self.options.remove(&k);
            } else {
                self.options.set(&k, label);
            }
        }
    }

    /// Records (or clears) the user's Pause of a torrent and, hosted, the host's
    /// `torrent.<key>.paused` mark in the mirror. Returns whether it changed.
    pub(crate) fn set_user_paused(&mut self, hash: &str, paused: bool) -> bool {
        let changed = self.paused_cached(hash, paused);
        if self.hosted {
            if let Some(key) = self.key_of(hash) {
                let k = format!("torrent.{key}.paused");
                if paused {
                    self.options.set(&k, "1");
                } else {
                    self.options.remove(&k);
                }
            }
        }
        changed
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

    /// A Web UI without a server for unit tests: `handle`'s engine, an empty cache, and
    /// `settings_dir` for the default `ipfilter.dat`.
    #[cfg(test)]
    pub(crate) fn shared_for_tests(handle: SessionHandle, settings_dir: PathBuf) -> Shared {
        Shared {
            handle,
            state: Mutex::new(State::for_tests()),
            replied: Condvar::new(),
            limits: http::Limits::default(),
            iterations: 1,
            state_file: None,
            settings_dir,
            rss: Mutex::new(rss::Rss::load(None)),
            tls: None,
            path_policy: path_policy::PathPolicy::default(),
            remotes: Mutex::new(Vec::new()),
            host: None,
        }
    }

    /// An empty cache for unit tests of the protocol code (no server, no engine).
    #[cfg(test)]
    pub(crate) fn for_tests() -> State {
        State {
            torrents: HashMap::new(),
            by_hash: HashMap::new(),
            by_key: HashMap::new(),
            hosted: false,
            next_version: 1,
            stats: SessionStats::default(),
            settings: Settings::default(),
            prefs: Prefs { alt_dl_limit: 0, alt_up_limit: 0, ..Prefs::defaults("", &Settings::default()) },
            username: "admin".into(),
            password: PasswordHash::new("x", 1),
            categories: BTreeMap::new(),
            categories_version: 1,
            torrent_categories: HashMap::new(),
            meta: HashMap::new(),
            options: patorrent_frontend::prefs::Prefs::default(),
            default_save_path: String::new(),
            listen_addr: SocketAddr::from(([127, 0, 0, 1], 8080)),
            gui_version: 1,
            scheduler: Scheduler::new(),
            scrapes: Scrapes::default(),
            torrent_files: TorrentFiles::new(),
            autoload: Poller::new(),
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
            user_paused: HashSet::new(),
            pending_top: HashSet::new(),
            rss_version: 1,
            awaiting: Default::default(),
            adds_version: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patorrent_core::InfoHashes;
    use patorrent_core::Sha1Hash;

    fn status(id: u64, v1: u8) -> TorrentStatus {
        TorrentStatus {
            id: TorrentId::from_raw(id),
            info_hashes: InfoHashes::v1(Sha1Hash([v1; 20])),
            name: format!("t{id}"),
            ..TorrentStatus::default()
        }
    }

    /// Hosted: a torrent's first sight takes the host's label and pause mark, `by_key` finds
    /// it by the host's key, and the Web UI's own changes land in the mirror as the host's keys
    /// (only real changes, as the journal records them).
    #[test]
    fn hosted_state_mirrors_labels_and_pause_marks() {
        let mut st = State::for_tests();
        st.hosted = true;
        st.options.track_changes();
        let key = Sha1Hash([7; 20]).to_hex();
        st.options.apply_changes(&[
            (format!("torrent.{key}.label"), Some("tv".to_owned())),
            (format!("torrent.{key}.paused"), Some("1".to_owned())),
        ]);
        st.update(status(1, 7), 0);
        st.update(status(2, 8), 0);
        let hash = torrent::hash_of(&InfoHashes::v1(Sha1Hash([7; 20])));
        assert_eq!(hash, key, "a v1 torrent's key is its hash");
        assert_eq!(st.by_key.get(&key), Some(&TorrentId::from_raw(1)));
        assert_eq!(st.torrent_categories.get(&hash).map(String::as_str), Some("tv"));
        assert!(st.categories.contains_key("tv") && st.user_paused.contains(&hash));
        assert!(st.options.take_changes().is_empty(), "the host's own keys are not journaled");

        // the Web UI's changes: the host's keys, removed keys as None
        let other = torrent::hash_of(&InfoHashes::v1(Sha1Hash([8; 20])));
        st.set_label(&other, "film");
        st.set_label(&hash, "");
        assert!(st.set_user_paused(&hash, false));
        assert!(!st.set_user_paused(&hash, false), "unchanged");
        assert!(st.set_user_paused(&other, true));
        let other_key = Sha1Hash([8; 20]).to_hex();
        assert_eq!(
            st.options.take_changes(),
            [
                (format!("torrent.{other_key}.label"), Some("film".to_owned())),
                (format!("torrent.{key}.label"), None),
                (format!("torrent.{key}.paused"), None),
                (format!("torrent.{other_key}.paused"), Some("1".to_owned())),
            ]
        );
        assert_eq!(st.torrent_categories.get(&other).map(String::as_str), Some("film"));
        assert!(!st.torrent_categories.contains_key(&hash) && !st.user_paused.contains(&hash));

        // nox: the same calls keep the lists and write no keys
        let mut st = State::for_tests();
        st.options.track_changes();
        st.update(status(1, 7), 0);
        st.set_label(&hash, "tv");
        st.set_user_paused(&hash, true);
        assert_eq!(st.torrent_categories.get(&hash).map(String::as_str), Some("tv"));
        assert!(st.options.take_changes().is_empty());
    }
}
