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
    /// New outgoing connection attempts per second.
    pub connection_speed: i32,
    /// Ports tried after the configured one when it is busy.
    pub max_retry_port_bind: u32,
    pub unchoke_slots_limit: i32,
    pub max_peerlist_size: i32,
    pub max_paused_peerlist_size: i32,

    pub enable_dht: bool,
    /// Seconds between DHT announces of each running torrent.
    pub dht_announce_interval: u32,
    /// Bytes per second the DHT answers queries with.
    pub dht_upload_rate_limit: u32,
    /// BEP 43: take part in lookups without answering queries.
    pub dht_read_only: bool,
    /// Obfuscate the info-hash in DHT lookups.
    pub dht_privacy_lookups: bool,
    /// Ignore DHT nodes whose id doesn't match their IP (BEP 42).
    pub dht_enforce_node_id: bool,
    pub dht_restrict_routing_ips: bool,
    pub dht_restrict_search_ips: bool,
    pub dht_prefer_verified_node_ids: bool,
    pub dht_extended_routing_table: bool,
    pub dht_aggressive_lookups: bool,
    pub dht_ignore_dark_internet: bool,
    pub dht_max_peers_reply: i32,
    pub dht_search_branching: i32,
    pub dht_max_fail_count: i32,
    pub dht_max_torrents: i32,
    pub dht_max_peers: i32,
    pub dht_block_timeout: i32,
    pub dht_block_ratelimit: i32,
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
    pub tracker_completion_timeout: u32,
    pub tracker_receive_timeout: u32,
    /// Smallest re-announce interval we honour, in seconds.
    pub min_announce_interval: u32,
    pub num_want: u32,
    /// Announce to one tracker in every tier (true), or only to the first working one.
    pub announce_to_all_tiers: bool,
    /// Announce to every tracker in a tier, not just the first working one.
    pub announce_to_all_trackers: bool,
    pub request_timeout: u32,
    pub peer_timeout: u32,
    pub peer_connect_timeout: u32,
    pub handshake_timeout: u32,
    /// Seconds of requests to keep queued per peer, at its current rate.
    pub request_queue_time: u32,
    pub max_out_request_queue: i32,
    pub max_allowed_in_request_queue: i32,
    /// Seconds between choker runs / optimistic unchoke rotations.
    pub unchoke_interval: u32,
    pub optimistic_unchoke_interval: u32,
    /// Bytes queued in a peer's send buffer before more disk reads are issued for it.
    pub send_buffer_watermark: u32,
    /// Disconnect peers when both sides are seeds (or neither side wants anything).
    pub close_redundant_connections: bool,
    /// Largest info dictionary accepted from peers (ut_metadata), in bytes.
    pub max_metadata_size: i32,
    /// Peers remembered per connection (and address family) from its ut_pex messages.
    pub max_pex_peers: i32,

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
            connection_speed: 30,
            max_retry_port_bind: 10,
            unchoke_slots_limit: 8,
            max_peerlist_size: 3000,
            max_paused_peerlist_size: 1000,
            enable_dht: true,
            dht_announce_interval: 15 * 60,
            dht_upload_rate_limit: 8000,
            dht_read_only: false,
            dht_privacy_lookups: false,
            dht_enforce_node_id: false,
            dht_restrict_routing_ips: true,
            dht_restrict_search_ips: true,
            dht_prefer_verified_node_ids: true,
            dht_extended_routing_table: true,
            dht_aggressive_lookups: true,
            dht_ignore_dark_internet: true,
            dht_max_peers_reply: 100,
            dht_search_branching: 5,
            dht_max_fail_count: 20,
            dht_max_torrents: 2000,
            dht_max_peers: 500,
            dht_block_timeout: 5 * 60,
            dht_block_ratelimit: 5,
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
            tracker_completion_timeout: 30,
            tracker_receive_timeout: 10,
            min_announce_interval: 300,
            num_want: 200,
            // libtorrent defaults to false for both; qBittorrent and µTorrent announce to every
            // tier, which finds more peers on multi-tier public torrents.
            announce_to_all_tiers: true,
            announce_to_all_trackers: false,
            request_timeout: 60,
            peer_timeout: 120,
            peer_connect_timeout: 15,
            handshake_timeout: 10,
            request_queue_time: 3,
            max_out_request_queue: 500,
            max_allowed_in_request_queue: 500,
            unchoke_interval: 15,
            optimistic_unchoke_interval: 30,
            send_buffer_watermark: 500 * 1024,
            close_redundant_connections: true,
            max_metadata_size: 3 * 1024 * 10240,
            max_pex_peers: 50,
            alert_queue_size: 2000,
            tick_interval: 500,
            aio_threads: 10,
            hashing_threads: 1,
            file_pool_size: 40,
            max_queued_disk_bytes: 1024 * 1024,
        }
    }
}
