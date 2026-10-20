//! Session settings: the subset of libtorrent's `settings_pack` the API needs so far.
//!
//! Names and defaults follow libtorrent 2.0.11 `settings_pack.cpp`, except identity strings.
//! The string-keyed table (get/set by libtorrent name, behind the µTorrent-style "Advanced"
//! preferences page) is `patorrent_frontend::settings`: a new field needs a row there too.
//! `docs/SETTINGS.md` records every deliberate change.

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

/// The kind of proxy server (libtorrent `settings_pack::proxy_type_t`; I2P is not supported).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProxyType {
    /// No proxy: everything connects directly.
    #[default]
    None,
    /// A SOCKS4 server; the user name is sent as the SOCKS4 user id. Host names go to it as
    /// SOCKS4a when `proxy_hostnames` is set.
    Socks4,
    /// A SOCKS5 server without authentication (RFC 1928).
    Socks5,
    /// A SOCKS5 server with username / password authentication (RFC 1929).
    Socks5Pw,
    /// An HTTP proxy: plain `http://` tracker and web seed requests are sent to it as proxy
    /// requests, everything else through `CONNECT`.
    Http,
    /// An HTTP proxy with basic authentication.
    HttpPw,
}

impl ProxyType {
    pub fn is_socks5(self) -> bool {
        matches!(self, ProxyType::Socks5 | ProxyType::Socks5Pw)
    }

    pub fn is_http(self) -> bool {
        matches!(self, ProxyType::Http | ProxyType::HttpPw)
    }
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
    /// Seconds in which every running torrent is announced once on the local network (BEP 14).
    pub local_service_announce_interval: u32,
    pub enable_upnp: bool,
    pub enable_natpmp: bool,
    /// Only take UPnP replies from the listen address's own subnet (libtorrent default: off).
    pub upnp_ignore_nonrouters: bool,
    /// Seconds a UPnP mapping is leased for; 0 asks for a permanent one.
    pub upnp_lease_duration: u32,
    pub enable_incoming_tcp: bool,
    pub enable_outgoing_tcp: bool,
    pub enable_incoming_utp: bool,
    pub enable_outgoing_utp: bool,
    /// uTP's target one-way delay, in milliseconds (LEDBAT).
    pub utp_target_delay: i32,
    /// Bytes the uTP congestion window grows per RTT when the delay is zero.
    pub utp_gain_factor: i32,
    /// Shortest uTP socket timeout, in milliseconds.
    pub utp_min_timeout: i32,
    /// SYN packets sent (and timed out) before an outgoing uTP connect is given up.
    pub utp_syn_resends: i32,
    pub utp_fin_resends: i32,
    /// Times a packet is sent (and timed out) before the connection is dropped.
    pub utp_num_resends: i32,
    /// Timeout of the first SYN, in milliseconds; doubles per timed-out packet in a row.
    pub utp_connect_timeout: i32,
    /// Percent of the congestion window kept when a packet is lost.
    pub utp_loss_multiplier: i32,
    /// Milliseconds between window cuts on loss.
    pub utp_cwnd_reduce_timer: i32,

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
    /// The IP filter also keeps us from contacting trackers at blocked addresses (for
    /// torrents it applies to).
    pub apply_ip_filter_to_trackers: bool,
    pub request_timeout: u32,
    /// A peer that sent part of a piece that failed its hash check may only download whole
    /// pieces by itself until one of them passes, so the next failure is clearly its own
    /// (libtorrent `use_parole_mode`).
    pub use_parole_mode: bool,
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
    /// Peer turnover (libtorrent `peer_turnover`, `peer_turnover_cutoff`,
    /// `peer_turnover_interval`): every `peer_turnover_interval` seconds, once the session has
    /// `peer_turnover_cutoff` percent of `connections_limit` peers, the torrent with the most
    /// peers drops `peer_turnover` percent of them, the least useful first (at least one, at
    /// most as many as it has other peers to connect to). Below the session's cutoff, every
    /// torrent at the cutoff of its own connection limit does the same.
    pub peer_turnover: u32,
    pub peer_turnover_cutoff: u32,
    pub peer_turnover_interval: u32,
    /// Socket buffer sizes in bytes (`SO_RCVBUF` / `SO_SNDBUF`) of peer connections and of the
    /// listen and UDP tracker sockets; 0 keeps the OS default.
    pub recv_socket_buffer_size: i32,
    pub send_socket_buffer_size: i32,
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
    /// The disk threads' read cache (libtorrent 1.2 names; 2.0 has no read cache): blocks peers
    /// ask for are read `read_cache_line_size` blocks at a time, capped by the piece, and the
    /// lines kept in up to `cache_size` blocks of 16 KiB (shared by the `aio_threads`; -1 is
    /// the default, 2048 = 32 MiB). A line unused for `cache_expiry` seconds is dropped. Off
    /// when `use_read_cache` is false or `read_cache_line_size` is 0.
    pub use_read_cache: bool,
    pub cache_size: i32,
    pub read_cache_line_size: i32,
    pub cache_expiry: u32,
    /// Web seeds (libtorrent's `urlseed_*` and web seed settings): BEP 17 requests in flight
    /// per connection (we also cap BEP 19 request runs by it).
    pub urlseed_pipeline_size: i32,
    /// Seconds without data before a web seed connection is closed.
    pub urlseed_timeout: u32,
    /// Seconds before a failed web seed is tried again.
    pub urlseed_wait_retry: u32,
    /// Largest HTTP range request to a web seed.
    pub urlseed_max_request_bytes: i32,
    /// Seconds before a web seed whose host name did not resolve is tried again.
    pub web_seed_name_lookup_retry: u32,
    /// Web seeds connected at once per torrent (0: 100).
    pub max_web_seed_connections: i32,
    /// A web seed that sends corrupt data may be banned, as a peer would be.
    pub ban_web_seeds: bool,
    /// Web seeds on the local network may not use query strings, and redirects from the
    /// internet may not lead to the local network (libtorrent `ssrf_mitigation`).
    pub ssrf_mitigation: bool,
    /// Allow international domain names (`xn--`) in web seed URLs.
    pub allow_idna: bool,
    /// Send the User-Agent on every web seed request, not just the first of a connection.
    pub always_send_user_agent: bool,

