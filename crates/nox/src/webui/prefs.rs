//! Preferences (docs/WEBUI.md slice 6): the model behind `GET` and `POST /pa/prefs`, generated
//! from `frontend::options` and `frontend::settings` and laid out as µTorrent 2.2.1's pages,
//! the way the Win32 dialog (`crates/win32/src/preferences.rs`) lays them out.
//!
//! Three kinds of field share one table: every engine setting by its libtorrent name (the
//! `frontend::settings::TABLE` rows, the Advanced page), the frontend's options nox has an
//! equivalent for (`GuiOptions`: the folders, the per-torrent defaults, the IP filter) and the
//! Web UI's own preferences (`webui.*`). The page learns the fields and their placement from
//! `GET /pa/prefs`, so a new field needs no page code except a place on a page.
//!
//! A `POST` carries a partial object of ids to values. Everything is parsed and set on a
//! [`Working`] copy first (with `frontend::settings::Entry::parse` for the engine settings and
//! the Win32 dialog's checks for the rest), then committed at once: the engine settings go out
//! through `apply_settings`, a new password is hashed, a changed domain list or CSRF flag holds
//! for the next request, and everything is stored in `State::options` (the frontend's
//! preferences keys, saved in the state file) for [`restore`] at the next start. The compat
//! shim's `app/setPreferences` maps its names onto these ids and takes the same path.

use std::path::Path;

use patorrent_core::{IpFilter, ProxyType, Settings, TorrentFlags};
use patorrent_frontend::options::{self, GuiOptions};
use patorrent_frontend::scheduler;
use patorrent_frontend::settings::{self, Entry, Kind as SettingKind, TABLE, Value};

use super::auth::PasswordHash;
use super::json::Json;
use super::{Prefs, Shared, State, unix_now};

/// A field's type, as the page sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    Text,
    Folder,
    /// A choice among labels; the value is the index.
    Enum(&'static [&'static str]),
    /// Write-only: the value read back is always empty, an empty value sent changes nothing.
    Password,
    /// The scheduler's week: 168 `frontend::scheduler::Mode` codes, Monday 00:00 first.
    Schedule,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Bool => "bool",
            Kind::Int { .. } => "int",
            Kind::Text => "text",
            Kind::Folder => "folder",
            Kind::Enum(_) => "enum",
            Kind::Password => "password",
            Kind::Schedule => "schedule",
        }
    }
}

/// The copy a `POST` edits before anything is committed. `settings` carries the *normal*
/// global rate limits (`Prefs::dl_limit` / `up_limit`), not the ones in force when the
/// alternative limits are on, so the `download_rate_limit` and `upload_rate_limit` rows edit
/// what the Bandwidth page shows.
#[derive(Clone)]
pub(crate) struct Working {
    pub settings: Settings,
    pub prefs: Prefs,
    pub username: String,
    pub new_password: Option<String>,
    pub listen: String,
}

impl Working {
    pub fn from_state(st: &State) -> Working {
        let mut settings = st.settings.clone();
        settings.download_rate_limit = st.prefs.dl_limit;
        settings.upload_rate_limit = st.prefs.up_limit;
        Working {
            settings,
            prefs: st.prefs.clone(),
            username: st.username.clone(),
            new_password: None,
            listen: st.listen_addr.to_string(),
        }
    }

    /// The defaults every field is compared with.
    fn defaults(st: &State) -> Working {
        Working {
            settings: Settings::default(),
            prefs: Prefs::defaults(&st.default_save_path, &Settings::default()),
            username: "admin".to_owned(),
            new_password: None,
            listen: st.listen_addr.to_string(),
        }
    }
}

/// A field that is not an engine setting row.
pub(crate) struct Field {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub readonly: bool,
    get: fn(&Working) -> Value,
    set: fn(&mut Working, Value),
}

fn boolean(v: &Value) -> bool {
    matches!(v, Value::Bool(true)) || matches!(v, Value::Int(i) if *i != 0)
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        Value::Bool(b) => *b as i64,
        Value::Str(s) => s.trim().parse().unwrap_or(0),
    }
}

fn string(v: Value) -> String {
    match v {
        Value::Str(s) => s,
        other => other.to_string(),
    }
}

/// The proxy "Type" combo, in µTorrent 2.2.1's order (its "HTTPS" entry is our HTTP type);
/// "Authentication" picks the `_pw` variant, SOCKS4 sends the user name as its user id.
const PROXY_TYPES: [&str; 4] = ["(none)", "Socks4", "Socks5", "HTTP"];
/// "Outgoing" encryption, in the combo's order.
const ENC_OUT: [&str; 3] = ["Disabled", "Enabled", "Forced"];

fn proxy_to_ui(s: &Settings) -> (i64, bool) {
    match s.proxy_type {
        ProxyType::None => (0, false),
        ProxyType::Socks4 => (1, !s.proxy_username.is_empty()),
        ProxyType::Socks5 => (2, false),
        ProxyType::Socks5Pw => (2, true),
        ProxyType::Http => (3, false),
        ProxyType::HttpPw => (3, true),
    }
}

fn proxy_from_ui(kind: i64, auth: bool) -> ProxyType {
    match (kind, auth) {
        (1, _) => ProxyType::Socks4,
        (2, false) => ProxyType::Socks5,
        (2, true) => ProxyType::Socks5Pw,
        (3, false) => ProxyType::Http,
        (3, true) => ProxyType::HttpPw,
        _ => ProxyType::None,
    }
}

fn enc_out(s: &Settings) -> i64 {
    match s.out_enc_policy {
        patorrent_core::EncPolicy::Disabled => 0,
        patorrent_core::EncPolicy::Enabled => 1,
        patorrent_core::EncPolicy::Forced => 2,
    }
}

fn enc_policy(i: i64) -> patorrent_core::EncPolicy {
    match i {
        0 => patorrent_core::EncPolicy::Disabled,
        2 => patorrent_core::EncPolicy::Forced,
        _ => patorrent_core::EncPolicy::Enabled,
    }
}

const RATE_MAX: i64 = u32::MAX as i64;
const YEAR: i64 = 86_400 * 365;

macro_rules! field {
    ($id:expr, $label:expr, $kind:expr, |$w:ident| $get:expr, |$m:ident, $v:ident| $set:expr) => {
        Field { id: $id, label: $label, kind: $kind, readonly: false, get: |$w| $get, set: |$m, $v| $set }
    };
}

