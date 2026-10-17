//! The string-keyed settings table: every [`Settings`] field by its libtorrent name, read and
//! written as text (µTorrent's Advanced preferences page, the nox API later), plus the µTorrent
//! settings that are composed of several libtorrent ones.
//!
//! Values are libtorrent's: booleans `true` / `false`, integers in decimal, the encryption
//! enums as libtorrent's numbers (`out_enc_policy` 0 forced, 1 enabled, 2 disabled;
//! `allowed_enc_level` 1 plaintext, 2 RC4, 3 both). The preferences file keeps the settings
//! that differ from the defaults as `lt.<name>=<value>` lines, so a later build's new defaults
//! reach everything the user never changed.

use patorrent_core::{EncLevel, EncPolicy, Settings};

use crate::prefs::Prefs;

/// A setting's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Str(String),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Str(s) => f.write_str(s),
        }
    }
}

/// A setting's type, which says how text is parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    /// An integer in `min..=max`.
    Int {
        min: i64,
        max: i64,
    },
    Str,
}

/// One row of the table.
pub struct Entry {
    pub name: &'static str,
    pub kind: Kind,
    get: fn(&Settings) -> Value,
    set: fn(&mut Settings, Value),
}

impl Entry {
    pub fn get(&self, s: &Settings) -> Value {
        (self.get)(s)
    }

    /// Parses `text` for this setting and stores it.
    pub fn set(&self, s: &mut Settings, text: &str) -> Result<(), String> {
        let v = self.parse(text)?;
        (self.set)(s, v);
        Ok(())
    }

    pub fn parse(&self, text: &str) -> Result<Value, String> {
        let t = text.trim();
        match self.kind {
            Kind::Bool => match t.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Ok(Value::Bool(true)),
                "false" | "0" | "no" | "off" => Ok(Value::Bool(false)),
                _ => Err(format!("{}: expected true or false, not \"{t}\"", self.name)),
            },
            Kind::Int { min, max } => match t.parse::<i64>() {
                Ok(i) if (min..=max).contains(&i) => Ok(Value::Int(i)),
                Ok(_) => Err(format!("{}: {t} is outside {min}..{max}", self.name)),
                Err(_) => Err(format!("{}: \"{t}\" is not a number", self.name)),
            },
            // strings are kept as typed (an empty one is meaningful)
            Kind::Str => Ok(Value::Str(text.to_owned())),
        }
    }

    pub fn is_default(&self, s: &Settings) -> bool {
        self.get(s) == self.get(&Settings::default())
    }
}

fn int(v: Value) -> i64 {
    match v {
        Value::Int(i) => i,
        Value::Bool(b) => b as i64,
        Value::Str(_) => 0,
    }
}

fn boolean(v: Value) -> bool {
    matches!(v, Value::Bool(true)) || matches!(v, Value::Int(i) if i != 0)
}

fn string(v: Value) -> String {
    match v {
        Value::Str(s) => s,
        other => other.to_string(),
    }
}

/// libtorrent's `enc_policy` numbers.
pub fn policy_to_int(p: EncPolicy) -> i64 {
    match p {
        EncPolicy::Forced => 0,
        EncPolicy::Enabled => 1,
        EncPolicy::Disabled => 2,
    }
}

pub fn policy_from_int(i: i64) -> EncPolicy {
    match i {
        0 => EncPolicy::Forced,
        2 => EncPolicy::Disabled,
        _ => EncPolicy::Enabled,
    }
}

/// libtorrent's `enc_level` numbers.
pub fn level_to_int(l: EncLevel) -> i64 {
    match l {
        EncLevel::Plaintext => 1,
        EncLevel::Rc4 => 2,
        EncLevel::Both => 3,
    }
}

pub fn level_from_int(i: i64) -> EncLevel {
    match i {
        1 => EncLevel::Plaintext,
        2 => EncLevel::Rc4,
        _ => EncLevel::Both,
    }
}

