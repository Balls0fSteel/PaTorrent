//! Status snapshots handed to frontends: libtorrent `torrent_status`, `peer_info`,
//! `announce_entry` and session counters. Field names follow libtorrent; "unknown" values that
//! libtorrent encodes as -1 are `Option`s here.

use std::net::SocketAddr;

use crate::flags::bit_flags;
use crate::params::{DownloadPriority, TorrentFlags};
use crate::types::{InfoHashes, TorrentId};

/// libtorrent `torrent_status::state_t`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum TorrentState {
    /// Verifying existing data on disk.
    CheckingFiles,
    /// Magnet / bare info-hash: fetching the info dictionary from peers.
    DownloadingMetadata,
    #[default]
    Downloading,
    /// All wanted pieces are done, but not every piece (some files are skipped).
    Finished,
    Seeding,
    /// Validating fast-resume data before starting.
    CheckingResumeData,
}

/// One torrent's state as of the last session tick (libtorrent `torrent_status`).
#[derive(Clone, Debug, Default)]
pub struct TorrentStatus {
    pub id: TorrentId,
    pub info_hashes: InfoHashes,
    pub name: String,
    pub save_path: String,
    pub state: TorrentState,
    pub flags: TorrentFlags,
    /// Empty when there is no error.
    pub error: String,
    pub has_metadata: bool,
    pub is_finished: bool,
    pub is_seeding: bool,
    /// BEP 16 super seeding (µTorrent "initial seeding") is in effect (libtorrent
    /// `torrent_status::super_seeding`).
    pub super_seeding: bool,
    pub moving_storage: bool,

    /// Progress of the current task (download or check), 0..=1_000_000.
    pub progress_ppm: u32,
    /// `None` when the torrent is not in the download queue (for example, seeding).
    pub queue_position: Option<u32>,

    /// Bytes per second, including protocol overhead.
    pub download_rate: u32,
    pub upload_rate: u32,
    /// Bytes per second, payload only. This is what µTorrent shows as Down/Up Speed.
    pub download_payload_rate: u32,
    pub upload_payload_rate: u32,

    /// Connected seeds / peers (peers include seeds).
    pub num_seeds: u32,
    pub num_peers: u32,
    /// Known in the peer list, not necessarily connected.
    pub list_seeds: u32,
    pub list_peers: u32,
    /// From the last tracker scrape; `None` if unknown.
    pub num_complete: Option<u32>,
    pub num_incomplete: Option<u32>,
    pub num_connections: u32,
    /// Swarm availability ("distributed copies"); `None` if not computed.
    pub distributed_copies: Option<f32>,

    /// Byte counters. `total_*` are since the torrent was started in this session;
    /// `all_time_*` persist through resume data.
    pub total_download: u64,
    pub total_upload: u64,
    pub total_payload_download: u64,
    pub total_payload_upload: u64,
    pub total_failed_bytes: u64,
    pub total_redundant_bytes: u64,
    pub all_time_download: u64,
    pub all_time_upload: u64,
    /// Size of all files / of wanted files, and how much of each is done.
    pub total: u64,
    pub total_done: u64,
    pub total_wanted: u64,
    pub total_wanted_done: u64,

    pub num_pieces: u32,
    pub pieces_total: u32,
    pub piece_length: u32,

    /// Unix seconds; 0 = never / unknown.
    pub added_time: i64,
    pub completed_time: i64,
    pub last_seen_complete: i64,

    /// Seconds.
    pub active_duration: u64,
    pub finished_duration: u64,
    pub seeding_duration: u64,

    pub current_tracker: String,
    /// Seconds until the next tracker announce; `None` if none is scheduled.
    pub next_announce: Option<u32>,
    /// How many trackers the torrent has (not in libtorrent's `torrent_status`).
    pub num_trackers: u32,
    /// The longest `interval` the torrent's trackers asked for in their last announce replies
    /// this session, in seconds; `None` before any reply (not in libtorrent's `torrent_status`).
    pub announce_interval: Option<u32>,
    /// Per-torrent limits in bytes per second; 0 = unlimited.
    pub download_limit: u32,
    pub upload_limit: u32,
    /// Per-torrent connection and upload slot limits (libtorrent `connections_limit`,
    /// `uploads_limit`); `None` = no limit of its own.
    pub connections_limit: Option<u32>,
    pub uploads_limit: Option<u32>,
}