/// The fields that are not engine setting rows: the frontend's options nox has an equivalent
/// for (their `GuiOptions` preferences keys as ids), the page's composite controls over several
/// settings (as the Win32 dialog composes them) and the Web UI's own preferences.
pub(crate) static FIELDS: &[Field] = &[
    // Directories
    field!(
        "dir.download",
        "Put new downloads in:",
        Kind::Folder,
        |w| Value::Str(w.prefs.save_path.clone()),
        |w, v| w.prefs.save_path = string(v).trim().to_owned()
    ),
    field!(
        "dir.move_completed",
        "Move completed downloads to:",
        Kind::Bool,
        |w| Value::Bool(w.prefs.move_completed),
        |w, v| w.prefs.move_completed = boolean(&v)
    ),
    field!("dir.completed", "", Kind::Folder, |w| Value::Str(w.prefs.completed_dir.clone()), |w, v| w
        .prefs
        .completed_dir =
        string(v).trim().to_owned()),
    field!(
        "dir.move_only_default",
        "Only move from the default download directory",
        Kind::Bool,
        |w| Value::Bool(w.prefs.move_only_default),
        |w, v| w.prefs.move_only_default = boolean(&v)
    ),
    field!(
        "dir.store_torrents",
        "Store .torrents in:",
        Kind::Bool,
        |w| Value::Bool(w.prefs.torrent_folders.store),
        |w, v| w.prefs.torrent_folders.store = boolean(&v)
    ),
    field!(
        "dir.torrents",
        "",
        Kind::Folder,
        |w| Value::Str(w.prefs.torrent_folders.store_dir.clone()),
        |w, v| w.prefs.torrent_folders.store_dir = string(v).trim().to_owned()
    ),
    field!(
        "dir.move_torrents",
        "Move .torrents for finished jobs to:",
        Kind::Bool,
        |w| Value::Bool(w.prefs.torrent_folders.move_finished),
        |w, v| w.prefs.torrent_folders.move_finished = boolean(&v)
    ),
    field!(
        "dir.finished_torrents",
        "",
        Kind::Folder,
        |w| Value::Str(w.prefs.torrent_folders.finished_dir.clone()),
        |w, v| w.prefs.torrent_folders.finished_dir = string(v).trim().to_owned()
    ),
    field!(
        "dir.autoload",
        "Automatically load .torrents from:",
        Kind::Bool,
        |w| Value::Bool(w.prefs.torrent_folders.autoload),
        |w, v| w.prefs.torrent_folders.autoload = boolean(&v)
    ),
    field!(
        "dir.autoload_dir",
        "",
        Kind::Folder,
        |w| Value::Str(w.prefs.torrent_folders.autoload_dir.clone()),
        |w, v| w.prefs.torrent_folders.autoload_dir = string(v).trim().to_owned()
    ),
    field!(
        "dir.autoload_delete",
        "Delete loaded .torrents",
        Kind::Bool,
        |w| Value::Bool(w.prefs.torrent_folders.autoload_delete),
        |w, v| w.prefs.torrent_folders.autoload_delete = boolean(&v)
    ),
    // UI Settings
    field!(
        "ui.confirm_delete",
        "Confirm when deleting torrents",
        Kind::Bool,
        |w| Value::Bool(w.prefs.confirm_delete),
        |w, v| w.prefs.confirm_delete = boolean(&v)
    ),
    field!(
        "ui.add_stopped",
        "Don't start the download automatically",
        Kind::Bool,
        |w| Value::Bool(w.prefs.start_paused),
        |w, v| w.prefs.start_paused = boolean(&v)
    ),
    field!(
        "ui.show_add_dialog",
        "Show a window that displays the files inside the torrent",
        Kind::Bool,
        |w| Value::Bool(w.prefs.show_add_dialog),
        |w, v| w.prefs.show_add_dialog = boolean(&v)
    ),
    // Connection
    field!(
        "listen_port",
        "Port used for incoming connections:",
        Kind::Int { min: 1, max: 65535 },
        |w| Value::Int(settings::listen_port(&w.settings).map_or(0, i64::from)),
        |w, v| settings::set_listen_port(&mut w.settings, int(&v) as u16)
    ),
    field!(
        "net.random_port",
        "Randomize port each start",
        Kind::Bool,
        |w| Value::Bool(w.prefs.random_port),
        |w, v| w.prefs.random_port = boolean(&v)
    ),
    field!(
        "proxy_kind",
        "Type:",
        Kind::Enum(&PROXY_TYPES),
        |w| Value::Int(proxy_to_ui(&w.settings).0),
        |w, v| w.settings.proxy_type = proxy_from_ui(int(&v), proxy_to_ui(&w.settings).1)
    ),
    field!(
        "proxy_auth",
        "Authentication",
        Kind::Bool,
        |w| Value::Bool(proxy_to_ui(&w.settings).1),
        |w, v| w.settings.proxy_type = proxy_from_ui(proxy_to_ui(&w.settings).0, boolean(&v))
    ),
    // Bandwidth
    field!(
        "bt.max_peers",
        "Maximum number of connected peers per torrent: [0: no limit]",
        Kind::Int { min: 0, max: 65535 },
        |w| Value::Int(w.prefs.max_peers_per_torrent as i64),
        |w, v| w.prefs.max_peers_per_torrent = int(&v) as u32
    ),
    field!(
        "bt.upload_slots",
        "Number of upload slots per torrent: [0: no limit]",
        Kind::Int { min: 0, max: 65535 },
        |w| Value::Int(w.prefs.upload_slots_per_torrent as i64),
        |w, v| w.prefs.upload_slots_per_torrent = int(&v) as u32
    ),
    field!(
        "alt_up_limit",
        "Alternative upload rate (kB/s): [0: unlimited]",
        Kind::Int { min: 0, max: RATE_MAX },
        |w| Value::Int(w.prefs.alt_up_limit as i64),
        |w, v| w.prefs.alt_up_limit = int(&v) as u32
    ),
    field!(
        "alt_dl_limit",
        "Alternative download rate (kB/s): [0: unlimited]",
        Kind::Int { min: 0, max: RATE_MAX },
        |w| Value::Int(w.prefs.alt_dl_limit as i64),
        |w, v| w.prefs.alt_dl_limit = int(&v) as u32
    ),
    field!(
        "alt_enabled",
        "Use the alternative rate limits now",
        Kind::Bool,
        |w| Value::Bool(w.prefs.alt_enabled),
        |w, v| w.prefs.alt_enabled = boolean(&v)
    ),
    // BitTorrent
    field!("bt.pex", "Enable Peer Exchange", Kind::Bool, |w| Value::Bool(w.prefs.pex), |w, v| w.prefs.pex =
        boolean(&v)),
    // uTP both ways; the four transports one by one are bt.transp_disposition on Advanced
    field!(
        "utp",
        "Enable uTP (UDP transport with bandwidth management)",
        Kind::Bool,
        |w| Value::Bool(w.settings.enable_outgoing_utp || w.settings.enable_incoming_utp),
        |w, v| (w.settings.enable_outgoing_utp, w.settings.enable_incoming_utp) = (boolean(&v), boolean(&v))
    ),
    field!("enc_out", "Outgoing:", Kind::Enum(&ENC_OUT), |w| Value::Int(enc_out(&w.settings)), |w, v| {
        let legacy = settings::allow_legacy_incoming(&w.settings);
        settings::set_encryption(&mut w.settings, enc_policy(int(&v)), legacy)
    }),
    field!(
        "enc_legacy",
        "Allow incoming legacy connections",
        Kind::Bool,
        |w| Value::Bool(settings::allow_legacy_incoming(&w.settings)),
        |w, v| {
            let out = w.settings.out_enc_policy;
            settings::set_encryption(&mut w.settings, out, boolean(&v))
        }
    ),
    field!("ipfilter.enable", "Enable IP filter", Kind::Bool, |w| Value::Bool(w.prefs.ip_filter), |w, v| w
        .prefs
        .ip_filter =
        boolean(&v)),
    field!("ipfilter.path", "", Kind::Text, |w| Value::Str(w.prefs.ip_filter_path.clone()), |w, v| w
        .prefs
        .ip_filter_path =
        string(v).trim().to_owned()),
    // Scheduler
    field!(
        "sched.enable",
        "Enable Scheduler",
        Kind::Bool,
        |w| Value::Bool(w.prefs.schedule.enabled),
        |w, v| w.prefs.schedule.enabled = boolean(&v)
    ),
    field!("sched.table", "", Kind::Schedule, |w| Value::Str(w.prefs.schedule.table_text()), |w, v| {
        if let Some(t) = scheduler::parse_table(&string(v)) {
            w.prefs.schedule.table = t;
        }
    }),
    field!(
        "sched.ul_rate",
        "Limited upload rate (kB/s): [0: unlimited]",
        Kind::Int { min: 0, max: RATE_MAX / 1024 },
        |w| Value::Int(w.prefs.schedule.limited_up_kib as i64),
        |w, v| w.prefs.schedule.limited_up_kib = int(&v) as u32
    ),
    field!(
        "sched.dl_rate",
        "Limited download rate (kB/s): [0: unlimited]",
        Kind::Int { min: 0, max: RATE_MAX / 1024 },
        |w| Value::Int(w.prefs.schedule.limited_down_kib as i64),
        |w, v| w.prefs.schedule.limited_down_kib = int(&v) as u32
    ),
    field!(
        "sched.dis_dht",
        "Disable DHT when turning off",
        Kind::Bool,
        |w| Value::Bool(w.prefs.schedule.disable_dht),
        |w, v| w.prefs.schedule.disable_dht = boolean(&v)
    ),
    // Web UI
    Field {
        id: "webui.listen",
        label: "Listening address:",
        kind: Kind::Text,
        readonly: true,
        get: |w| Value::Str(w.listen.clone()),
        set: |_, _| {},
    },
    field!("webui.username", "Username:", Kind::Text, |w| Value::Str(w.username.clone()), |w, v| w
        .username =
        string(v).trim().to_owned()),
    field!("webui.password", "Password:", Kind::Password, |_w| Value::Str(String::new()), |w, v| {
        let p = string(v);
        if !p.is_empty() {
            w.new_password = Some(p);
        }
    }),
    field!(
        "webui.bypass_local_auth",
        "Bypass authentication for clients on localhost",
        Kind::Bool,
        |w| Value::Bool(w.prefs.bypass_local_auth),
        |w, v| w.prefs.bypass_local_auth = boolean(&v)
    ),
    field!(
        "webui.host_header_validation",
        "Enable Host header validation",
        Kind::Bool,
        |w| Value::Bool(w.prefs.host_header_validation),
        |w, v| w.prefs.host_header_validation = boolean(&v)
    ),
    field!(
        "webui.domain_list",
        "Server domains (separate multiple entries with a semicolon):",
        Kind::Text,
        |w| Value::Str(w.prefs.domain_list.clone()),
        |w, v| w.prefs.domain_list = string(v).trim().to_owned()
    ),
    field!(
        "webui.csrf",
        "Enable Cross-Site Request Forgery (CSRF) protection",
        Kind::Bool,
        |w| Value::Bool(w.prefs.csrf_protection),
        |w, v| w.prefs.csrf_protection = boolean(&v)
    ),
    field!(
        "webui.max_auth_fail_count",
        "Ban client after consecutive failures:",
        Kind::Int { min: 0, max: 1000 },
        |w| Value::Int(w.prefs.max_auth_fail_count as i64),
        |w, v| w.prefs.max_auth_fail_count = int(&v) as u32
    ),
    field!(
        "webui.ban_duration",
        "Ban for (seconds):",
        Kind::Int { min: 1, max: YEAR },
        |w| Value::Int(w.prefs.ban_duration as i64),
        |w, v| w.prefs.ban_duration = int(&v) as u64
    ),
    field!(
        "webui.session_timeout",
        "Session timeout (seconds):",
        Kind::Int { min: 1, max: YEAR },
        |w| Value::Int(w.prefs.session_timeout as i64),
        |w, v| w.prefs.session_timeout = int(&v) as u64
    ),
];