macro_rules! entry {
    ($f:ident: bool) => {
        Entry {
            name: stringify!($f),
            kind: Kind::Bool,
            get: |s| Value::Bool(s.$f),
            set: |s, v| s.$f = boolean(v),
        }
    };
    ($f:ident: str) => {
        Entry {
            name: stringify!($f),
            kind: Kind::Str,
            get: |s| Value::Str(s.$f.clone()),
            set: |s, v| s.$f = string(v),
        }
    };
    ($f:ident: $t:ty) => {
        Entry {
            name: stringify!($f),
            kind: Kind::Int { min: <$t>::MIN as i64, max: <$t>::MAX as i64 },
            get: |s| Value::Int(s.$f as i64),
            set: |s, v| s.$f = int(v) as $t,
        }
    };
}

/// µTorrent's `bt.transp_disposition` bits.
pub const TRANSP_OUT_TCP: u32 = 1;
pub const TRANSP_OUT_UTP: u32 = 2;
pub const TRANSP_IN_TCP: u32 = 4;
pub const TRANSP_IN_UTP: u32 = 8;

/// The four `enable_*` transport switches as µTorrent's `bt.transp_disposition` bits.
pub fn transp_disposition(s: &Settings) -> u32 {
    [
        (s.enable_outgoing_tcp, TRANSP_OUT_TCP),
        (s.enable_outgoing_utp, TRANSP_OUT_UTP),
        (s.enable_incoming_tcp, TRANSP_IN_TCP),
        (s.enable_incoming_utp, TRANSP_IN_UTP),
    ]
    .into_iter()
    .filter(|&(on, _)| on)
    .fold(0, |bits, (_, bit)| bits | bit)
}

pub fn set_transp_disposition(s: &mut Settings, bits: u32) {
    s.enable_outgoing_tcp = bits & TRANSP_OUT_TCP != 0;
    s.enable_outgoing_utp = bits & TRANSP_OUT_UTP != 0;
    s.enable_incoming_tcp = bits & TRANSP_IN_TCP != 0;
    s.enable_incoming_utp = bits & TRANSP_IN_UTP != 0;
}