impl TorrentStatus {
    /// Share ratio as µTorrent shows it: uploaded / downloaded (all time). Infinite when nothing
    /// was downloaded but something was uploaded.
    pub fn ratio(&self) -> f64 {
        let down = self.all_time_download.max(self.total_wanted_done);
        match (self.all_time_upload, down) {
            (0, _) => 0.0,
            (_, 0) => f64::INFINITY,
            (up, down) => up as f64 / down as f64,
        }
    }

    /// Estimated seconds until the wanted pieces are done; `None` if not downloading or stalled.
    pub fn eta(&self) -> Option<u64> {
        if self.is_finished || self.download_payload_rate == 0 {
            return None;
        }
        let left = self.total_wanted.saturating_sub(self.total_wanted_done);
        Some(left.div_ceil(self.download_payload_rate as u64))
    }
}

/// Session-wide counters (a named subset of libtorrent's `session_stats_alert` metrics; Phase 2
/// adds the full counter table).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionStats {
    pub dht_nodes: u32,
    pub num_peers_connected: u32,
    pub num_torrents: u32,
    pub num_downloading: u32,
    pub num_seeding: u32,
    /// Totals since session start, including protocol overhead.
    pub recv_bytes: u64,
    pub sent_bytes: u64,
    pub recv_payload_bytes: u64,
    pub sent_payload_bytes: u64,
    /// Bytes per second, payload.
    pub download_rate: u32,
    pub upload_rate: u32,
    /// True once any incoming connection succeeded: the port is reachable (µTorrent's green
    /// network indicator).
    pub has_incoming_connections: bool,
    pub disk_read_queue: u32,
    pub disk_write_queue: u32,
    /// uTP sockets alive, connected ones included (libtorrent `utp_stats`).
    pub utp_sockets: u32,
    pub utp_packets_in: u64,
    pub utp_packets_out: u64,
    /// Packets found lost (fast retransmits and timeouts).
    pub utp_packet_loss: u64,
    pub utp_timeouts: u64,
    pub utp_resends: u64,
    /// Blocks read for peers (libtorrent `disk.num_blocks_read`), the file reads made for them
    /// (`disk.num_read_ops`; a read cache line counts once) and those answered from the read
    /// cache (`disk.num_blocks_cache_hits`), since session start.
    pub disk_blocks_read: u64,
    pub disk_read_ops: u64,
    pub disk_cache_hits: u64,
    /// Bytes held by the read cache.
    pub read_cache_bytes: u64,
}

bit_flags! {
    /// libtorrent `peer_flags_t`.
    pub struct PeerFlags: u32 {
        /// We are interested in pieces from this peer.
        const INTERESTING = 0;
        /// We have choked this peer.
        const CHOKED = 1;
        const REMOTE_INTERESTED = 2;
        const REMOTE_CHOKED = 3;
        const SUPPORTS_EXTENSIONS = 4;
        /// We initiated the connection.
        const OUTGOING_CONNECTION = 5;
        const HANDSHAKE = 6;
        const CONNECTING = 7;
        const ON_PAROLE = 9;
        const SEED = 10;
        const OPTIMISTIC_UNCHOKE = 11;
        const SNUBBED = 12;
        const UPLOAD_ONLY = 13;
        const ENDGAME_MODE = 14;
        const HOLEPUNCHED = 15;
        const UTP_SOCKET = 17;
        const SSL_SOCKET = 18;
        const RC4_ENCRYPTED = 19;
        const PLAINTEXT_ENCRYPTED = 20;
    }
}

bit_flags! {
    /// libtorrent `peer_source_flags_t`: how we learned about the peer.
    pub struct PeerSource: u8 {
        const TRACKER = 0;
        const DHT = 1;
        const PEX = 2;
        const LSD = 3;
        const RESUME_DATA = 4;
        const INCOMING = 5;
    }
}

