// Ported from libtorrent-rasterbar 2.0.11: src/settings_pack.cpp (the dht_* settings and defaults)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a plain struct copied out of `Settings` instead of string-keyed lookups.

/// The DHT's settings (libtorrent `settings_pack::dht_*`), with libtorrent's defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct DhtSettings {
    /// Most peers returned in one `get_peers` reply.
    pub max_peers_reply: i32,
    /// Branch factor (alpha) of lookups.
    pub search_branching: i32,
    /// Failures in a row before a node leaves the routing table.
    pub max_fail_count: i32,
    pub max_torrents: i32,
    pub max_dht_items: i32,
    /// Peers stored per torrent.
    pub max_peers: i32,
    pub max_torrent_search_reply: i32,
    /// Bigger buckets near the top of the routing table.
    pub extended_routing_table: bool,
    /// Keep `search_branching` requests outstanding at the top of the result list.
    pub aggressive_lookups: bool,
    /// Obfuscate the target of `get_peers` lookups.
    pub privacy_lookups: bool,
    /// Ignore nodes whose id doesn't match their IP (BEP 42).
    pub enforce_node_id: bool,
    /// Drop packets from unrouted class A networks.
    pub ignore_dark_internet: bool,
    /// BEP 43: don't answer queries, mark our queries `ro`.
    pub read_only: bool,
    /// One routing table entry per IP, and no two entries in one /24 (/64) per bucket.
    pub restrict_routing_ips: bool,
    /// No two results of a lookup from one /24 (/64).
    pub restrict_search_ips: bool,
    /// Only BEP 42 nodes may split buckets.
    pub prefer_verified_node_ids: bool,
    /// Seconds a flooding IP stays blocked.
    pub block_timeout: i32,
    /// Messages per second from one IP before it is blocked.
    pub block_ratelimit: i32,
    /// Bytes per second we answer queries with.
    pub upload_rate_limit: i32,
    /// Seconds between DHT announces of every running torrent.
    pub announce_interval: i32,
}

impl Default for DhtSettings {
    fn default() -> DhtSettings {
        DhtSettings {
            max_peers_reply: 100,
            search_branching: 5,
            max_fail_count: 20,
            max_torrents: 2000,
            max_dht_items: 700,
            max_peers: 500,
            max_torrent_search_reply: 20,
            extended_routing_table: true,
            aggressive_lookups: true,
            privacy_lookups: false,
            enforce_node_id: false,
            ignore_dark_internet: true,
            read_only: false,
            restrict_routing_ips: true,
            restrict_search_ips: true,
            prefer_verified_node_ids: true,
            block_timeout: 5 * 60,
            block_ratelimit: 5,
            upload_rate_limit: 8000,
            announce_interval: 15 * 60,
        }
    }
}

impl From<&crate::settings::Settings> for DhtSettings {
    fn from(s: &crate::settings::Settings) -> DhtSettings {
        DhtSettings {
            max_peers_reply: s.dht_max_peers_reply,
            search_branching: s.dht_search_branching.max(1),
            max_fail_count: s.dht_max_fail_count,
            max_torrents: s.dht_max_torrents,
            max_peers: s.dht_max_peers,
            extended_routing_table: s.dht_extended_routing_table,
            aggressive_lookups: s.dht_aggressive_lookups,
            privacy_lookups: s.dht_privacy_lookups,
            enforce_node_id: s.dht_enforce_node_id,
            ignore_dark_internet: s.dht_ignore_dark_internet,
            read_only: s.dht_read_only,
            restrict_routing_ips: s.dht_restrict_routing_ips,
            restrict_search_ips: s.dht_restrict_search_ips,
            prefer_verified_node_ids: s.dht_prefer_verified_node_ids,
            block_timeout: s.dht_block_timeout,
            block_ratelimit: s.dht_block_ratelimit,
            upload_rate_limit: s.dht_upload_rate_limit.min(i32::MAX as u32) as i32,
            announce_interval: s.dht_announce_interval.min(i32::MAX as u32) as i32,
            ..DhtSettings::default()
        }
    }
}