/// Every [`Settings`] field (libtorrent names) and then µTorrent's composed settings.
pub static TABLE: &[Entry] = &[
    entry!(user_agent: str),
    entry!(peer_fingerprint: str),
    entry!(listen_interfaces: str),
    entry!(dht_bootstrap_nodes: str),
    entry!(download_rate_limit: u32),
    entry!(upload_rate_limit: u32),
    entry!(active_downloads: i32),
    entry!(active_seeds: i32),
    entry!(active_checking: i32),
    entry!(active_dht_limit: i32),
    entry!(active_tracker_limit: i32),
    entry!(active_lsd_limit: i32),
    entry!(active_limit: i32),
    entry!(dont_count_slow_torrents: bool),
    entry!(auto_manage_interval: u32),
    entry!(connections_limit: i32),
    entry!(connection_speed: i32),
    entry!(max_retry_port_bind: u32),
    entry!(unchoke_slots_limit: i32),
    entry!(max_peerlist_size: i32),
    entry!(max_paused_peerlist_size: i32),
    entry!(enable_dht: bool),
    entry!(dht_announce_interval: u32),
    entry!(dht_upload_rate_limit: u32),
    entry!(dht_read_only: bool),
    entry!(dht_privacy_lookups: bool),
    entry!(dht_enforce_node_id: bool),
    entry!(dht_restrict_routing_ips: bool),
    entry!(dht_restrict_search_ips: bool),
    entry!(dht_prefer_verified_node_ids: bool),
    entry!(dht_extended_routing_table: bool),
    entry!(dht_aggressive_lookups: bool),
    entry!(dht_ignore_dark_internet: bool),
    entry!(dht_max_peers_reply: i32),
    entry!(dht_search_branching: i32),
    entry!(dht_max_fail_count: i32),
    entry!(dht_max_torrents: i32),
    entry!(dht_max_peers: i32),
    entry!(dht_block_timeout: i32),
    entry!(dht_block_ratelimit: i32),
    entry!(enable_lsd: bool),
    entry!(local_service_announce_interval: u32),
    entry!(enable_upnp: bool),
    entry!(enable_natpmp: bool),
    entry!(upnp_ignore_nonrouters: bool),
    entry!(upnp_lease_duration: u32),
    entry!(enable_incoming_tcp: bool),
    entry!(enable_outgoing_tcp: bool),
    entry!(enable_incoming_utp: bool),
    entry!(enable_outgoing_utp: bool),
    entry!(utp_target_delay: i32),
    entry!(utp_gain_factor: i32),
    entry!(utp_min_timeout: i32),
    entry!(utp_syn_resends: i32),
    entry!(utp_fin_resends: i32),
    entry!(utp_num_resends: i32),
    entry!(utp_connect_timeout: i32),
    entry!(utp_loss_multiplier: i32),
    entry!(utp_cwnd_reduce_timer: i32),
    Entry {
        name: "out_enc_policy",
        kind: Kind::Int { min: 0, max: 2 },
        get: |s| Value::Int(policy_to_int(s.out_enc_policy)),
        set: |s, v| s.out_enc_policy = policy_from_int(int(v)),
    },
    Entry {
        name: "in_enc_policy",
        kind: Kind::Int { min: 0, max: 2 },
        get: |s| Value::Int(policy_to_int(s.in_enc_policy)),
        set: |s, v| s.in_enc_policy = policy_from_int(int(v)),
    },
    Entry {
        name: "allowed_enc_level",
        kind: Kind::Int { min: 1, max: 3 },
        get: |s| Value::Int(level_to_int(s.allowed_enc_level)),
        set: |s, v| s.allowed_enc_level = level_from_int(int(v)),
    },
    entry!(prefer_rc4: bool),
    entry!(stop_tracker_timeout: u32),
    entry!(tracker_completion_timeout: u32),
    entry!(tracker_receive_timeout: u32),
    entry!(min_announce_interval: u32),
    entry!(num_want: u32),
    entry!(announce_to_all_tiers: bool),
    entry!(announce_to_all_trackers: bool),
    entry!(apply_ip_filter_to_trackers: bool),
    entry!(request_timeout: u32),
    entry!(use_parole_mode: bool),
    entry!(peer_timeout: u32),
    entry!(peer_connect_timeout: u32),
    entry!(handshake_timeout: u32),
    entry!(request_queue_time: u32),
    entry!(max_out_request_queue: i32),
    entry!(max_allowed_in_request_queue: i32),
    entry!(unchoke_interval: u32),
    entry!(optimistic_unchoke_interval: u32),
    entry!(send_buffer_watermark: u32),
    entry!(close_redundant_connections: bool),
    entry!(max_metadata_size: i32),
    entry!(max_pex_peers: i32),
    Entry {
        name: "alert_queue_size",
        kind: Kind::Int { min: 1, max: u32::MAX as i64 },
        get: |s| Value::Int(s.alert_queue_size as i64),
        set: |s, v| s.alert_queue_size = int(v) as usize,
    },
    entry!(tick_interval: u32),
    entry!(aio_threads: u32),
    entry!(hashing_threads: u32),
    entry!(file_pool_size: u32),
    entry!(max_queued_disk_bytes: u32),
    entry!(urlseed_pipeline_size: i32),
    entry!(urlseed_timeout: u32),
    entry!(urlseed_wait_retry: u32),
    entry!(urlseed_max_request_bytes: i32),
    entry!(web_seed_name_lookup_retry: u32),
    entry!(max_web_seed_connections: i32),
    entry!(ban_web_seeds: bool),
    entry!(ssrf_mitigation: bool),
    entry!(allow_idna: bool),
    entry!(always_send_user_agent: bool),
    // µTorrent names for settings made of several libtorrent ones; not persisted (their parts are)
    Entry {
        name: "bt.transp_disposition",
        kind: Kind::Int { min: 0, max: 15 },
        get: |s| Value::Int(transp_disposition(s) as i64),
        set: |s, v| set_transp_disposition(s, int(v) as u32),
    },
];