/// The 2.2.1 wording of the engine settings the pages show (the Advanced table shows the
/// libtorrent names); a row not listed here is labelled by its name.
const LABELS: &[(&str, &str)] = &[
    ("enable_upnp", "Enable UPnP port mapping"),
    ("enable_natpmp", "Enable NAT-PMP port mapping"),
    ("proxy_hostname", "Proxy:"),
    ("proxy_port", "Port:"),
    ("proxy_username", "Username:"),
    ("proxy_password", "Password:"),
    ("proxy_hostnames", "Resolve hostnames through proxy"),
    ("proxy_peer_connections", "Use proxy for peer-to-peer connections"),
    ("proxy_tracker_connections", "Use proxy for tracker connections"),
    ("upload_rate_limit", "Maximum upload rate (kB/s): [0: unlimited]"),
    ("download_rate_limit", "Maximum download rate (kB/s): [0: unlimited]"),
    ("connections_limit", "Global maximum number of connections:"),
    ("unchoke_slots_limit", "Global number of upload slots: [-1: no limit]"),
    ("enable_dht", "Enable DHT Network"),
    ("enable_lsd", "Enable Local Peer Discovery"),
    ("active_limit", "Maximum number of active torrents (upload or download): [-1: no limit]"),
    ("active_downloads", "Maximum number of active downloads: [-1: no limit]"),
    ("active_seeds", "Maximum number of active seeds: [-1: no limit]"),
    ("dont_count_slow_torrents", "Don't count slow torrents toward these limits"),
    ("max_queued_disk_bytes", "Disk write queue (kB):"),
    ("file_pool_size", "Maximum number of open files:"),
    ("aio_threads", "Disk I/O threads (used from the next start):"),
    ("hashing_threads", "Hashing threads (used from the next start):"),
];

