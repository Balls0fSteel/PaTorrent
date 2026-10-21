// PaTorrent's own code, not ported: libtorrent has no remote session.

//! The remote protocol's encoding: every message is one bencoded list, a variant name first,
//! then the fields in declaration order (positional: both ends run the same [`PROTOCOL`]
//! version, which the upgrade request names). Structs are lists of their fields, `Option`s are
//! lists of zero or one element, flags are integers, addresses are text, floats are their bit
//! patterns. Encoding writes bencode directly; decoding reads a `bdecode` tree and fails (with
//! `None`) on anything malformed, so a corrupt frame never yields a half-filled value.
//!
//! The struct and enum codecs destructure exhaustively, so a field or variant added to a type
//! without its codec does not compile.
//!
//! [`PROTOCOL`]: super::PROTOCOL

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use crate::alert::{Alert, BlockReason, FileOperation, PortmapTransport, Protocol, ProxyOperation};
use crate::bencode::{ListIter, Node, bdecode_with_limits};
use crate::error::Error;
use crate::metainfo::WebSeedKind;
use crate::params::{
    AddTorrentParams, DownloadPriority, MoveFlags, QueueMove, RemoveFlags, TorrentFlags, TorrentSource,
};
use crate::session::Command;
use crate::settings::{EncLevel, EncPolicy, ProxyType, Settings};
use crate::status::{
    AnnounceEntry, BlockState, FileEntry, PartialPiece, PeerFlags, PeerInfo, PeerSource, SessionStats,
    TorrentState, TorrentStatus, TrackerStatus,
};
use crate::types::{InfoHashes, Sha1Hash, Sha256Hash, TorrentId};
use crate::util::bitfield::Bitfield;

/// A bencode writer.
#[derive(Default)]
pub(crate) struct Enc(pub Vec<u8>);

impl Enc {
    pub fn int(&mut self, i: i64) {
        self.0.push(b'i');
        self.0.extend_from_slice(i.to_string().as_bytes());
        self.0.push(b'e');
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b.len().to_string().as_bytes());
        self.0.push(b':');
        self.0.extend_from_slice(b);
    }

    pub fn list(&mut self) {
        self.0.push(b'l');
    }

    pub fn end(&mut self) {
        self.0.push(b'e');
    }
}

/// A value the protocol carries.
pub(crate) trait Wire: Sized {
    fn put(&self, e: &mut Enc);
    fn get(n: Node<'_>) -> Option<Self>;
}

/// The elements of a list node, read in order.
pub(crate) struct Fields<'t>(ListIter<'t>);

impl<'t> Fields<'t> {
    pub fn of(n: Node<'t>) -> Option<Fields<'t>> {
        n.is_list().then(|| Fields(n.list_iter()))
    }

    pub fn next<T: Wire>(&mut self) -> Option<T> {
        T::get(self.0.next()?)
    }

    pub fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.0.next()?;
        n.is_string().then(|| n.string_value().to_vec())
    }
}