/// Prefix of the composed µTorrent rows, which are views of other rows.
const COMPOSED: &str = "bt.";

pub fn find(name: &str) -> Option<&'static Entry> {
    TABLE.iter().find(|e| e.name == name)
}

pub fn get(s: &Settings, name: &str) -> Option<Value> {
    find(name).map(|e| e.get(s))
}

pub fn set(s: &mut Settings, name: &str, text: &str) -> Result<(), String> {
    find(name).ok_or_else(|| format!("unknown setting \"{name}\""))?.set(s, text)
}

/// Prefix of the engine settings in the preferences file.
pub const PREF_PREFIX: &str = "lt.";

/// The engine settings from the preferences: defaults, then the older `net.port` key, then every
/// `lt.<name>` line. Unknown or malformed lines are skipped (and kept in the file).
pub fn load(prefs: &Prefs) -> Settings {
    let mut s = Settings::default();
    if let Some(port) = prefs.get_i32("net.port").filter(|p| (1..65536).contains(p)) {
        set_listen_port(&mut s, port as u16);
    }
    for e in TABLE.iter().filter(|e| !e.name.starts_with(COMPOSED)) {
        if let Some(v) = prefs.get(&format!("{PREF_PREFIX}{}", e.name)) {
            let _ = e.set(&mut s, v);
        }
    }
    s
}

/// Writes the settings that differ from the defaults and removes the rest (and `net.port`,
/// which `lt.listen_interfaces` replaces).
pub fn store(prefs: &mut Prefs, s: &Settings) {
    prefs.remove("net.port");
    for e in TABLE.iter().filter(|e| !e.name.starts_with(COMPOSED)) {
        let key = format!("{PREF_PREFIX}{}", e.name);
        if e.is_default(s) {
            prefs.remove(&key);
        } else {
            prefs.set(&key, e.get(s));
        }
    }
}

/// The port of the first `listen_interfaces` entry (µTorrent's single "port used for incoming
/// connections"), if it has one.
pub fn listen_port(s: &Settings) -> Option<u16> {
    let first = s.listen_interfaces.split(',').map(str::trim).find(|e| !e.is_empty())?;
    let (_, port) = first.rsplit_once(':')?;
    port.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse().ok()
}

/// Sets the port of every `listen_interfaces` entry (keeping the addresses, devices and flags);
/// an empty list becomes µTorrent's "every interface".
pub fn set_listen_port(s: &mut Settings, port: u16) {
    let entries: Vec<String> = s
        .listen_interfaces
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| match e.rsplit_once(':') {
            // `[::1]` alone has colons but no port
            Some((addr, rest)) if !addr.is_empty() && (!addr.starts_with('[') || addr.ends_with(']')) => {
                let flags: String = rest.chars().skip_while(char::is_ascii_digit).collect();
                format!("{addr}:{port}{flags}")
            }
            _ => format!("{e}:{port}"),
        })
        .collect();
    s.listen_interfaces =
        if entries.is_empty() { format!("0.0.0.0:{port},[::]:{port}") } else { entries.join(",") };
}

/// µTorrent's encryption settings: "Outgoing" is `out_enc_policy`; "Allow incoming legacy
/// connections" checked is `in_enc_policy = Enabled`, unchecked `Forced` (µTorrent never refuses
/// encrypted incoming connections, so `in_enc_policy = Disabled` reads as allowed).
pub fn allow_legacy_incoming(s: &Settings) -> bool {
    s.in_enc_policy != EncPolicy::Forced
}