/// One editable thing: an engine setting row or one of [`FIELDS`].
#[derive(Clone, Copy)]
pub(crate) enum Def {
    Setting(&'static Entry),
    Own(&'static Field),
}

impl Def {
    pub fn id(&self) -> &'static str {
        match self {
            Def::Setting(e) => e.name,
            Def::Own(f) => f.id,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Def::Setting(e) => LABELS.iter().find(|(n, _)| *n == e.name).map_or(e.name, |(_, l)| l),
            Def::Own(f) => f.label,
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Def::Setting(e) => match e.kind {
                SettingKind::Bool => Kind::Bool,
                SettingKind::Int { min, max } => Kind::Int { min, max },
                SettingKind::Str => Kind::Text,
            },
            Def::Own(f) => f.kind,
        }
    }

    fn readonly(&self) -> bool {
        matches!(self, Def::Own(f) if f.readonly)
    }

    pub fn get(&self, w: &Working) -> Value {
        match self {
            Def::Setting(e) => e.get(&w.settings),
            Def::Own(f) => (f.get)(w),
        }
    }

    fn set(&self, w: &mut Working, v: Value) {
        match self {
            Def::Setting(e) => {
                let _ = e.set(&mut w.settings, &v.to_string());
            }
            Def::Own(f) => (f.set)(w, v),
        }
    }

    /// Parses a value for this field: the engine settings through `Entry::parse`, the rest by
    /// their kind the same way. The error says why, without the field's name.
    pub fn parse(&self, text: &str) -> Result<Value, String> {
        if let Def::Setting(e) = self {
            return e
                .parse(text)
                .map_err(|m| m.strip_prefix(&format!("{}: ", e.name)).unwrap_or(&m).to_owned());
        }
        let t = text.trim();
        match self.kind() {
            Kind::Bool => match t.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Ok(Value::Bool(true)),
                "false" | "0" | "no" | "off" => Ok(Value::Bool(false)),
                _ => Err(format!("expected true or false, not \"{t}\"")),
            },
            Kind::Int { min, max } => match t.parse::<i64>() {
                Ok(i) if (min..=max).contains(&i) => Ok(Value::Int(i)),
                Ok(_) => Err(format!("{t} is outside {min}..{max}")),
                Err(_) => Err(format!("\"{t}\" is not a number")),
            },
            Kind::Enum(options) => match t.parse::<i64>() {
                Ok(i) if (0..options.len() as i64).contains(&i) => Ok(Value::Int(i)),
                Ok(_) => Err(format!("{t} is outside 0..{}", options.len() - 1)),
                Err(_) => options
                    .iter()
                    .position(|o| o.eq_ignore_ascii_case(t))
                    .map(|i| Value::Int(i as i64))
                    .ok_or_else(|| format!("\"{t}\" is not one of {}", options.join(", "))),
            },
            Kind::Text | Kind::Folder | Kind::Password => Ok(Value::Str(text.to_owned())),
            Kind::Schedule => match scheduler::parse_table(t) {
                Some(_) => Ok(Value::Str(t.to_owned())),
                None => Err(format!(
                    "expected {} hours, each 0 (full speed), 1 (turn off), 2 (limited) or 3 (seeding only)",
                    scheduler::CELLS
                )),
            },
        }
    }
}

pub(crate) fn find(id: &str) -> Option<Def> {
    FIELDS.iter().find(|f| f.id == id).map(Def::Own).or_else(|| settings::find(id).map(Def::Setting))
}

/// Every field once: the engine settings in the table's order, then [`FIELDS`].
fn all() -> impl Iterator<Item = Def> {
    TABLE.iter().map(Def::Setting).chain(FIELDS.iter().map(Def::Own))
}

fn json_of(v: &Value) -> Json {
    match v {
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::Int(*i),
        Value::Str(s) => Json::Str(s.clone()),
    }
}

/// A JSON value as the text the parsers take.
fn text_of(v: &Json) -> Option<String> {
    match v {
        Json::Bool(b) => Some(b.to_string()),
        Json::Int(i) => Some(i.to_string()),
        Json::Float(_) => v.as_i64().map(|i| i.to_string()),
        Json::Str(s) => Some(s.clone()),
        _ => None,
    }
}

// ---- the page layout -----------------------------------------------------------------------

fn f(id: &str) -> Json {
    Json::obj().with("f", id)
}

/// A field shown in kB/s (or kB) on the page; the value is bytes.
fn kb(id: &str) -> Json {
    f(id).with("kb", true)
}

fn ranged(id: &str, min: i64, max: i64) -> Json {
    f(id).with("min", min).with("max", max)
}

/// Greys `item` while the check box `id` is off (several ids, space-separated: while any is).
fn enabled_by(item: Json, id: &str) -> Json {
    item.with("enabled_by", id)
}

fn row(items: Vec<Json>) -> Json {
    Json::obj().with("row", Json::Arr(items))
}

/// A control that is a placeholder until its feature exists (a disabled check box, or a
/// disabled text field with `text`).
fn stub(label: &str) -> Json {
    Json::obj().with("stub", label)
}

fn note(text: impl Into<String>) -> Json {
    Json::obj().with("note", text.into())
}

fn group(title: Option<&str>, items: Vec<Json>) -> Json {
    Json::obj().with("title", title.map_or(Json::Null, Json::from)).with("items", Json::Arr(items))
}

fn page(id: &str, title: &str, groups: Vec<Json>) -> Json {
    Json::obj().with("id", id).with("title", title).with("groups", Json::Arr(groups))
}