    /// The proxy server (libtorrent `proxy_type`, `proxy_hostname`, `proxy_port`,
    /// `proxy_username`, `proxy_password`).
    pub proxy_type: ProxyType,
    pub proxy_hostname: String,
    pub proxy_port: u16,
    pub proxy_username: String,
    pub proxy_password: String,
    /// Host names (trackers, web seeds) are resolved by the proxy instead of locally: SOCKS5
    /// sends the name, SOCKS4 uses SOCKS4a. Plain HTTP proxy requests always carry the name.
    pub proxy_hostnames: bool,
    /// Peer and web seed connections go through the proxy. Incoming connections are then
    /// refused, no TCP listen socket is opened, and LSD and port mapping are off (libtorrent's
    /// "proxy" listen socket); uTP and the DHT need a SOCKS5 proxy's UDP associate.
    pub proxy_peer_connections: bool,
    /// Tracker connections (HTTP and UDP) go through the proxy.
    pub proxy_tracker_connections: bool,
    /// HTTP CONNECT names the host (`CONNECT tracker.example:443`, with a `Host` header) rather
    /// than the address it resolved to locally. Not in libtorrent 2.0.11.
    pub proxy_send_host_in_connect: bool,
    /// The SOCKS5 UDP ASSOCIATE command names our UDP socket's address and port, so the proxy
    /// can forward datagrams to us before we send any (wrong if there is a NAT in between).
    pub socks5_udp_send_local_ep: bool,
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
            local_service_announce_interval: 5 * 60,
            enable_upnp: true,
            enable_natpmp: true,
            upnp_ignore_nonrouters: false,
            upnp_lease_duration: 3600,
            enable_incoming_tcp: true,
            enable_outgoing_tcp: true,
            enable_incoming_utp: true,
            enable_outgoing_utp: true,
            utp_target_delay: 100,
            utp_gain_factor: 3000,
            utp_min_timeout: 500,
            utp_syn_resends: 2,
            utp_fin_resends: 2,
            utp_num_resends: 3,
            utp_connect_timeout: 3000,
            utp_loss_multiplier: 50,
            utp_cwnd_reduce_timer: 100,
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
            apply_ip_filter_to_trackers: true,
            request_timeout: 60,
            use_parole_mode: true,
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
            peer_turnover: 4,
            peer_turnover_cutoff: 90,
            peer_turnover_interval: 300,
            recv_socket_buffer_size: 0,
            send_socket_buffer_size: 0,
            max_metadata_size: 3 * 1024 * 10240,
            max_pex_peers: 50,
            alert_queue_size: 2000,
            tick_interval: 500,
            aio_threads: 10,
            hashing_threads: 1,
            file_pool_size: 40,
            max_queued_disk_bytes: 1024 * 1024,
            use_read_cache: true,
            cache_size: 2048,
            read_cache_line_size: 32,
            cache_expiry: 300,
            urlseed_pipeline_size: 5,
            urlseed_timeout: 20,
            urlseed_wait_retry: 30,
            urlseed_max_request_bytes: 16 * 1024 * 1024,
            web_seed_name_lookup_retry: 1800,
            max_web_seed_connections: 3,
            ban_web_seeds: true,
            ssrf_mitigation: true,
            allow_idna: false,
            always_send_user_agent: false,
            proxy_type: ProxyType::None,
            proxy_hostname: String::new(),
            proxy_port: 0,
            proxy_username: String::new(),
            proxy_password: String::new(),
            proxy_hostnames: true,
            proxy_peer_connections: true,
            proxy_tracker_connections: true,
            proxy_send_host_in_connect: false,
            socks5_udp_send_local_ep: false,
        }
    }
}