/// Sets µTorrent's two encryption controls. `Forced` outgoing pairs with RC4 only, as
/// qBittorrent and Transmission do; anything else allows both levels.
pub fn set_encryption(s: &mut Settings, outgoing: EncPolicy, allow_legacy: bool) {
    s.out_enc_policy = outgoing;
    s.in_enc_policy = if allow_legacy { EncPolicy::Enabled } else { EncPolicy::Forced };
    s.allowed_enc_level = if outgoing == EncPolicy::Forced { EncLevel::Rc4 } else { EncLevel::Both };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Field names of `Settings` read from its `Debug` output, so a field added to the struct
    /// without a row here fails this test.
    fn field_names() -> Vec<String> {
        format!("{:#?}", Settings::default())
            .lines()
            .filter(|l| l.starts_with("    ") && !l.starts_with("     "))
            .filter_map(|l| l.trim().split_once(':').map(|(n, _)| n.to_owned()))
            .collect()
    }

    #[test]
    fn every_field_has_a_row() {
        let fields = field_names();
        assert!(fields.len() > 100, "{fields:?}");
        for f in &fields {
            assert!(find(f).is_some(), "Settings::{f} has no row in the settings table");
        }
        let rows: Vec<&str> = TABLE.iter().map(|e| e.name).filter(|n| !n.starts_with(COMPOSED)).collect();
        assert_eq!(rows.len(), fields.len());
        let mut names: Vec<&str> = TABLE.iter().map(|e| e.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), TABLE.len(), "duplicate rows");
    }

    #[test]
    fn every_row_round_trips() {
        let d = Settings::default();
        for e in TABLE {
            assert!(e.is_default(&d), "{}", e.name);
            let mut s = d.clone();
            let text = e.get(&d).to_string();
            e.set(&mut s, &text).unwrap();
            assert_eq!(s, d, "{} = {text}", e.name);
            // a different value sticks and reads back
            let other = match e.kind {
                Kind::Bool => (!matches!(e.get(&d), Value::Bool(true))).to_string(),
                Kind::Int { min, max } => {
                    let Value::Int(i) = e.get(&d) else { panic!() };
                    (if i < max { i + 1 } else { min }).to_string()
                }
                Kind::Str => "x".to_owned(),
            };
            e.set(&mut s, &other).unwrap();
            assert_eq!(e.get(&s).to_string(), other, "{}", e.name);
            assert!(!e.is_default(&s), "{}", e.name);
        }
    }

    #[test]
    fn parse_errors_and_names() {
        let mut s = Settings::default();
        set(&mut s, "enable_dht", "false").unwrap();
        assert!(!s.enable_dht);
        set(&mut s, "enable_dht", " On ").unwrap();
        assert!(s.enable_dht);
        assert!(set(&mut s, "enable_dht", "maybe").is_err());
        set(&mut s, "connections_limit", "42").unwrap();
        assert_eq!(s.connections_limit, 42);
        assert!(set(&mut s, "connections_limit", "x").is_err());
        assert!(set(&mut s, "upload_rate_limit", "-1").is_err());
        assert!(set(&mut s, "out_enc_policy", "3").is_err());
        set(&mut s, "out_enc_policy", "0").unwrap();
        assert_eq!(s.out_enc_policy, EncPolicy::Forced);
        set(&mut s, "allowed_enc_level", "2").unwrap();
        assert_eq!(s.allowed_enc_level, EncLevel::Rc4);
        assert!(set(&mut s, "no_such_setting", "1").is_err());
        assert_eq!(get(&s, "connections_limit"), Some(Value::Int(42)));
        assert_eq!(get(&s, "user_agent").unwrap().to_string(), patorrent_core::USER_AGENT);
    }

    #[test]
    fn transport_bits() {
        let mut s = Settings::default();
        assert_eq!(transp_disposition(&s), 15);
        set(&mut s, "bt.transp_disposition", "5").unwrap();
        assert!(s.enable_outgoing_tcp && s.enable_incoming_tcp);
        assert!(!s.enable_outgoing_utp && !s.enable_incoming_utp);
        assert_eq!(get(&s, "bt.transp_disposition"), Some(Value::Int(5)));
        set_transp_disposition(&mut s, TRANSP_OUT_UTP | TRANSP_IN_UTP);
        assert!(!s.enable_outgoing_tcp && s.enable_outgoing_utp && s.enable_incoming_utp);
        assert!(set(&mut s, "bt.transp_disposition", "16").is_err());
    }

    #[test]
    fn listen_ports() {
        let mut s = Settings::default();
        assert_eq!(listen_port(&s), Some(6881));
        set_listen_port(&mut s, 12345);
        assert_eq!(s.listen_interfaces, "0.0.0.0:12345,[::]:12345");
        assert_eq!(listen_port(&s), Some(12345));
        s.listen_interfaces = "eth0:6881l, [::1]:7000s,10.0.0.1".to_owned();
        assert_eq!(listen_port(&s), Some(6881));
        set_listen_port(&mut s, 4000);
        assert_eq!(s.listen_interfaces, "eth0:4000l,[::1]:4000s,10.0.0.1:4000");
        s.listen_interfaces = "[::1]".to_owned();
        assert_eq!(listen_port(&s), None);
        set_listen_port(&mut s, 5);
        assert_eq!(s.listen_interfaces, "[::1]:5");
        s.listen_interfaces.clear();
        set_listen_port(&mut s, 6);
        assert_eq!(s.listen_interfaces, "0.0.0.0:6,[::]:6");
    }

    #[test]
    fn encryption_controls() {
        let mut s = Settings::default();
        assert!(allow_legacy_incoming(&s));
        set_encryption(&mut s, EncPolicy::Forced, false);
        assert_eq!(
            (s.out_enc_policy, s.in_enc_policy, s.allowed_enc_level),
            (EncPolicy::Forced, EncPolicy::Forced, EncLevel::Rc4)
        );
        assert!(!allow_legacy_incoming(&s));
        set_encryption(&mut s, EncPolicy::Disabled, true);
        assert_eq!(
            (s.out_enc_policy, s.in_enc_policy, s.allowed_enc_level),
            (EncPolicy::Disabled, EncPolicy::Enabled, EncLevel::Both)
        );
        s.in_enc_policy = EncPolicy::Disabled;
        assert!(allow_legacy_incoming(&s));
    }

    #[test]
    fn preferences_round_trip() {
        let dir = std::env::temp_dir().join(format!("patorrent-settings-ini-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("patorrent.ini");
        let mut prefs = Prefs::load(&path);
        prefs.set("net.port", 7777);
        prefs.set("ui.toolbar", "0");
        let mut s = load(&prefs);
        assert_eq!(listen_port(&s), Some(7777));
        s.enable_dht = false;
        s.upload_rate_limit = 50 * 1024;
        s.user_agent = "with = sign".to_owned();
        set_encryption(&mut s, EncPolicy::Forced, false);
        store(&mut prefs, &s);
        assert_eq!(prefs.get("net.port"), None);
        assert_eq!(prefs.get("lt.enable_dht"), Some("false"));
        assert_eq!(prefs.get("lt.download_rate_limit"), None, "defaults are not written");
        assert_eq!(prefs.get("lt.bt.transp_disposition"), None);
        prefs.save().unwrap();

        let mut again = Prefs::load(&path);
        assert_eq!(load(&again), s);
        assert_eq!(again.get("ui.toolbar"), Some("0"), "other keys are kept");
        // back to the defaults: the lines go away
        store(&mut again, &Settings::default());
        assert!(!again.to_text().contains("lt."), "{}", again.to_text());
        // malformed lines are skipped
        again.set("lt.connections_limit", "lots");
        again.set("lt.bogus", "1");
        assert_eq!(load(&again), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