/// The tree, in µTorrent 2.2.1's order; Disk Cache sits under Advanced.
fn pages(settings_dir: &Path) -> Vec<Json> {
    let ipf_default = settings_dir.join("ipfilter.dat");
    vec![
        page(
            "general",
            "General",
            vec![
                group(Some("Language"), vec![stub("User interface:").with("text", "English")]),
                group(
                    Some("Windows Integration"),
                    vec![
                        stub("Associate with .torrent files"),
                        stub("Associate with magnet: links"),
                        stub("Start PaTorrent on system startup"),
                        note("File associations and autostart belong to the desktop client."),
                    ],
                ),
            ],
        ),
        page(
            "ui",
            "UI Settings",
            vec![
                group(Some("Display Options"), vec![f("ui.confirm_delete")]),
                group(Some("When Adding Torrents"), vec![f("ui.add_stopped"), f("ui.show_add_dialog")]),
            ],
        ),
        page(
            "directories",
            "Directories",
            vec![
                group(
                    Some("Location of Downloaded Files"),
                    vec![
                        f("dir.download"),
                        f("dir.move_completed"),
                        enabled_by(f("dir.completed"), "dir.move_completed"),
                        enabled_by(f("dir.move_only_default"), "dir.move_completed"),
                    ],
                ),
                group(
                    Some("Location of .torrents"),
                    vec![
                        f("dir.store_torrents"),
                        enabled_by(f("dir.torrents"), "dir.store_torrents"),
                        enabled_by(f("dir.move_torrents"), "dir.store_torrents"),
                        enabled_by(f("dir.finished_torrents"), "dir.store_torrents dir.move_torrents"),
                        f("dir.autoload"),
                        enabled_by(f("dir.autoload_dir"), "dir.autoload"),
                        enabled_by(f("dir.autoload_delete"), "dir.autoload"),
                    ],
                ),
            ],
        ),
        page(
            "connection",
            "Connection",
            vec![
                group(
                    Some("Listening Port"),
                    vec![
                        row(vec![
                            f("listen_port"),
                            Json::obj().with("button", "Random port").with("action", "random_port"),
                        ]),
                        f("net.random_port"),
                        f("enable_upnp"),
                        f("enable_natpmp"),
                    ],
                ),
                group(
                    Some("Proxy Server"),
                    vec![
                        row(vec![f("proxy_kind"), f("proxy_hostname"), ranged("proxy_port", 0, 65535)]),
                        f("proxy_auth"),
                        row(vec![
                            enabled_by(f("proxy_username"), "proxy_auth"),
                            enabled_by(f("proxy_password").with("secret", true), "proxy_auth"),
                        ]),
                        f("proxy_hostnames"),
                        f("proxy_peer_connections"),
                        f("proxy_tracker_connections"),
                    ],
                ),
            ],
        ),
        page(
            "bandwidth",
            "Bandwidth",
            vec![
                group(Some("Global Upload Rate Limiting"), vec![kb("upload_rate_limit")]),
                group(Some("Global Download Rate Limiting"), vec![kb("download_rate_limit")]),
                group(
                    Some("Number of Connections"),
                    vec![
                        ranged("connections_limit", 2, 65535),
                        f("bt.max_peers"),
                        f("bt.upload_slots"),
                        ranged("unchoke_slots_limit", -1, 65535),
                    ],
                ),
                group(None, vec![note("The per-torrent limits apply to torrents added from now on.")]),
                group(
                    Some("Alternative Rate Limits"),
                    vec![kb("alt_up_limit"), kb("alt_dl_limit"), f("alt_enabled")],
                ),
            ],
        ),
        page(
            "bittorrent",
            "BitTorrent",
            vec![
                group(
                    Some("Basic BitTorrent Features"),
                    vec![f("enable_dht"), f("enable_lsd"), f("bt.pex"), f("utp")],
                ),
                group(Some("Protocol Encryption"), vec![f("enc_out"), f("enc_legacy")]),
                group(
                    Some("IP Filter"),
                    vec![
                        f("ipfilter.enable"),
                        enabled_by(f("ipfilter.path"), "ipfilter.enable"),
                        note(format!("An empty path loads {}.", ipf_default.display())),
                    ],
                ),
            ],
        ),
        page(
            "queueing",
            "Queueing",
            vec![
                group(
                    Some("Queue Settings"),
                    vec![
                        ranged("active_limit", -1, 65535),
                        ranged("active_downloads", -1, 65535),
                        ranged("active_seeds", -1, 65535),
                        f("dont_count_slow_torrents"),
                    ],
                ),
                group(
                    Some("Seeding Goal"),
                    vec![
                        stub("When torrents reach a ratio of (%):").with("text", ""),
                        note("Seeding goals are not available yet."),
                    ],
                ),
            ],
        ),
        page(
            "scheduler",
            "Scheduler",
            vec![
                group(None, vec![f("sched.enable")]),
                group(Some("Scheduler Table"), vec![enabled_by(f("sched.table"), "sched.enable")]),
                group(
                    Some("Scheduler Settings"),
                    vec![
                        enabled_by(f("sched.ul_rate"), "sched.enable"),
                        enabled_by(f("sched.dl_rate"), "sched.enable"),
                        enabled_by(f("sched.dis_dht"), "sched.enable"),
                    ],
                ),
            ],
        ),
        page(
            "webui",
            "Web UI",
            vec![
                group(None, vec![stub("Enable Web UI").with("on", true)]),
                group(
                    Some("Authentication"),
                    vec![f("webui.username"), f("webui.password"), f("webui.bypass_local_auth")],
                ),
                group(
                    Some("Connectivity"),
                    vec![
                        f("webui.listen"),
                        f("webui.host_header_validation"),
                        f("webui.domain_list"),
                        f("webui.csrf"),
                    ],
                ),
                group(
                    Some("Security"),
                    vec![f("webui.max_auth_fail_count"), f("webui.ban_duration"), f("webui.session_timeout")],
                ),
            ],
        ),
        page(
            "advanced",
            "Advanced",
            vec![group(
                None,
                vec![note(
                    "libtorrent setting names; changed values are shown in bold. Values are applied with OK or Apply.",
                )],
            )],
        )
        .with("advanced", true),
        page(
            "diskcache",
            "Disk Cache",
            vec![
                group(
                    Some("Disk Queue and Files"),
                    vec![
                        kb("max_queued_disk_bytes").with("min", 1024),
                        ranged("file_pool_size", 1, 65535),
                        ranged("aio_threads", 1, 64),
                        ranged("hashing_threads", 1, 64),
                    ],
                ),
                group(
                    None,
                    vec![note(
                        "PaTorrent has no disk read/write cache yet, so µTorrent's cache size settings have no \
                         counterpart; these are the limits of the engine's disk queue.",
                    )],
                ),
            ],
        )
        .with("under", "advanced"),
    ]
}

/// `GET /pa/prefs`: `pages` (the tree in order, each with its groups of items: `{"f": id}`
/// with optional `kb`, `min`, `max`, `enabled_by` (space-separated ids), `secret`;
/// `{"row": [items]}`; `{"stub": label}`; `{"note": text}`; `{"button": label, "action": name}`), `fields` (id to
/// `label`, `type`, `value`, `default`, `min` / `max` for ints, `options` for enums,
/// `readonly`) and `advanced` (every engine setting's id, the table's order).
pub(crate) fn model(shared: &Shared, st: &State) -> Json {
    let w = Working::from_state(st);
    let d = Working::defaults(st);
    let mut fields = Json::obj();
    for def in all() {
        let mut j = Json::obj()
            .with("label", def.label())
            .with("type", def.kind().name())
            .with("value", json_of(&def.get(&w)))
            .with("default", json_of(&def.get(&d)));
        match def.kind() {
            Kind::Int { min, max } => {
                j.set("min", min);
                j.set("max", max);
            }
            Kind::Enum(options) => {
                j.set("options", Json::Arr(options.iter().map(|o| Json::from(*o)).collect()));
            }
            _ => {}
        }
        if def.readonly() {
            j.set("readonly", true);
        }
        fields.set(def.id(), j);
    }
    let advanced: Vec<Json> = TABLE.iter().map(|e| Json::from(e.name)).collect();
    Json::obj()
        .with("pages", Json::Arr(pages(&shared.settings_dir)))
        .with("fields", fields)
        .with("advanced", Json::Arr(advanced))
}

// ---- applying a POST -----------------------------------------------------------------------

/// A refused change: the field and why.
pub(crate) type Refusal = (String, String);

/// One parsed change.
pub(crate) type Change = (Def, Value);

/// Parses one id and value.
pub(crate) fn parse_one(id: &str, v: &Json) -> Result<Change, Refusal> {
    let def = find(id).ok_or_else(|| (id.to_owned(), "unknown preference".to_owned()))?;
    if def.readonly() {
        return Err((id.to_owned(), "read-only".to_owned()));
    }
    let text = text_of(v).ok_or_else(|| (id.to_owned(), format!("unexpected value {}", v.to_json())))?;
    let value = def.parse(&text).map_err(|why| (id.to_owned(), why))?;
    Ok((def, value))
}

/// Parses a `POST /pa/prefs` body: a partial object of ids to values. Stops at the first bad
/// field.
pub(crate) fn parse(body: &Json) -> Result<Vec<Change>, Refusal> {
    body.entries().iter().map(|(k, v)| parse_one(k, v)).collect()
}