/// Decodes one message: its variant name and a reader positioned after it. The token limit
/// scales with the frame (a status update of every torrent is one message).
pub(crate) fn decode<T>(frame: &[u8], f: impl FnOnce(&str, &mut Fields<'_>) -> Option<T>) -> Option<T> {
    let tokens = (frame.len() / 2).max(1000);
    let tree = bdecode_with_limits(frame, crate::bencode::DEFAULT_DEPTH_LIMIT, tokens).ok()?;
    let mut fields = Fields::of(tree.root()?)?;
    let name: String = fields.next()?;
    f(&name, &mut fields)
}

// --- primitives ------------------------------------------------------------------------------

macro_rules! wire_int {
    ($($t:ty),*) => {$(
        impl Wire for $t {
            fn put(&self, e: &mut Enc) {
                e.int(*self as i64);
            }
            fn get(n: Node<'_>) -> Option<Self> {
                n.try_int_value().map(|i| i as $t)
            }
        }
    )*};
}

wire_int!(u8, u16, u32, u64, usize, i32, i64);

impl Wire for bool {
    fn put(&self, e: &mut Enc) {
        e.int(i64::from(*self));
    }
    fn get(n: Node<'_>) -> Option<Self> {
        Some(n.try_int_value()? != 0)
    }
}

impl Wire for f32 {
    fn put(&self, e: &mut Enc) {
        e.int(i64::from(self.to_bits()));
    }
    fn get(n: Node<'_>) -> Option<Self> {
        Some(f32::from_bits(n.try_int_value()? as u32))
    }
}

impl Wire for String {
    fn put(&self, e: &mut Enc) {
        e.bytes(self.as_bytes());
    }
    fn get(n: Node<'_>) -> Option<Self> {
        n.is_string().then(|| String::from_utf8_lossy(n.string_value()).into_owned())
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn put(&self, e: &mut Enc) {
        e.list();
        for x in self {
            x.put(e);
        }
        e.end();
    }
    fn get(n: Node<'_>) -> Option<Self> {
        n.is_list().then_some(())?;
        n.list_iter().map(T::get).collect()
    }
}

impl<T: Wire> Wire for Option<T> {
    fn put(&self, e: &mut Enc) {
        e.list();
        if let Some(x) = self {
            x.put(e);
        }
        e.end();
    }
    fn get(n: Node<'_>) -> Option<Self> {
        let mut f = Fields::of(n)?;
        match n.list_size() {
            0 => Some(None),
            1 => Some(Some(f.next()?)),
            _ => None,
        }
    }
}

impl Wire for Arc<[FileEntry]> {
    fn put(&self, e: &mut Enc) {
        e.list();
        for x in self.iter() {
            x.put(e);
        }
        e.end();
    }
    fn get(n: Node<'_>) -> Option<Self> {
        Vec::<FileEntry>::get(n).map(Arc::from)
    }
}

/// Raw bytes (a `.torrent`, resume data), as a bencoded string.
struct Bytes(Vec<u8>);

impl Wire for Bytes {
    fn put(&self, e: &mut Enc) {
        e.bytes(&self.0);
    }
    fn get(n: Node<'_>) -> Option<Self> {
        n.is_string().then(|| Bytes(n.string_value().to_vec()))
    }
}

impl Wire for Arc<[u8]> {
    fn put(&self, e: &mut Enc) {
        e.bytes(self);
    }
    fn get(n: Node<'_>) -> Option<Self> {
        n.is_string().then(|| Arc::from(n.string_value()))
    }
}

impl Wire for [u8; 20] {
    fn put(&self, e: &mut Enc) {
        e.bytes(self);
    }
    fn get(n: Node<'_>) -> Option<Self> {
        n.is_string().then_some(())?;
        n.string_value().try_into().ok()
    }
}

impl Wire for Sha1Hash {
    fn put(&self, e: &mut Enc) {
        e.bytes(&self.0);
    }
    fn get(n: Node<'_>) -> Option<Self> {
        n.is_string().then_some(())?;
        Some(Sha1Hash(n.string_value().try_into().ok()?))
    }
}

impl Wire for Sha256Hash {
    fn put(&self, e: &mut Enc) {
        e.bytes(&self.0);
    }
    fn get(n: Node<'_>) -> Option<Self> {
        n.is_string().then_some(())?;
        Some(Sha256Hash(n.string_value().try_into().ok()?))
    }
}

impl Wire for TorrentId {
    fn put(&self, e: &mut Enc) {
        e.int(self.to_raw() as i64);
    }
    fn get(n: Node<'_>) -> Option<Self> {
        Some(TorrentId::from_raw(n.try_int_value()? as u64))
    }
}

impl Wire for SocketAddr {
    fn put(&self, e: &mut Enc) {
        e.bytes(self.to_string().as_bytes());
    }
    fn get(n: Node<'_>) -> Option<Self> {
        n.str_value()?.parse().ok()
    }
}

impl Wire for IpAddr {
    fn put(&self, e: &mut Enc) {
        e.bytes(self.to_string().as_bytes());
    }
    fn get(n: Node<'_>) -> Option<Self> {
        n.str_value()?.parse().ok()
    }
}

impl Wire for Bitfield {
    fn put(&self, e: &mut Enc) {
        e.list();
        e.int(self.len() as i64);
        e.bytes(self.as_bytes());
        e.end();
    }
    fn get(n: Node<'_>) -> Option<Self> {
        let mut f = Fields::of(n)?;
        let len: usize = f.next()?;
        Bitfield::from_bytes(&f.bytes()?, len)
    }
}

impl Wire for DownloadPriority {
    fn put(&self, e: &mut Enc) {
        e.int(i64::from(self.0));
    }
    fn get(n: Node<'_>) -> Option<Self> {
        Some(DownloadPriority(u8::try_from(n.try_int_value()?).ok()?))
    }
}

impl Wire for BTreeMap<u32, String> {
    fn put(&self, e: &mut Enc) {
        e.list();
        for (k, v) in self {
            k.put(e);
            v.put(e);
        }
        e.end();
    }
    fn get(n: Node<'_>) -> Option<Self> {
        let mut f = Fields::of(n)?;
        let mut out = BTreeMap::new();
        for _ in 0..n.list_size() / 2 {
            out.insert(f.next()?, f.next()?);
        }
        (n.list_size() % 2 == 0).then_some(out)
    }
}

/// Flag sets travel as their bits.
macro_rules! wire_flags {
    ($($t:ident),*) => {$(
        impl Wire for $t {
            fn put(&self, e: &mut Enc) {
                e.int(i64::from(self.bits()));
            }
            fn get(n: Node<'_>) -> Option<Self> {
                Some($t::from_bits_truncate(n.try_int_value()? as _))
            }
        }
    )*};
}

wire_flags!(TorrentFlags, RemoveFlags, PeerFlags, PeerSource);

/// Field-less enums travel as the variant's position in this list.
macro_rules! wire_unit {
    ($t:ident { $($v:ident),* $(,)? }) => {
        impl Wire for $t {
            fn put(&self, e: &mut Enc) {
                const ALL: &[$t] = &[$($t::$v),*];
                // exhaustive: a new variant must be added to the list
                match self { $($t::$v => {})* }
                e.int(ALL.iter().position(|v| v == self).expect("listed") as i64);
            }
            fn get(n: Node<'_>) -> Option<Self> {
                const ALL: &[$t] = &[$($t::$v),*];
                ALL.get(usize::try_from(n.try_int_value()?).ok()?).copied()
            }
        }
    };
}

wire_unit!(TorrentState {
    CheckingFiles,
    DownloadingMetadata,
    Downloading,
    Finished,
    Seeding,
    CheckingResumeData
});
wire_unit!(TrackerStatus { NotContacted, Working, Updating, Error });
wire_unit!(MoveFlags { AlwaysReplaceFiles, FailIfExist, DontReplace, ResetSavePath, ResetSavePathUnchecked });
wire_unit!(QueueMove { Up, Down, Top, Bottom });
wire_unit!(FileOperation { Unknown, FileStat, Mkdir, FileRename, FileCopy, PartfileMove });
wire_unit!(Protocol { Tcp, Udp });
wire_unit!(PortmapTransport { Upnp, NatPmp });
wire_unit!(BlockReason { IpFilter, PortFilter });
wire_unit!(ProxyOperation { HostnameLookup, Connect, Handshake, SockRead });
wire_unit!(WebSeedKind { UrlSeed, HttpSeed });
wire_unit!(EncPolicy { Forced, Enabled, Disabled });
wire_unit!(EncLevel { Plaintext, Rc4, Both });
wire_unit!(ProxyType { None, Socks4, Socks5, Socks5Pw, Http, HttpPw });

/// Structs travel as the list of their fields.
macro_rules! wire_struct {
    ($t:ident { $($f:ident),* $(,)? }) => {
        impl Wire for $t {
            fn put(&self, e: &mut Enc) {
                let $t { $($f),* } = self;
                e.list();
                $( $f.put(e); )*
                e.end();
            }
            fn get(n: Node<'_>) -> Option<Self> {
                let mut f = Fields::of(n)?;
                Some($t { $($f: f.next()?,)* })
            }
        }
    };
}

wire_struct!(InfoHashes { v1, v2 });
wire_struct!(TorrentStatus {
    id,
    info_hashes,
    name,
    save_path,
    state,
    flags,
    error,
    has_metadata,
    is_finished,
    is_seeding,
    super_seeding,
    moving_storage,
    progress_ppm,
    queue_position,
    download_rate,
    upload_rate,
    download_payload_rate,
    upload_payload_rate,
    num_seeds,
    num_peers,
    list_seeds,
    list_peers,
    num_complete,
    num_incomplete,
    num_connections,
    distributed_copies,
    total_download,
    total_upload,
    total_payload_download,
    total_payload_upload,
    total_failed_bytes,
    total_redundant_bytes,
    all_time_download,
    all_time_upload,
    total,
    total_done,
    total_wanted,
    total_wanted_done,
    num_pieces,
    pieces_total,
    piece_length,
    added_time,
    completed_time,
    last_seen_complete,
    active_duration,
    finished_duration,
    seeding_duration,
    current_tracker,
    next_announce,
    num_trackers,
    announce_interval,
    announcing_to_trackers,
    announcing_to_lsd,
    announcing_to_dht,
    download_limit,
    upload_limit,
    connections_limit,
    uploads_limit,
});
wire_struct!(SessionStats {
    dht_nodes,
    num_peers_connected,
    num_torrents,
    num_downloading,
    num_seeding,
    recv_bytes,
    sent_bytes,
    recv_payload_bytes,
    sent_payload_bytes,
    download_rate,
    upload_rate,
    has_incoming_connections,
    disk_read_queue,
    disk_write_queue,
    utp_sockets,
    utp_packets_in,
    utp_packets_out,
    utp_packet_loss,
    utp_timeouts,
    utp_resends,
    disk_blocks_read,
    disk_read_ops,
    disk_cache_hits,
    read_cache_bytes,
});
wire_struct!(PeerInfo {
    ip,
    client,
    flags,
    source,
    progress_ppm,
    down_speed,
    up_speed,
    total_download,
    total_upload,
    download_queue_length,
    upload_queue_length,
    num_hashfails,
    is_web_seed,
});
wire_struct!(PartialPiece { piece, size, blocks });
wire_struct!(AnnounceEntry {
    url,
    tier,
    status,
    message,
    scrape_complete,
    scrape_incomplete,
    scrape_downloaded,
    next_announce,
    fails,
});
wire_struct!(FileEntry { path, size, offset, first_piece, last_piece, pad_file, priority });
wire_struct!(Settings {
    user_agent,
    peer_fingerprint,
    listen_interfaces,
    dht_bootstrap_nodes,
    download_rate_limit,
    upload_rate_limit,
    active_downloads,
    active_seeds,
    active_checking,
    active_dht_limit,
    active_tracker_limit,
    active_lsd_limit,
    active_limit,
    dont_count_slow_torrents,
    auto_manage_interval,
    connections_limit,
    connection_speed,
    max_retry_port_bind,
    unchoke_slots_limit,
    max_peerlist_size,
    max_failcount,
    min_reconnect_time,
    max_paused_peerlist_size,
    enable_dht,
    dht_announce_interval,
    dht_upload_rate_limit,
    dht_read_only,
    dht_privacy_lookups,
    dht_enforce_node_id,
    dht_restrict_routing_ips,
    dht_restrict_search_ips,
    dht_prefer_verified_node_ids,
    dht_extended_routing_table,
    dht_aggressive_lookups,
    dht_ignore_dark_internet,
    dht_max_peers_reply,
    dht_search_branching,
    dht_max_fail_count,
    dht_max_torrents,
    dht_max_peers,
    dht_block_timeout,
    dht_block_ratelimit,
    enable_lsd,
    local_service_announce_interval,
    enable_upnp,
    enable_natpmp,
    upnp_ignore_nonrouters,
    upnp_lease_duration,
    enable_incoming_tcp,
    enable_outgoing_tcp,
    enable_incoming_utp,
    enable_outgoing_utp,
    utp_target_delay,
    utp_gain_factor,
    utp_min_timeout,
    utp_syn_resends,
    utp_fin_resends,
    utp_num_resends,
    utp_connect_timeout,
    utp_loss_multiplier,
    utp_cwnd_reduce_timer,
    out_enc_policy,
    in_enc_policy,
    allowed_enc_level,
    prefer_rc4,
    stop_tracker_timeout,
    tracker_completion_timeout,
    tracker_receive_timeout,
    min_announce_interval,
    num_want,
    announce_to_all_tiers,
    announce_to_all_trackers,
    apply_ip_filter_to_trackers,
    request_timeout,
    use_parole_mode,
    peer_timeout,
    peer_connect_timeout,
    handshake_timeout,
    request_queue_time,
    max_out_request_queue,
    max_allowed_in_request_queue,
    unchoke_interval,
    optimistic_unchoke_interval,
    send_buffer_watermark,
    close_redundant_connections,
    peer_turnover,
    peer_turnover_cutoff,
    peer_turnover_interval,
    recv_socket_buffer_size,
    send_socket_buffer_size,
    max_metadata_size,
    max_pex_peers,
    alert_queue_size,
    alert_mask,
    tick_interval,
    aio_threads,
    hashing_threads,
    file_pool_size,
    max_queued_disk_bytes,
    use_read_cache,
    cache_size,
    read_cache_line_size,
    cache_expiry,
    urlseed_pipeline_size,
    urlseed_timeout,
    urlseed_wait_retry,
    urlseed_max_request_bytes,
    web_seed_name_lookup_retry,
    max_web_seed_connections,
    ban_web_seeds,
    ssrf_mitigation,
    allow_idna,
    always_send_user_agent,
    proxy_type,
    proxy_hostname,
    proxy_port,
    proxy_username,
    proxy_password,
    proxy_hostnames,
    proxy_peer_connections,
    proxy_tracker_connections,
    proxy_send_host_in_connect,
    socks5_udp_send_local_ep,
});

impl Wire for BlockState {
    fn put(&self, e: &mut Enc) {
        // 0 free, 1 writing, 2 finished, 3 + n requested by n peers
        e.int(match *self {
            BlockState::Free => 0,
            BlockState::Writing => 1,
            BlockState::Finished => 2,
            BlockState::Requested(n) => 3 + i64::from(n),
        });
    }
    fn get(n: Node<'_>) -> Option<Self> {
        Some(match n.try_int_value()? {
            0 => BlockState::Free,
            1 => BlockState::Writing,
            2 => BlockState::Finished,
            i @ 3..=258 => BlockState::Requested((i - 3) as u8),
            _ => return None,
        })
    }
}

/// Errors travel as their kind and text; the engine's own unit errors keep their identity
/// (frontends test for [`Error::DuplicateTorrent`]), the rest come back as I/O errors with the
/// same message.
impl Wire for Error {
    fn put(&self, e: &mut Enc) {
        let tag = match self {
            Error::DuplicateTorrent => 0,
            Error::MismatchingInfoHash => 1,
            Error::InvalidTorrentHandle => 2,
            Error::SessionClosed => 3,
            Error::Io { kind: std::io::ErrorKind::NotFound, .. } => 4,
            Error::Io { kind: std::io::ErrorKind::PermissionDenied, .. } => 5,
            Error::Io { kind: std::io::ErrorKind::AlreadyExists, .. } => 6,
            _ => 7,
        };
        e.list();
        e.int(tag);
        e.bytes(self.to_string().as_bytes());
        e.end();
    }
    fn get(n: Node<'_>) -> Option<Self> {
        let mut f = Fields::of(n)?;
        let tag: u8 = f.next()?;
        let message: String = f.next()?;
        let io = |kind| Error::Io { kind, message: message.clone() };
        Some(match tag {
            0 => Error::DuplicateTorrent,
            1 => Error::MismatchingInfoHash,
            2 => Error::InvalidTorrentHandle,
            3 => Error::SessionClosed,
            4 => io(std::io::ErrorKind::NotFound),
            5 => io(std::io::ErrorKind::PermissionDenied),
            6 => io(std::io::ErrorKind::AlreadyExists),
            _ => io(std::io::ErrorKind::Other),
        })
    }
}

impl Wire for TorrentSource {
    fn put(&self, e: &mut Enc) {
        e.list();
        match self {
            TorrentSource::TorrentFile(b) => {
                e.int(0);
                e.bytes(b);
            }
            TorrentSource::Magnet(m) => {
                e.int(1);
                m.put(e);
            }
            TorrentSource::Resume(b) => {
                e.int(2);
                e.bytes(b);
            }
            TorrentSource::InfoHash(h) => {
                e.int(3);
                h.put(e);
            }
        }
        e.end();
    }
    fn get(n: Node<'_>) -> Option<Self> {
        let mut f = Fields::of(n)?;
        Some(match f.next::<u8>()? {
            0 => TorrentSource::TorrentFile(f.next::<Bytes>()?.0),
            1 => TorrentSource::Magnet(f.next()?),
            2 => TorrentSource::Resume(f.next::<Bytes>()?.0),
            3 => TorrentSource::InfoHash(f.next()?),
            _ => return None,
        })
    }
}

wire_struct!(AddTorrentParams {
    source,
    save_path,
    name,
    flags,
    trackers,
    tracker_tiers,
    file_priorities,
    url_seeds,
    http_seeds,
    download_limit,
    upload_limit,
    max_connections,
    max_uploads,
    renamed_files,
});

// --- messages --------------------------------------------------------------------------------

/// Writes a message: `[name, fields...]`.
macro_rules! message {
    ($e:expr, $name:literal $(, $f:expr)*) => {{
        let e: &mut Enc = $e;
        e.list();
        e.bytes($name.as_bytes());
        $( $f.put(e); )*
        e.end();
    }};
}

/// A command from a remote client. `None` for what does not cross the network: a session's
/// IP filter tables (they are the server's), the test hook, and `Shutdown` (the client closing).
pub(crate) fn encode_command(c: &Command, e: &mut Enc) -> bool {
    match c {
        Command::AddTorrent(p) => message!(e, "add", **p),
        Command::RemoveTorrent(id, flags) => message!(e, "remove", id, flags),
        Command::Pause(id) => message!(e, "pause", id),
        Command::Resume(id) => message!(e, "resume", id),
        Command::SetFlags { id, flags, mask } => message!(e, "set_flags", id, flags, mask),
        Command::Queue(id, m) => message!(e, "queue", id, m),
        Command::ForceRecheck(id) => message!(e, "recheck", id),
        Command::ForceReannounce(id) => message!(e, "reannounce", id),
        Command::ScrapeTracker(id, index) => message!(e, "scrape", id, index),
        Command::ScrapeTrackers(ids) => message!(e, "scrape_all", ids),
        Command::SetFilePriority { id, file, priority } => message!(e, "file_priority", id, file, priority),
        Command::PrioritizeFiles(id, p) => message!(e, "prioritize_files", id, p),
        Command::SetDownloadLimit(id, l) => message!(e, "download_limit", id, l),
        Command::SetUploadLimit(id, l) => message!(e, "upload_limit", id, l),
        Command::SetMaxConnections(id, l) => message!(e, "max_connections", id, l),
        Command::SetMaxUploads(id, l) => message!(e, "max_uploads", id, l),
        Command::ReplaceTrackers(id, t) => message!(e, "replace_trackers", id, t),
        Command::AddTracker(id, t) => message!(e, "add_tracker", id, t),
        Command::MoveStorage(id, path, flags) => message!(e, "move_storage", id, path, flags),
        Command::RenameFile(id, index, name) => message!(e, "rename_file", id, index, name),
        Command::ConnectPeer(id, addr) => message!(e, "connect_peer", id, addr),
        Command::AddWebSeed(id, url, kind) => message!(e, "add_web_seed", id, url, kind),
        Command::RemoveWebSeed(id, url, kind) => message!(e, "remove_web_seed", id, url, kind),
        Command::PostWebSeeds(id) => message!(e, "post_web_seeds", id),
        Command::PostTorrentUpdates => message!(e, "post_torrent_updates"),
        Command::PostSessionStats => message!(e, "post_session_stats"),
        Command::SaveResumeData => message!(e, "save_resume_data"),
        Command::PostPeerInfo(id) => message!(e, "post_peer_info", id),
        Command::PostFileList(id) => message!(e, "post_file_list", id),
        Command::PostFileProgress(id) => message!(e, "post_file_progress", id),
        Command::PostTrackers(id) => message!(e, "post_trackers", id),
        Command::PostPieceInfo(id) => message!(e, "post_piece_info", id),
        Command::PostTorrentFile(id) => message!(e, "post_torrent_file", id),
        Command::ApplySettings(s) => message!(e, "apply_settings", **s),
        Command::PauseSession => message!(e, "pause_session"),
        Command::ResumeSession => message!(e, "resume_session"),
        Command::ReopenNetworkSockets => message!(e, "reopen_network_sockets"),
        Command::SetIpFilter(_) | Command::SetPortFilter(_) | Command::Shutdown => return false,
        #[cfg(test)]
        Command::SetFakeNetwork(_) => return false,
    }
    true
}

pub(crate) fn decode_command(frame: &[u8]) -> Option<Command> {
    decode(frame, |name, f| {
        Some(match name {
            "add" => Command::AddTorrent(Box::new(f.next()?)),
            "remove" => Command::RemoveTorrent(f.next()?, f.next()?),
            "pause" => Command::Pause(f.next()?),
            "resume" => Command::Resume(f.next()?),
            "set_flags" => Command::SetFlags { id: f.next()?, flags: f.next()?, mask: f.next()? },
            "queue" => Command::Queue(f.next()?, f.next()?),
            "recheck" => Command::ForceRecheck(f.next()?),
            "reannounce" => Command::ForceReannounce(f.next()?),
            "scrape" => Command::ScrapeTracker(f.next()?, f.next()?),
            "scrape_all" => Command::ScrapeTrackers(f.next()?),
            "file_priority" => {
                Command::SetFilePriority { id: f.next()?, file: f.next()?, priority: f.next()? }
            }
            "prioritize_files" => Command::PrioritizeFiles(f.next()?, f.next()?),
            "download_limit" => Command::SetDownloadLimit(f.next()?, f.next()?),
            "upload_limit" => Command::SetUploadLimit(f.next()?, f.next()?),
            "max_connections" => Command::SetMaxConnections(f.next()?, f.next()?),
            "max_uploads" => Command::SetMaxUploads(f.next()?, f.next()?),
            "replace_trackers" => Command::ReplaceTrackers(f.next()?, f.next()?),
            "add_tracker" => Command::AddTracker(f.next()?, f.next()?),
            "move_storage" => Command::MoveStorage(f.next()?, f.next()?, f.next()?),
            "rename_file" => Command::RenameFile(f.next()?, f.next()?, f.next()?),
            "connect_peer" => Command::ConnectPeer(f.next()?, f.next()?),
            "add_web_seed" => Command::AddWebSeed(f.next()?, f.next()?, f.next()?),
            "remove_web_seed" => Command::RemoveWebSeed(f.next()?, f.next()?, f.next()?),
            "post_web_seeds" => Command::PostWebSeeds(f.next()?),
            "post_torrent_updates" => Command::PostTorrentUpdates,
            "post_session_stats" => Command::PostSessionStats,
            "save_resume_data" => Command::SaveResumeData,
            "post_peer_info" => Command::PostPeerInfo(f.next()?),
            "post_file_list" => Command::PostFileList(f.next()?),
            "post_file_progress" => Command::PostFileProgress(f.next()?),
            "post_trackers" => Command::PostTrackers(f.next()?),
            "post_piece_info" => Command::PostPieceInfo(f.next()?),
            "post_torrent_file" => Command::PostTorrentFile(f.next()?),
            "apply_settings" => Command::ApplySettings(Box::new(f.next()?)),
            "pause_session" => Command::PauseSession,
            "resume_session" => Command::ResumeSession,
            "reopen_network_sockets" => Command::ReopenNetworkSockets,
            _ => return None,
        })
    })
}

pub(crate) fn encode_alert(a: &Alert, e: &mut Enc) {
    match a {
        Alert::TorrentAdded { id, info_hashes, name } => message!(e, "TorrentAdded", id, info_hashes, name),
        Alert::AddTorrentFailed { name, error } => message!(e, "AddTorrentFailed", name, error),
        Alert::TorrentRemoved { id, info_hashes } => message!(e, "TorrentRemoved", id, info_hashes),
        Alert::TorrentDeleted { id, info_hashes } => message!(e, "TorrentDeleted", id, info_hashes),
        Alert::TorrentDeleteFailed { id, info_hashes, error } => {
            message!(e, "TorrentDeleteFailed", id, info_hashes, error)
        }
        Alert::StorageMoved { id, old, new } => message!(e, "StorageMoved", id, old, new),
        Alert::StorageMovedFailed { id, error, file, operation } => {
            message!(e, "StorageMovedFailed", id, error, file, operation)
        }
        Alert::FileRenamed { id, index, old_name, new_name } => {
            message!(e, "FileRenamed", id, index, old_name, new_name)
        }
        Alert::FileRenameFailed { id, index, error } => message!(e, "FileRenameFailed", id, index, error),
        Alert::StateChanged { id, state, prev_state } => message!(e, "StateChanged", id, state, prev_state),
        Alert::TorrentPaused { id } => message!(e, "TorrentPaused", id),
        Alert::TorrentResumed { id } => message!(e, "TorrentResumed", id),
        Alert::TorrentChecked { id } => message!(e, "TorrentChecked", id),
        Alert::TorrentFinished { id } => message!(e, "TorrentFinished", id),
        Alert::MetadataReceived { id } => message!(e, "MetadataReceived", id),
        Alert::MetadataFailed { id, error } => message!(e, "MetadataFailed", id, error),
        Alert::TorrentError { id, error } => message!(e, "TorrentError", id, error),
        Alert::FileError { id, file, error } => message!(e, "FileError", id, file, error),
        Alert::TrackerReply { id, url, num_peers } => message!(e, "TrackerReply", id, url, num_peers),
        Alert::TrackerWarning { id, url, message } => message!(e, "TrackerWarning", id, url, message),
        Alert::TrackerError { id, url, times_in_row, message } => {
            message!(e, "TrackerError", id, url, times_in_row, message)
        }
        Alert::ScrapeReply { id, url, complete, incomplete } => {
            message!(e, "ScrapeReply", id, url, complete, incomplete)
        }
        Alert::ScrapeFailed { id, url, message } => message!(e, "ScrapeFailed", id, url, message),
        Alert::ListenSucceeded { address, protocol } => message!(e, "ListenSucceeded", address, protocol),
        Alert::ListenFailed { address, protocol, error } => {
            message!(e, "ListenFailed", address, protocol, error)
        }
        Alert::PortmapSucceeded { transport, protocol, external_port } => {
            message!(e, "PortmapSucceeded", transport, protocol, external_port)
        }
        Alert::PortmapFailed { transport, message } => message!(e, "PortmapFailed", transport, message),
        Alert::PortmapLog { transport, message } => message!(e, "PortmapLog", transport, message),
        Alert::ExternalIp { ip } => message!(e, "ExternalIp", ip),
        Alert::DhtReply { id, num_peers } => message!(e, "DhtReply", id, num_peers),
        Alert::DhtBootstrap => message!(e, "DhtBootstrap"),
        Alert::LsdPeer { id, address } => message!(e, "LsdPeer", id, address),
        Alert::LsdError { address, error } => message!(e, "LsdError", address, error),
        Alert::PeerBanned { id, addr, peer_id } => message!(e, "PeerBanned", id, addr, peer_id),
        Alert::PeerBlocked { id, addr, reason } => message!(e, "PeerBlocked", id, addr, reason),
        Alert::PeerDisconnected { id, addr, peer_id, utp, reason, error } => {
            message!(e, "PeerDisconnected", id, addr, peer_id, utp, reason, error)
        }
        Alert::Proxy { proxy, operation, error } => message!(e, "Proxy", proxy, operation, error),
        Alert::UrlSeed { id, url, message } => message!(e, "UrlSeed", id, url, message),
        Alert::StateUpdate(v) => message!(e, "StateUpdate", v),
        Alert::SessionStats(s) => message!(e, "SessionStats", s),
        Alert::ResumeDataSaved => message!(e, "ResumeDataSaved"),
        Alert::PeerInfo { id, peers } => message!(e, "PeerInfo", id, peers),
        Alert::FileList { id, files } => message!(e, "FileList", id, files),
        Alert::FileProgress { id, done } => message!(e, "FileProgress", id, done),
        Alert::TrackerList { id, trackers } => message!(e, "TrackerList", id, trackers),
        Alert::PieceInfo { id, have, availability, downloading } => {
            message!(e, "PieceInfo", id, have, availability, downloading)
        }
        Alert::WebSeeds { id, url_seeds, http_seeds } => message!(e, "WebSeeds", id, url_seeds, http_seeds),
        Alert::TorrentFile { id, data } => message!(e, "TorrentFile", id, data),
        Alert::Log { id, message } => message!(e, "Log", id, message),
        Alert::AlertsDropped { count } => message!(e, "AlertsDropped", count),
        Alert::RemoteDisconnected { message } => message!(e, "RemoteDisconnected", message),
    }
}

pub(crate) fn decode_alert(frame: &[u8]) -> Option<Alert> {
    decode(frame, |name, f| {
        Some(match name {
            "TorrentAdded" => Alert::TorrentAdded { id: f.next()?, info_hashes: f.next()?, name: f.next()? },
            "AddTorrentFailed" => Alert::AddTorrentFailed { name: f.next()?, error: f.next()? },
            "TorrentRemoved" => Alert::TorrentRemoved { id: f.next()?, info_hashes: f.next()? },
            "TorrentDeleted" => Alert::TorrentDeleted { id: f.next()?, info_hashes: f.next()? },
            "TorrentDeleteFailed" => {
                Alert::TorrentDeleteFailed { id: f.next()?, info_hashes: f.next()?, error: f.next()? }
            }
            "StorageMoved" => Alert::StorageMoved { id: f.next()?, old: f.next()?, new: f.next()? },
            "StorageMovedFailed" => Alert::StorageMovedFailed {
                id: f.next()?,
                error: f.next()?,
                file: f.next()?,
                operation: f.next()?,
            },
            "FileRenamed" => Alert::FileRenamed {
                id: f.next()?,
                index: f.next()?,
                old_name: f.next()?,
                new_name: f.next()?,
            },
            "FileRenameFailed" => {
                Alert::FileRenameFailed { id: f.next()?, index: f.next()?, error: f.next()? }
            }
            "StateChanged" => Alert::StateChanged { id: f.next()?, state: f.next()?, prev_state: f.next()? },
            "TorrentPaused" => Alert::TorrentPaused { id: f.next()? },
            "TorrentResumed" => Alert::TorrentResumed { id: f.next()? },
            "TorrentChecked" => Alert::TorrentChecked { id: f.next()? },
            "TorrentFinished" => Alert::TorrentFinished { id: f.next()? },
            "MetadataReceived" => Alert::MetadataReceived { id: f.next()? },
            "MetadataFailed" => Alert::MetadataFailed { id: f.next()?, error: f.next()? },
            "TorrentError" => Alert::TorrentError { id: f.next()?, error: f.next()? },
            "FileError" => Alert::FileError { id: f.next()?, file: f.next()?, error: f.next()? },
            "TrackerReply" => Alert::TrackerReply { id: f.next()?, url: f.next()?, num_peers: f.next()? },
            "TrackerWarning" => Alert::TrackerWarning { id: f.next()?, url: f.next()?, message: f.next()? },
            "TrackerError" => Alert::TrackerError {
                id: f.next()?,
                url: f.next()?,
                times_in_row: f.next()?,
                message: f.next()?,
            },
            "ScrapeReply" => Alert::ScrapeReply {
                id: f.next()?,
                url: f.next()?,
                complete: f.next()?,
                incomplete: f.next()?,
            },
            "ScrapeFailed" => Alert::ScrapeFailed { id: f.next()?, url: f.next()?, message: f.next()? },
            "ListenSucceeded" => Alert::ListenSucceeded { address: f.next()?, protocol: f.next()? },
            "ListenFailed" => {
                Alert::ListenFailed { address: f.next()?, protocol: f.next()?, error: f.next()? }
            }
            "PortmapSucceeded" => Alert::PortmapSucceeded {
                transport: f.next()?,
                protocol: f.next()?,
                external_port: f.next()?,
            },
            "PortmapFailed" => Alert::PortmapFailed { transport: f.next()?, message: f.next()? },
            "PortmapLog" => Alert::PortmapLog { transport: f.next()?, message: f.next()? },
            "ExternalIp" => Alert::ExternalIp { ip: f.next()? },
            "DhtReply" => Alert::DhtReply { id: f.next()?, num_peers: f.next()? },
            "DhtBootstrap" => Alert::DhtBootstrap,
            "LsdPeer" => Alert::LsdPeer { id: f.next()?, address: f.next()? },
            "LsdError" => Alert::LsdError { address: f.next()?, error: f.next()? },
            "PeerBanned" => Alert::PeerBanned { id: f.next()?, addr: f.next()?, peer_id: f.next()? },
            "PeerBlocked" => Alert::PeerBlocked { id: f.next()?, addr: f.next()?, reason: f.next()? },
            "PeerDisconnected" => Alert::PeerDisconnected {
                id: f.next()?,
                addr: f.next()?,
                peer_id: f.next()?,
                utp: f.next()?,
                reason: f.next()?,
                error: f.next()?,
            },
            "Proxy" => Alert::Proxy { proxy: f.next()?, operation: f.next()?, error: f.next()? },
            "UrlSeed" => Alert::UrlSeed { id: f.next()?, url: f.next()?, message: f.next()? },
            "StateUpdate" => Alert::StateUpdate(f.next()?),
            "SessionStats" => Alert::SessionStats(f.next()?),
            "ResumeDataSaved" => Alert::ResumeDataSaved,
            "PeerInfo" => Alert::PeerInfo { id: f.next()?, peers: f.next()? },
            "FileList" => Alert::FileList { id: f.next()?, files: f.next()? },
            "FileProgress" => Alert::FileProgress { id: f.next()?, done: f.next()? },
            "TrackerList" => Alert::TrackerList { id: f.next()?, trackers: f.next()? },
            "PieceInfo" => Alert::PieceInfo {
                id: f.next()?,
                have: f.next()?,
                availability: f.next()?,
                downloading: f.next()?,
            },
            "WebSeeds" => Alert::WebSeeds { id: f.next()?, url_seeds: f.next()?, http_seeds: f.next()? },
            "TorrentFile" => Alert::TorrentFile { id: f.next()?, data: f.next()? },
            "Log" => Alert::Log { id: f.next()?, message: f.next()? },
            "AlertsDropped" => Alert::AlertsDropped { count: f.next()? },
            "RemoteDisconnected" => Alert::RemoteDisconnected { message: f.next()? },
            _ => return None,
        })
    })
}

/// What `f` writes, as bytes.
#[cfg(test)]
pub(crate) fn to_bytes(f: impl FnOnce(&mut Enc)) -> Vec<u8> {
    let mut e = Enc::default();
    f(&mut e);
    e.0
}