/// One connected peer (libtorrent `peer_info`, subset).
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub ip: SocketAddr,
    /// Client name decoded from the peer id / extension handshake.
    pub client: String,
    pub flags: PeerFlags,
    pub source: PeerSource,
    pub progress_ppm: u32,
    /// Payload bytes per second.
    pub down_speed: u32,
    pub up_speed: u32,
    pub total_download: u64,
    pub total_upload: u64,
    /// Outstanding block requests we sent / they sent.
    pub download_queue_length: u32,
    pub upload_queue_length: u32,
    pub num_hashfails: u32,
    /// True for web seeds (BEP 17/19) rather than BitTorrent peers.
    pub is_web_seed: bool,
}

/// The state of one block of a piece being downloaded (libtorrent `block_info::block_state_t`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockState {
    /// Not requested from anyone.
    Free,
    /// Requested from this many peers (more than one only in end-game).
    Requested(u8),
    /// Received and handed to the disk.
    Writing,
    /// On disk.
    Finished,
}

/// A piece with blocks requested, being written or on disk, but not yet passed its hash check
/// (libtorrent `partial_piece_info`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialPiece {
    pub piece: u32,
    /// Bytes in the piece (the last piece may be shorter).
    pub size: u32,
    /// One entry per block, in order.
    pub blocks: Vec<BlockState>,
}

impl PartialPiece {
    /// Blocks on disk (libtorrent `finished`).
    pub fn finished(&self) -> usize {
        self.blocks.iter().filter(|&&b| b == BlockState::Finished).count()
    }

    /// Blocks received and waiting for the disk (libtorrent `writing`).
    pub fn writing(&self) -> usize {
        self.blocks.iter().filter(|&&b| b == BlockState::Writing).count()
    }

    /// Blocks requested and not received yet (libtorrent `requested`).
    pub fn requested(&self) -> usize {
        self.blocks.iter().filter(|b| matches!(b, BlockState::Requested(_))).count()
    }
}

/// Tracker state as µTorrent's Trackers tab shows it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TrackerStatus {
    #[default]
    NotContacted,
    Working,
    Updating,
    Error,
}

/// One tracker URL (libtorrent `announce_entry`, endpoints folded into one row).
#[derive(Clone, Debug, Default)]
pub struct AnnounceEntry {
    pub url: String,
    pub tier: u32,
    pub status: TrackerStatus,
    /// Last error or warning message; empty if none.
    pub message: String,
    pub scrape_complete: Option<u32>,
    pub scrape_incomplete: Option<u32>,
    pub scrape_downloaded: Option<u32>,
    /// Seconds until the next announce.
    pub next_announce: Option<u32>,
    pub fails: u32,
}

impl AnnounceEntry {
    /// A tracker to hand to [`TorrentHandle::replace_trackers`] or
    /// [`TorrentHandle::add_tracker`](crate::TorrentHandle::add_tracker): only the URL and the
    /// tier are read there (libtorrent `announce_entry(url)` plus `tier`).
    ///
    /// [`TorrentHandle::replace_trackers`]: crate::TorrentHandle::replace_trackers
    pub fn new(url: impl Into<String>, tier: u32) -> AnnounceEntry {
        AnnounceEntry { url: url.into(), tier, ..AnnounceEntry::default() }
    }
}

/// One file in a torrent (libtorrent `file_storage` entry).
#[derive(Clone, Debug)]
pub struct FileEntry {
    /// Path relative to the save path, `/`-separated, UTF-8.
    pub path: String,
    pub size: u64,
    /// Byte offset of the file within the torrent.
    pub offset: u64,
    pub first_piece: u32,
    /// Inclusive.
    pub last_piece: u32,
    /// BEP 47 padding file (hidden from users).
    pub pad_file: bool,
    pub priority: DownloadPriority,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_and_eta() {
        let mut st = TorrentStatus { all_time_download: 200, all_time_upload: 100, ..Default::default() };
        assert_eq!(st.ratio(), 0.5);
        st.all_time_download = 0;
        assert!(st.ratio().is_infinite());
        st.all_time_upload = 0;
        assert_eq!(st.ratio(), 0.0);

        st.total_wanted = 1000;
        st.total_wanted_done = 400;
        assert_eq!(st.eta(), None);
        st.download_payload_rate = 100;
        assert_eq!(st.eta(), Some(6));
        st.is_finished = true;
        assert_eq!(st.eta(), None);
    }
}