/// The Win32 dialog's checks across fields.
pub(crate) fn validate(w: &Working) -> Result<(), Refusal> {
    let refuse = |id: &str, why: &str| Err((id.to_owned(), why.to_owned()));
    if w.settings.proxy_type != ProxyType::None {
        if w.settings.proxy_hostname.trim().is_empty() {
            return refuse("proxy_hostname", "Please enter the proxy server's address.");
        }
        if w.settings.proxy_port == 0 {
            return refuse("proxy_port", "Please enter the proxy server's port.");
        }
    }
    if w.prefs.save_path.trim().is_empty() {
        return refuse("dir.download", "Please choose a folder for new downloads.");
    }
    if w.prefs.move_completed && w.prefs.completed_dir.is_empty() {
        return refuse("dir.completed", "Please choose a folder for completed downloads.");
    }
    let tf = &w.prefs.torrent_folders;
    if tf.store && tf.store_dir.is_empty() {
        return refuse("dir.torrents", "Please choose a folder to store .torrents in.");
    }
    if tf.store && tf.move_finished && tf.finished_dir.is_empty() {
        return refuse("dir.finished_torrents", "Please choose a folder for the .torrents of finished jobs.");
    }
    if tf.autoload && tf.autoload_dir.is_empty() {
        return refuse("dir.autoload_dir", "Please choose a folder to load .torrents from.");
    }
    if w.username.is_empty() {
        return refuse("webui.username", "Please enter a user name.");
    }
    Ok(())
}

/// Applies parsed changes to a working copy of `st`, checks it, and commits: engine settings
/// through `apply_settings` (with the global limits of the mode in force), the IP filter loaded
/// or cleared, peer exchange toggled on every torrent, a new password hashed, the folders
/// handed to the owner's `MoveCompleted`, and everything stored in the state file.
pub(crate) fn commit(shared: &Shared, st: &mut State, changes: &[Change]) -> Result<(), Refusal> {
    let mut w = Working::from_state(st);
    for (def, v) in changes {
        def.set(&mut w, v.clone());
    }
    validate(&w)?;
    let old = Working::from_state(st);

    // the IP filter first: a block list that does not load refuses the whole change
    let filter = |w: &Working| (w.prefs.ip_filter, w.prefs.gui().ip_filter_file(&shared.settings_dir));
    if filter(&w) != filter(&old) {
        let (on, file) = filter(&w);
        if on {
            match shared.handle.load_ip_filter(&file) {
                Ok(n) => {
                    st.push_log(None, format!("IP filter: {n} rules from {}", file.display()), unix_now())
                }
                Err(e) => return Err(("ipfilter.path".to_owned(), format!("{}: {e}", file.display()))),
            }
        } else {
            shared.handle.set_ip_filter(IpFilter::new());
            st.push_log(None, "IP filter off".to_owned(), unix_now());
        }
    }
    if w.prefs.pex != old.prefs.pex {
        for id in st.torrents.keys() {
            let t = shared.handle.torrent(*id);
            if w.prefs.pex {
                t.unset_flags(TorrentFlags::DISABLE_PEX);
            } else {
                t.set_flags(TorrentFlags::DISABLE_PEX);
            }
        }
    }
    let folders =
        |p: &Prefs| (p.move_completed, p.completed_dir.clone(), p.move_only_default, p.save_path.clone());
    if folders(&w.prefs) != folders(&old.prefs) {
        st.gui_version += 1;
    }
    w.prefs.dl_limit = w.settings.download_rate_limit;
    w.prefs.up_limit = w.settings.upload_rate_limit;
    let limits = |p: &Prefs| (p.dl_limit, p.up_limit, p.alt_dl_limit, p.alt_up_limit, p.alt_enabled);
    let engine_changed = w.settings != old.settings
        || limits(&w.prefs) != limits(&old.prefs)
        || w.prefs.schedule != old.prefs.schedule;
    st.prefs = w.prefs.clone();
    st.settings = w.settings.clone();
    if engine_changed {
        // sets the limits of the mode in force and sends every setting
        shared.apply_limits(st);
    }
    st.username = w.username.clone();
    if let Some(p) = &w.new_password {
        st.password = PasswordHash::new(p, shared.iterations);
    }
    let changed: Vec<&'static str> =
        TABLE.iter().filter(|e| e.get(&w.settings) != e.get(&old.settings)).map(|e| e.name).collect();
    store(st, &changed);
    shared.save(st);
    Ok(())
}

