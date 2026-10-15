//! Session settings: the subset of libtorrent's `settings_pack` the API needs so far.
//!
//! Names and defaults follow libtorrent 2.0.11 `settings_pack.cpp`, except identity strings.
//! Phase 2 ports the full name table (string-keyed get/set, which also backs the µTorrent-style
//! "Advanced" preferences page) and `docs/SETTINGS.md` records every deliberate change.

/// Protocol-encryption policy (libtorrent `settings_pack::enc_policy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncPolicy {
    /// Only encrypted connections are accepted / made.
    Forced,
    /// Encrypted and plaintext connections are allowed.
    Enabled,
    /// Only plaintext connections.
    Disabled,
}

/// Which MSE/PE encryption levels are allowed (libtorrent `settings_pack::enc_level`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncLevel {
    Plaintext,
    Rc4,
    Both,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub user_agent: String,
    pub peer_fingerprint: String,
    /// Comma-separated `ip:port` list; `[...]` for IPv6.
    pub listen_interfaces: String,
    pub dht_bootstrap_nodes: String,

    /// Bytes per second; 0 = unlimited.
    pub download_rate_limit: u32,
    pub upload_rate_limit: u32,

    /// Queueing limits; -1 = unlimited.
    pub active_downloads: i32,
    pub active_seeds: i32,
    pub active_checking: i32,
    pub active_dht_limit: i32,
    pub active_tracker_limit: i32,
    pub active_lsd_limit: i32,
    pub active_limit: i32,
    pub dont_count_slow_torrents: bool,
    pub auto_manage_interval: u32,

    pub connections_limit: i32,
    pub unchoke_slots_limit: i32,
    pub max_peerlist_size: i32,
    pub max_paused_peerlist_size: i32,

    pub enable_dht: bool,
    pub enable_lsd: bool,
    pub enable_upnp: bool,
    pub enable_natpmp: bool,
    pub enable_incoming_tcp: bool,
    pub enable_outgoing_tcp: bool,
    pub enable_incoming_utp: bool,
    pub enable_outgoing_utp: bool,

    pub out_enc_policy: EncPolicy,
    pub in_enc_policy: EncPolicy,
    pub allowed_enc_level: EncLevel,
    pub prefer_rc4: bool,

    /// Seconds.
    pub stop_tracker_timeout: u32,
    pub request_timeout: u32,
    pub peer_timeout: u32,

    pub alert_queue_size: usize,
    /// Milliseconds between session ticks.
    pub tick_interval: u32,
    pub aio_threads: u32,
    pub hashing_threads: u32,
    pub file_pool_size: u32,
    pub max_queued_disk_bytes: u32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            user_agent: crate::USER_AGENT.to_owned(),
            peer_fingerprint: crate::PEER_ID_PREFIX.to_owned(),
            listen_interfaces: "0.0.0.0:6881,[::]:6881".to_owned(),
            dht_bootstrap_nodes: "dht.libtorrent.org:25401".to_owned(),
            download_rate_limit: 0,
            upload_rate_limit: 0,
            active_downloads: 3,
            active_seeds: 5,
            active_checking: 1,
            active_dht_limit: 88,
            active_tracker_limit: 1600,
            active_lsd_limit: 60,
            active_limit: 500,
            dont_count_slow_torrents: true,
            auto_manage_interval: 30,
            connections_limit: 200,
            unchoke_slots_limit: 8,
            max_peerlist_size: 3000,
            max_paused_peerlist_size: 1000,
            enable_dht: true,
            enable_lsd: true,
            enable_upnp: true,
            enable_natpmp: true,
            enable_incoming_tcp: true,
            enable_outgoing_tcp: true,
            enable_incoming_utp: true,
            enable_outgoing_utp: true,
            out_enc_policy: EncPolicy::Enabled,
            in_enc_policy: EncPolicy::Enabled,
            allowed_enc_level: EncLevel::Both,
            prefer_rc4: false,
            stop_tracker_timeout: 5,
            request_timeout: 60,
            peer_timeout: 120,
            alert_queue_size: 2000,
            tick_interval: 500,
            aio_threads: 10,
            hashing_threads: 1,
            file_pool_size: 40,
            max_queued_disk_bytes: 1024 * 1024,
        }
    }
}