/// Writes the preferences into `State::options` (the frontend's keys): the engine settings as
/// `lt.<name>` with the normal rate limits (the ones that differ from the defaults, as the
/// Win32 client stores them, plus every row `changed` here or stored before, so a value set
/// back to its default still wins over the command line at the next start), the `GuiOptions`
/// keys (without the desktop-only tray ones), and the Web UI's own as `webui.<name>`.
fn store(st: &mut State, changed: &[&'static str]) {
    let mut normal = st.settings.clone();
    normal.download_rate_limit = st.prefs.dl_limit;
    normal.upload_rate_limit = st.prefs.up_limit;
    let kept: Vec<String> = st
        .options
        .entries()
        .filter_map(|(k, _)| k.strip_prefix(settings::PREF_PREFIX).map(str::to_owned))
        .collect();
    settings::store(&mut st.options, &normal);
    for name in kept.iter().map(String::as_str).chain(changed.iter().copied()) {
        if let Some(e) = settings::find(name).filter(|e| !e.name.starts_with("bt.")) {
            st.options.set(&format!("{}{}", settings::PREF_PREFIX, e.name), e.get(&normal));
        }
    }
    st.prefs.gui().store(&mut st.options);
    for k in ["ui.close_to_tray", "ui.minimize_to_tray", "ui.balloons"] {
        st.options.remove(k);
    }
    let p = &st.prefs;
    let o = &mut st.options;
    o.set("webui.alt_dl_limit", p.alt_dl_limit);
    o.set("webui.alt_up_limit", p.alt_up_limit);
    o.set_bool("webui.alt_enabled", p.alt_enabled);
    o.set_bool("webui.bypass_local_auth", p.bypass_local_auth);
    o.set_bool("webui.csrf", p.csrf_protection);
    o.set_bool("webui.host_header_validation", p.host_header_validation);
    o.set("webui.domain_list", &p.domain_list);
    o.set("webui.max_auth_fail_count", p.max_auth_fail_count);
    o.set("webui.ban_duration", p.ban_duration);
    o.set("webui.session_timeout", p.session_timeout);
}

/// At start: re-applies the stored preferences onto the settings the daemon was started with
/// (so a stored value wins over the command line, a value never stored keeps the command
/// line's), picks a random listen port when asked, and loads the IP filter unless the daemon
/// loaded that same file already (`loaded_ip_filter`).
pub(crate) fn restore(shared: &Shared, st: &mut State, loaded_ip_filter: Option<&Path>) {
    let o = &st.options;
    let mut changed = false;
    for e in TABLE.iter().filter(|e| !e.name.starts_with("bt.")) {
        if let Some(v) = o.get(&format!("{}{}", settings::PREF_PREFIX, e.name)) {
            changed |= e.set(&mut st.settings, v).is_ok();
        }
    }
    st.prefs.dl_limit = st.settings.download_rate_limit;
    st.prefs.up_limit = st.settings.upload_rate_limit;
    // the GuiOptions keys: the stored ones over the current values (the command line's)
    let mut filled = patorrent_frontend::prefs::Prefs::default();
    st.prefs.gui().store(&mut filled);
    for (k, v) in o.entries() {
        filled.set(k, v);
    }
    let gui = GuiOptions::load(&filled, &st.default_save_path);
    st.prefs.set_gui(&gui);
    let u32_of = |k: &str| o.get(k).and_then(|v| v.trim().parse::<u32>().ok());
    let u64_of = |k: &str| o.get(k).and_then(|v| v.trim().parse::<u64>().ok());
    let p = &st.prefs;
    let prefs = Prefs {
        alt_dl_limit: u32_of("webui.alt_dl_limit").unwrap_or(p.alt_dl_limit),
        alt_up_limit: u32_of("webui.alt_up_limit").unwrap_or(p.alt_up_limit),
        alt_enabled: o.get_bool("webui.alt_enabled", p.alt_enabled),
        bypass_local_auth: o.get_bool("webui.bypass_local_auth", p.bypass_local_auth),
        csrf_protection: o.get_bool("webui.csrf", p.csrf_protection),
        host_header_validation: o.get_bool("webui.host_header_validation", p.host_header_validation),
        domain_list: o.get("webui.domain_list").map_or(p.domain_list.clone(), str::to_owned),
        max_auth_fail_count: u32_of("webui.max_auth_fail_count").unwrap_or(p.max_auth_fail_count),
        ban_duration: u64_of("webui.ban_duration").unwrap_or(p.ban_duration),
        session_timeout: u64_of("webui.session_timeout").unwrap_or(p.session_timeout),
        ..p.clone()
    };
    changed |=
        prefs.alt_enabled || prefs.alt_dl_limit != p.alt_dl_limit || prefs.alt_up_limit != p.alt_up_limit;
    st.prefs = prefs;
    if st.prefs.random_port {
        settings::set_listen_port(&mut st.settings, options::random_port());
        changed = true;
    }
    if changed {
        shared.apply_limits(st);
    }
    let file = st.prefs.gui().ip_filter_file(&shared.settings_dir);
    if st.prefs.ip_filter {
        if loaded_ip_filter != Some(file.as_path()) {
            match shared.handle.load_ip_filter(&file) {
                Ok(n) => {
                    st.push_log(None, format!("IP filter: {n} rules from {}", file.display()), unix_now())
                }
                Err(e) => st.push_log(None, format!("IP filter: {}: {e}", file.display()), unix_now()),
            }
        }
    } else if loaded_ip_filter.is_some() {
        shared.handle.set_ip_filter(IpFilter::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        let mut st = State::for_tests();
        st.default_save_path = "/dl".to_owned();
        st.prefs = Prefs::defaults("/dl", &Settings::default());
        st
    }

    /// A working copy with `changes` applied (no engine, no commit).
    fn edited(st: &State, body: &str) -> Result<Working, Refusal> {
        let changes = parse(&super::super::json::parse(body).unwrap())?;
        let mut w = Working::from_state(st);
        for (def, v) in &changes {
            def.set(&mut w, v.clone());
        }
        validate(&w)?;
        Ok(w)
    }

    #[test]
    fn every_field_once_and_the_pages_stable() {
        let mut ids: Vec<&str> = all().map(|d| d.id()).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate ids");
        assert_eq!(n, TABLE.len() + FIELDS.len());
        for e in TABLE {
            assert!(matches!(find(e.name), Some(Def::Setting(_))), "{}", e.name);
        }
        for f in FIELDS {
            assert!(matches!(find(f.id), Some(Def::Own(_))), "{}", f.id);
            assert!(settings::find(f.id).is_none(), "{} shadows an engine setting", f.id);
        }
        assert!(find("nope").is_none());

        let st = state();
        // the layout without a Shared: the page list is static but for one note
        let names: Vec<&str> = TABLE.iter().map(|e| e.name).collect();
        let w = Working::from_state(&st);
        for def in all() {
            // every field reads back its own default on a fresh state
            let d = Working::defaults(&st);
            assert_eq!(def.get(&w), def.get(&d), "{}", def.id());
        }
        assert_eq!(find("enable_dht").unwrap().get(&w), Value::Bool(true));
        assert_eq!(find("webui.max_auth_fail_count").unwrap().get(&w), Value::Int(5));
        assert_eq!(find("alt_dl_limit").unwrap().get(&w), Value::Int(10 * 1024));
        assert_eq!(find("dir.download").unwrap().get(&w), Value::Str("/dl".into()));
        assert_eq!(find("listen_port").unwrap().get(&w), Value::Int(6881));
        assert_eq!(find("webui.listen").unwrap().get(&w), Value::Str("127.0.0.1:8080".into()));
        assert!(names.contains(&"user_agent") && names.contains(&"bt.transp_disposition"));
    }

    #[test]
    fn model_json_groups_every_page_field() {
        // the pages reference existing fields only, in 2.2.1's order
        let st = state();
        let w = Working::from_state(&st);
        let d = Working::defaults(&st);
        let mut fields = Json::obj();
        for def in all() {
            fields.set(def.id(), json_of(&def.get(&w)));
        }
        let pages_json = pages(Path::new("S"));
        let ids: Vec<String> =
            pages_json.iter().map(|p| p.get("id").and_then(Json::as_str).unwrap().to_owned()).collect();
        assert_eq!(
            ids,
            [
                "general",
                "ui",
                "directories",
                "connection",
                "bandwidth",
                "bittorrent",
                "queueing",
                "scheduler",
                "webui",
                "advanced",
                "diskcache"
            ]
        );
        let titles: Vec<&str> =
            pages_json.iter().map(|p| p.get("title").and_then(Json::as_str).unwrap()).collect();
        assert_eq!(titles[1], "UI Settings");
        assert_eq!(pages_json[10].get("under").and_then(Json::as_str), Some("advanced"));
        assert_eq!(pages_json[9].get("advanced"), Some(&Json::Bool(true)));
        let mut placed = Vec::new();
        fn walk(items: &[Json], placed: &mut Vec<String>) {
            for it in items {
                if let Some(id) = it.get("f").and_then(Json::as_str) {
                    placed.push(id.to_owned());
                }
                if let Some(Json::Arr(r)) = it.get("row") {
                    walk(r, placed);
                }
            }
        }
        for p in &pages_json {
            let Some(Json::Arr(groups)) = p.get("groups") else { panic!() };
            for g in groups {
                let Some(Json::Arr(items)) = g.get("items") else { panic!() };
                walk(items, &mut placed);
            }
        }
        for id in &placed {
            assert!(fields.get(id).is_some(), "page field {id} is not a field");
        }
        let mut sorted = placed.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), placed.len(), "a field placed twice");
        // every non-setting field is on some page
        for f in FIELDS {
            assert!(placed.contains(&f.id.to_owned()), "{} is on no page", f.id);
        }
        assert!(d.prefs.show_add_dialog);
        assert_eq!(json_of(&Value::Int(3)), Json::Int(3));
    }

    #[test]
    fn parses_and_refuses() {
        let st = state();
        let w = edited(
            &st,
            r#"{"enable_dht":false,"listen_port":"7000","connections_limit":42,"ui.add_stopped":1}"#,
        )
        .unwrap();
        assert!(!w.settings.enable_dht && w.prefs.start_paused);
        assert_eq!(settings::listen_port(&w.settings), Some(7000));
        assert_eq!(w.settings.connections_limit, 42);
        assert_eq!(w.settings.listen_interfaces, "0.0.0.0:7000,[::]:7000");

        let bad = |body: &str| edited(&st, body).err().unwrap();
        assert_eq!(bad(r#"{"no_such":1}"#), ("no_such".to_owned(), "unknown preference".to_owned()));
        assert_eq!(bad(r#"{"connections_limit":"lots"}"#).0, "connections_limit");
        assert!(bad(r#"{"connections_limit":"lots"}"#).1.contains("not a number"));
        assert!(bad(r#"{"upload_rate_limit":-1}"#).1.contains("outside"));
        assert_eq!(bad(r#"{"listen_port":0}"#).1, "0 is outside 1..65535");
        assert_eq!(bad(r#"{"webui.listen":"x"}"#).1, "read-only");
        assert_eq!(bad(r#"{"enable_dht":"maybe"}"#).1, "expected true or false, not \"maybe\"");
        assert_eq!(bad(r#"{"enable_dht":null}"#).1, "unexpected value null");
        assert_eq!(bad(r#"{"proxy_kind":9}"#).1, "9 is outside 0..3");
        // the first bad field is named, in body order
        assert_eq!(bad(r#"{"enable_dht":false,"zzz":1,"yyy":2}"#).0, "zzz");

        // the Win32 dialog's checks
        assert_eq!(bad(r#"{"proxy_kind":2}"#).0, "proxy_hostname");
        assert_eq!(bad(r#"{"proxy_kind":"Socks5","proxy_hostname":"p"}"#).0, "proxy_port");
        let w =
            edited(&st, r#"{"proxy_kind":"HTTP","proxy_hostname":"p","proxy_port":8080,"proxy_auth":true}"#)
                .unwrap();
        assert_eq!(w.settings.proxy_type, ProxyType::HttpPw);
        let w = edited(&st, r#"{"proxy_kind":3,"proxy_hostname":"p","proxy_port":8080}"#).unwrap();
        assert_eq!(w.settings.proxy_type, ProxyType::Http);
        assert_eq!(bad(r#"{"dir.download":"  "}"#).0, "dir.download");
        assert_eq!(bad(r#"{"dir.move_completed":true}"#).0, "dir.completed");
        assert_eq!(bad(r#"{"webui.username":""}"#).0, "webui.username");

        // composites
        let w = edited(&st, r#"{"enc_out":"Forced","enc_legacy":false}"#).unwrap();
        assert_eq!(w.settings.out_enc_policy, patorrent_core::EncPolicy::Forced);
        assert_eq!(w.settings.in_enc_policy, patorrent_core::EncPolicy::Forced);
        assert_eq!(w.settings.allowed_enc_level, patorrent_core::EncLevel::Rc4);
        let w = edited(&st, r#"{"utp":false}"#).unwrap();
        assert!(
            !w.settings.enable_outgoing_utp
                && !w.settings.enable_incoming_utp
                && w.settings.enable_outgoing_tcp
        );
        // the password: write-only, empty means unchanged
        let w = edited(&st, r#"{"webui.password":""}"#).unwrap();
        assert_eq!(w.new_password, None);
        let w = edited(&st, r#"{"webui.password":"s3cret"}"#).unwrap();
        assert_eq!(w.new_password.as_deref(), Some("s3cret"));
        assert_eq!(find("webui.password").unwrap().get(&w), Value::Str(String::new()));
        // the rate limit rows edit the normal limits
        let w = edited(&st, r#"{"download_rate_limit":2048}"#).unwrap();
        assert_eq!(w.settings.download_rate_limit, 2048);
    }

    #[test]
    fn storing_keeps_the_frontend_keys() {
        let mut st = state();
        st.settings.enable_dht = false;
        st.prefs.dl_limit = 4096;
        st.settings.download_rate_limit = 1; // the alternative mode's value, not stored
        st.prefs.completed_dir = "/done".to_owned();
        st.prefs.move_completed = true;
        st.prefs.domain_list = "*.example".to_owned();
        store(&mut st, &["enable_lsd"]);
        let o = &st.options;
        assert_eq!(o.get("lt.enable_dht"), Some("false"));
        assert_eq!(o.get("lt.download_rate_limit"), Some("4096"));
        assert_eq!(o.get("lt.upload_rate_limit"), None, "defaults are not written");
        assert_eq!(o.get("lt.enable_lsd"), Some("true"), "a row set to its default on purpose is");
        assert_eq!(o.get("dir.completed"), Some("/done"));
        assert_eq!(o.get("dir.move_completed"), Some("1"));
        assert_eq!(o.get("ui.close_to_tray"), None, "desktop-only keys are not written");
        assert_eq!(o.get("webui.domain_list"), Some("*.example"));
        assert_eq!(o.get("webui.csrf"), Some("1"));
        // the model's values follow what is stored
        let w = Working::from_state(&st);
        assert_eq!(find("download_rate_limit").unwrap().get(&w), Value::Int(4096));
    }

    #[test]
    fn the_scheduler_fields_edit_the_schedule() {
        use patorrent_frontend::scheduler::{CELLS, Mode};
        let st = state();
        let mut table = "0".repeat(CELLS);
        table.replace_range(9..10, "2");
        table.replace_range(CELLS - 1.., "3");
        let body = format!(
            r#"{{"sched.enable":true,"sched.table":"{table}","sched.ul_rate":20,"sched.dl_rate":300,"sched.dis_dht":false}}"#
        );
        let w = edited(&st, &body).unwrap();
        let s = &w.prefs.schedule;
        assert!(s.enabled && !s.disable_dht);
        assert_eq!((s.limited_up_kib, s.limited_down_kib), (20, 300));
        assert_eq!((s.get(0, 9), s.get(6, 23), s.get(0, 0)), (Mode::Limited, Mode::SeedOnly, Mode::Full));
        assert_eq!(find("sched.table").unwrap().get(&w), Value::Str(table));
        assert_eq!(find("sched.table").unwrap().kind().name(), "schedule");

        let bad = |body: &str| edited(&st, body).err().unwrap();
        assert_eq!(bad(r#"{"sched.table":"0123"}"#).0, "sched.table");
        assert!(bad(&format!(r#"{{"sched.table":"{}9"}}"#, "0".repeat(CELLS - 1))).1.contains("168 hours"));
        assert!(bad(r#"{"sched.ul_rate":-1}"#).1.contains("outside"));

        // stored as the frontend's keys, as the Win32 client stores them
        let mut st = state();
        st.prefs.schedule = w.prefs.schedule.clone();
        store(&mut st, &[]);
        assert_eq!(st.options.get("sched.enable"), Some("1"));
        assert_eq!(st.options.get("sched.ul_rate"), Some("20"));
        assert_eq!(st.options.get("sched.table").map(str::len), Some(CELLS));
    }
}
