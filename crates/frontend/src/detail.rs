//! The detail tabs under the torrent list (General, Trackers, Peers, Pieces, Files, Speed,
//! Logger) in µTorrent 2.2.1's layout: what each GUI keeps for the selected torrent
//! ([`Detail`]), the General tab's fields, and the tables' column titles, default widths and
//! cell text. The drawing is the toolkit's.

use std::collections::VecDeque;
use std::sync::Arc;

use patorrent_core::{
    AnnounceEntry, Bitfield, BlockState, DownloadPriority, FileEntry, PartialPiece, PeerFlags, PeerInfo,
    PeerSource, TorrentId, TorrentStatus, TrackerStatus,
};

use crate::lang::S;
use crate::status::status_text;
use crate::{fmt, tr};

/// Title, width at 96 DPI, right-aligned.
pub type ColumnDef = (S, i32, bool);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    General,
    Trackers,
    Peers,
    Pieces,
    Files,
    Speed,
    Logger,
}

impl Tab {
    pub const ALL: [Tab; 7] =
        [Tab::General, Tab::Trackers, Tab::Peers, Tab::Pieces, Tab::Files, Tab::Speed, Tab::Logger];

    pub fn title(self) -> S {
        match self {
            Tab::General => S::TabGeneral,
            Tab::Trackers => S::TabTrackers,
            Tab::Peers => S::TabPeers,
            Tab::Pieces => S::TabPieces,
            Tab::Files => S::TabFiles,
            Tab::Speed => S::TabSpeed,
            Tab::Logger => S::TabLogger,
        }
    }

    /// The name kept in the preferences (`ui.tab`), whatever the language.
    pub fn key(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Trackers => "Trackers",
            Tab::Peers => "Peers",
            Tab::Pieces => "Pieces",
            Tab::Files => "Files",
            Tab::Speed => "Speed",
            Tab::Logger => "Logger",
        }
    }

    pub fn from_key(key: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.key() == key)
    }

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }

    /// Tabs that are one plain table; General, Pieces and Speed are drawn (Pieces with a table
    /// of its own under the map).
    pub fn is_list(self) -> bool {
        matches!(self, Tab::Trackers | Tab::Peers | Tab::Files | Tab::Logger)
    }

    /// The table's columns ([`PIECE_COLUMNS`] for Pieces).
    pub fn columns(self) -> &'static [ColumnDef] {
        const LOG: [ColumnDef; 1] = [(S::ColMessage, 900, false)];
        match self {
            Tab::Trackers => &TRACKER_COLUMNS,
            Tab::Peers => &PEER_COLUMNS,
            Tab::Pieces => &PIECE_COLUMNS,
            Tab::Files => &FILE_COLUMNS,
            Tab::Logger => &LOG,
            _ => &[],
        }
    }
}

/// Lines the Logger keeps.
pub const LOG_LINES: usize = 1000;
/// Seconds of speed history.
pub const SPEED_SAMPLES: usize = 600;

/// What the detail tabs show for the selected torrent, plus the session-wide log and speeds.
#[derive(Default)]
pub struct Detail {
    pub id: Option<TorrentId>,
    pub peers: Vec<PeerInfo>,
    pub trackers: Vec<AnnounceEntry>,
    pub files: Option<Arc<[FileEntry]>>,
    /// Indices into `files` without pad files.
    pub file_rows: Vec<usize>,
    pub file_done: Vec<u64>,
    pub have: Option<Bitfield>,
    pub availability: Vec<u32>,
    /// The pieces being downloaded, ascending (the Pieces list).
    pub partial: Vec<PartialPiece>,
    pub log: VecDeque<String>,
    /// (download, upload) payload bytes per second, oldest first.
    pub speed: VecDeque<(u32, u32)>,
}

impl Detail {
    /// Switches to another torrent, dropping the previous one's data.
    pub fn select(&mut self, id: Option<TorrentId>) {
        if self.id != id {
            self.id = id;
            self.peers.clear();
            self.trackers.clear();
            self.files = None;
            self.file_rows.clear();
            self.file_done.clear();
            self.have = None;
            self.availability.clear();
            self.partial.clear();
        }
    }

    pub fn set_files(&mut self, files: Arc<[FileEntry]>) {
        self.file_rows = (0..files.len()).filter(|&i| !files[i].pad_file).collect();
        self.files = Some(files);
    }

    pub fn push_log(&mut self, line: String) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    /// The engine's peer list for the Peers tab (see [`listed_peers`]).
    pub fn set_peers(&mut self, peers: Vec<PeerInfo>) {
        self.peers = listed_peers(peers);
    }

    pub fn push_speed(&mut self, down: u32, up: u32) {
        if self.speed.len() == SPEED_SAMPLES {
            self.speed.pop_front();
        }
        self.speed.push_back((down, up));
    }

    pub fn rows(&self, tab: Tab) -> usize {
        match tab {
            Tab::Trackers => self.trackers.len(),
            Tab::Peers => self.peers.len(),
            Tab::Pieces => self.partial.len(),
            Tab::Files => self.file_rows.len(),
            Tab::Logger => self.log.len(),
            _ => 0,
        }
    }

    pub fn cell(&self, tab: Tab, row: usize, col: usize) -> String {
        match tab {
            Tab::Trackers => self.trackers.get(row).map(|t| tracker_cell(t, col)),
            Tab::Peers => self.peers.get(row).map(|p| peer_cell(p, col)),
            Tab::Pieces => self.partial.get(row).map(|p| piece_cell(p, col)),
            Tab::Files => self.file_rows.get(row).and_then(|&i| {
                let f = &self.files.as_ref()?[i];
                Some(file_cell(f, self.file_done.get(i).copied(), col))
            }),
            Tab::Logger => self.log.get(row).cloned(),
            _ => None,
        }
        .unwrap_or_default()
    }

    /// File index (into the torrent's file list) of a Files tab row.
    pub fn file_index(&self, row: usize) -> Option<u32> {
        self.file_rows.get(row).map(|&i| i as u32)
    }

    /// The piece map's colour for piece `i`: on disk, being downloaded, or by how many peers
    /// have it.
    pub fn piece_shade(&self, i: usize) -> PieceShade {
        if self.have.as_ref().is_some_and(|h| h.get(i)) {
            PieceShade::Have
        } else if self.partial.binary_search_by_key(&(i as u32), |p| p.piece).is_ok() {
            PieceShade::Downloading
        } else {
            PieceShade::Available(self.availability.get(i).copied().unwrap_or(0).min(4) as u8)
        }
    }

    /// The text above the piece map.
    pub fn pieces_summary(&self) -> String {
        let Some(have) = &self.have else { return String::new() };
        match self.partial.len() {
            0 => tr!(PiecesSummary, count = have.len(), done = have.count()),
            k => tr!(PiecesSummaryDownloading, count = have.len(), done = have.count(), downloading = k),
        }
    }
}

/// How the piece map shows a piece.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PieceShade {
    Have,
    Downloading,
    /// Peers that have it, capped at 4.
    Available(u8),
}

/// The General tab's texts for one torrent: labels with their values, in µTorrent 2.2.1's
/// layout.
pub struct General {
    /// The Downloaded and Availability bars' labels and values.
    pub bars: [(&'static str, String); 2],
    /// The Transfer section: four rows of three.
    pub transfer: [[(&'static str, String); 3]; 4],
    /// The Transfer section's last line, across the width.
    pub status: (&'static str, String),
    /// The General section: three rows of two (the left one wider).
    pub general: [[(&'static str, String); 2]; 3],
    /// The General section's last line.
    pub hash: (&'static str, String),
}

pub fn general(st: &TorrentStatus, user_paused: bool) -> General {
    let remaining = st.total_wanted.saturating_sub(st.total_wanted_done);
    let pieces = if st.piece_length > 0 {
        tr!(
            GenPiecesValue,
            count = st.pieces_total,
            size = fmt::size(st.piece_length as u64),
            have = st.num_pieces
        )
    } else {
        String::new()
    };
    General {
        bars: [
            (tr!(GenDownloaded), fmt::percent(st.progress_ppm)),
            (tr!(GenAvailability), fmt::availability(st.distributed_copies)),
        ],
        transfer: [
            [
                (tr!(GenTimeElapsed), fmt::duration(st.active_duration)),
                (tr!(GenRemaining), if st.is_finished { String::new() } else { fmt::eta(st.eta()) }),
                (tr!(GenWasted), fmt::size(st.total_failed_bytes + st.total_redundant_bytes)),
            ],
            [
                (tr!(GenDownloaded), fmt::size(st.all_time_download)),
                (tr!(GenUploaded), fmt::size(st.all_time_upload)),
                (
                    tr!(GenSeeds),
                    tr!(GenConnected, count = st.num_seeds, total = st.num_complete.unwrap_or(st.list_seeds)),
                ),
            ],
            [
                (tr!(GenDownloadSpeed), fmt::speed_always(st.download_payload_rate)),
                (tr!(GenUploadSpeed), fmt::speed_always(st.upload_payload_rate)),
                (
                    tr!(GenPeers),
                    tr!(
                        GenConnected,
                        count = st.num_peers.saturating_sub(st.num_seeds),
                        total = st.num_incomplete.unwrap_or(st.list_peers.saturating_sub(st.list_seeds))
                    ),
                ),
            ],
            [
                (tr!(GenDownLimit), limit(st.download_limit)),
                (tr!(GenUpLimit), limit(st.upload_limit)),
                (tr!(GenShareRatio), fmt::ratio(st.ratio())),
            ],
        ],
        status: (tr!(GenStatus), status_text(st, user_paused)),
        general: [
            [(tr!(GenSaveAs), st.save_path.clone()), (tr!(GenRemaining), fmt::size(remaining))],
            [(tr!(GenTotalSize), fmt::size(st.total)), (tr!(GenAddedOn), fmt::date_time(st.added_time))],
            [(tr!(GenPieces), pieces), (tr!(GenCompletedOn), fmt::date_time(st.completed_time))],
        ],
        hash: (tr!(GenHash), st.info_hashes.key_hex().to_uppercase()),
    }
}

/// A rate limit: `∞` for none.
pub fn limit(v: u32) -> String {
    if v == 0 { "∞".to_owned() } else { fmt::speed_always(v) }
}

/// The Speed graph's top: the highest sample (at least 10 kB/s) rounded up to 1, 2 or 5 × 10ⁿ
/// kB/s.
pub fn speed_scale(speed: &VecDeque<(u32, u32)>) -> u64 {
    let max = speed.iter().map(|&(a, b)| a.max(b)).max().unwrap_or(0).max(10 * 1024) as u64;
    let mut base = 1024u64;
    loop {
        for m in [1, 2, 5] {
            if base * m >= max {
                return base * m;
            }
        }
        base *= 10;
    }
}

pub const TRACKER_COLUMNS: [ColumnDef; 6] = [
    (S::ColName, 300, false),
    (S::ColStatus, 200, false),
    (S::ColUpdateIn, 80, true),
    (S::ColSeeds, 60, true),
    (S::ColPeers, 60, true),
    (S::ColDownloaded, 80, true),
];

pub fn tracker_cell(t: &AnnounceEntry, col: usize) -> String {
    let opt = |v: Option<u32>| v.map(|v| v.to_string()).unwrap_or_default();
    match col {
        0 => t.url.clone(),
        1 => tracker_status(t),
        2 => t.next_announce.map(|s| fmt::duration(s as u64)).unwrap_or_default(),
        3 => opt(t.scrape_complete),
        4 => opt(t.scrape_incomplete),
        5 => opt(t.scrape_downloaded),
        _ => String::new(),
    }
}

pub fn tracker_status(t: &AnnounceEntry) -> String {
    match t.status {
        TrackerStatus::NotContacted => String::new(),
        TrackerStatus::Working if t.message.is_empty() => tr!(TrackerWorking).to_owned(),
        TrackerStatus::Working => tr!(TrackerWorkingMessage, message = t.message),
        TrackerStatus::Updating => tr!(Updating).to_owned(),
        TrackerStatus::Error if t.message.is_empty() => tr!(TrackerError).to_owned(),
        TrackerStatus::Error => t.message.clone(),
    }
}

pub const PEER_COLUMNS: [ColumnDef; 9] = [
    (S::ColIp, 160, false),
    (S::ColClient, 130, false),
    (S::ColFlags, 60, false),
    (S::ColPercent, 50, true),
    (S::ColDownSpeed, 75, true),
    (S::ColUpSpeed, 75, true),
    (S::ColReqs, 50, true),
    (S::ColDownloaded, 75, true),
    (S::ColUploaded, 75, true),
];

/// The peers the Peers tab lists: those past the handshake, as in µTorrent, whose tab showed
/// the connected peers its Peers column counts. A connection still being made or in its
/// handshake has no client, no progress and no flags yet, and would show as an empty row.
pub fn listed_peers(peers: Vec<PeerInfo>) -> Vec<PeerInfo> {
    let mut peers = peers;
    peers.retain(|p| !p.flags.intersects(PeerFlags::CONNECTING | PeerFlags::HANDSHAKE));
    peers
}

/// A peer's cell. A web seed shows its URL in the IP column (its `client` is the server's
/// `Server` header), as µTorrent did.
pub fn peer_cell(p: &PeerInfo, col: usize) -> String {
    match col {
        0 => p.url.clone().unwrap_or_else(|| p.ip.to_string()),
        1 => p.client.clone(),
        2 => peer_flags(p),
        3 => format!("{:.1}", p.progress_ppm as f64 / 10_000.0),
        4 => fmt::speed(p.down_speed),
        5 => fmt::speed(p.up_speed),
        6 => format!("{} | {}", p.download_queue_length, p.upload_queue_length),
        7 => fmt::size(p.total_download),
        8 => fmt::size(p.total_upload),
        _ => String::new(),
    }
}

/// µTorrent's peer flags: `D`/`d` downloading from them (unchoked / choked), `U`/`u` uploading
/// to them (unchoked / choked), `O` optimistic unchoke, `S` snubbed, `I` incoming, `E`/`e`
/// encrypted (RC4 / header only), `P` uTP, `H` DHT, `X` PEX, `L` local discovery.
pub fn peer_flags(p: &PeerInfo) -> String {
    let f = p.flags;
    let mut s = String::new();
    if f.contains(PeerFlags::INTERESTING) {
        s.push(if f.contains(PeerFlags::REMOTE_CHOKED) { 'd' } else { 'D' });
    }
    if f.contains(PeerFlags::REMOTE_INTERESTED) {
        s.push(if f.contains(PeerFlags::CHOKED) { 'u' } else { 'U' });
    }
    if f.contains(PeerFlags::OPTIMISTIC_UNCHOKE) {
        s.push('O');
    }
    if f.contains(PeerFlags::SNUBBED) {
        s.push('S');
    }
    if p.source.contains(PeerSource::INCOMING) {
        s.push('I');
    }
    if f.contains(PeerFlags::RC4_ENCRYPTED) {
        s.push('E');
    } else if f.contains(PeerFlags::PLAINTEXT_ENCRYPTED) {
        s.push('e');
    }
    if f.contains(PeerFlags::UTP_SOCKET) {
        s.push('P');
    }
    if p.source.contains(PeerSource::DHT) {
        s.push('H');
    }
    if p.source.contains(PeerSource::PEX) {
        s.push('X');
    }
    if p.source.contains(PeerSource::LSD) {
        s.push('L');
    }
    s
}

/// The Pieces tab's list of pieces being downloaded. The Blocks cell is drawn, not text: one
/// mark per block, coloured by [`block_kind`].
pub const PIECE_COLUMNS: [ColumnDef; 5] = [
    (S::ColPiece, 60, true),
    (S::ColSize, 75, true),
    (S::ColBlockCount, 75, true),
    (S::ColBlocks, 320, false),
    (S::ColBlocksCompleted, 75, true),
];

/// The Blocks column's index in [`PIECE_COLUMNS`].
pub const PIECE_BLOCKS_COLUMN: usize = 3;

pub fn piece_cell(p: &PartialPiece, col: usize) -> String {
    match col {
        0 => p.piece.to_string(),
        1 => fmt::size(p.size as u64),
        2 => p.blocks.len().to_string(),
        4 => p.finished().to_string(),
        _ => String::new(),
    }
}

/// How the Blocks cell shows a block, in drawing order from empty to done.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockKind {
    Free,
    Requested,
    Writing,
    Finished,
}

impl BlockKind {
    /// One letter per kind, for the Web UI's block strings.
    pub fn letter(self) -> char {
        match self {
            BlockKind::Free => '-',
            BlockKind::Requested => 'r',
            BlockKind::Writing => 'w',
            BlockKind::Finished => 'f',
        }
    }
}

pub fn block_kind(b: BlockState) -> BlockKind {
    match b {
        BlockState::Free => BlockKind::Free,
        BlockState::Requested(_) => BlockKind::Requested,
        BlockState::Writing => BlockKind::Writing,
        BlockState::Finished => BlockKind::Finished,
    }
}

/// The blocks as runs of one kind: (kind, first block, count), in order.
pub fn block_runs(p: &PartialPiece) -> Vec<(BlockKind, usize, usize)> {
    let mut runs: Vec<(BlockKind, usize, usize)> = Vec::new();
    for (i, &b) in p.blocks.iter().enumerate() {
        let k = block_kind(b);
        match runs.last_mut() {
            Some((last, _, n)) if *last == k => *n += 1,
            _ => runs.push((k, i, 1)),
        }
    }
    runs
}

pub const FILE_COLUMNS: [ColumnDef; 7] = [
    (S::ColName, 300, false),
    (S::ColSize, 75, true),
    (S::ColDone, 75, true),
    (S::ColPercent, 50, true),
    (S::ColPieceCount, 60, true),
    (S::ColFirstPiece, 70, true),
    (S::ColPriority, 90, false),
];

/// `done` is the file's completed bytes, if known.
pub fn file_cell(f: &FileEntry, done: Option<u64>, col: usize) -> String {
    match col {
        0 => f.path.clone(),
        1 => fmt::size(f.size),
        2 => done.map(fmt::size).unwrap_or_default(),
        3 => match done {
            Some(d) if f.size > 0 => format!("{:.1} %", (d as f64 / f.size as f64 * 1000.0).floor() / 10.0),
            Some(_) => "100.0 %".to_owned(),
            None => String::new(),
        },
        4 => (f.last_piece - f.first_piece + 1).to_string(),
        5 => f.first_piece.to_string(),
        6 => priority_text(f.priority).to_owned(),
        _ => String::new(),
    }
}

/// µTorrent's three file priorities plus "skip".
pub fn priority_text(p: DownloadPriority) -> &'static str {
    match p.0 {
        0 => tr!(PrioDontDownload),
        1..=3 => tr!(PrioLow),
        4 => tr!(PrioNormal),
        _ => tr!(PrioHigh),
    }
}

/// The priorities the Files tab's menu offers, in menu order.
pub const PRIORITIES: [(S, DownloadPriority); 4] = [
    (S::PrioHighItem, DownloadPriority::TOP),
    (S::PrioNormalItem, DownloadPriority::NORMAL),
    (S::PrioLowItem, DownloadPriority::LOW),
    (S::PrioDontDownload, DownloadPriority::DONT_DOWNLOAD),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(flags: PeerFlags, source: PeerSource) -> PeerInfo {
        PeerInfo {
            ip: "10.0.0.1:6881".parse().unwrap(),
            client: "qBittorrent 5.2.4".into(),
            flags,
            source,
            progress_ppm: 500_000,
            down_speed: 0,
            up_speed: 0,
            total_download: 0,
            total_upload: 0,
            download_queue_length: 3,
            upload_queue_length: 0,
            num_hashfails: 0,
            is_web_seed: false,
            url: None,
        }
    }

    #[test]
    fn the_peers_tab_lists_connected_peers_only() {
        let mut d = Detail::default();
        d.set_peers(vec![
            peer(PeerFlags::CONNECTING, PeerSource::DHT),
            peer(PeerFlags::HANDSHAKE | PeerFlags::OUTGOING_CONNECTION, PeerSource::TRACKER),
            peer(PeerFlags::INTERESTING, PeerSource::PEX),
        ]);
        assert_eq!(d.rows(Tab::Peers), 1);
        assert_eq!(d.cell(Tab::Peers, 0, 1), "qBittorrent 5.2.4");
        assert_eq!(d.cell(Tab::Peers, 0, 2), "DX");
    }

    #[test]
    fn peer_flag_letters() {
        let p = peer(
            PeerFlags::INTERESTING | PeerFlags::REMOTE_INTERESTED | PeerFlags::CHOKED,
            PeerSource::INCOMING,
        );
        assert_eq!(peer_flags(&p), "DuI");
        let p = peer(
            PeerFlags::INTERESTING | PeerFlags::REMOTE_CHOKED | PeerFlags::RC4_ENCRYPTED,
            PeerSource::TRACKER,
        );
        assert_eq!(peer_flags(&p), "dE");
        assert_eq!(peer_cell(&p, 3), "50.0");
        assert_eq!(peer_cell(&p, 6), "3 | 0");
        assert_eq!(peer_cell(&p, 0), "10.0.0.1:6881");
        // a web seed: the URL in the IP column, the server header as the client
        let w = PeerInfo {
            is_web_seed: true,
            url: Some("http://mirror.example/ubuntu.iso".into()),
            client: "nginx".into(),
            ..p
        };
        assert_eq!(peer_cell(&w, 0), "http://mirror.example/ubuntu.iso");
        assert_eq!(peer_cell(&w, 1), "nginx");
    }

    #[test]
    fn piece_rows_and_block_runs() {
        let p = PartialPiece {
            piece: 41,
            size: 4 * 16 * 1024,
            blocks: vec![
                BlockState::Finished,
                BlockState::Finished,
                BlockState::Writing,
                BlockState::Requested(2),
                BlockState::Free,
            ],
        };
        let cells: Vec<String> = (0..PIECE_COLUMNS.len()).map(|c| piece_cell(&p, c)).collect();
        assert_eq!(cells, ["41", "64.0 kB", "5", "", "2"]);
        assert_eq!(PIECE_COLUMNS[PIECE_BLOCKS_COLUMN].0, S::ColBlocks);
        assert_eq!(
            block_runs(&p),
            [
                (BlockKind::Finished, 0, 2),
                (BlockKind::Writing, 2, 1),
                (BlockKind::Requested, 3, 1),
                (BlockKind::Free, 4, 1)
            ]
        );
        let letters: String = p.blocks.iter().map(|&b| block_kind(b).letter()).collect();
        assert_eq!(letters, "ffwr-");
        assert!(block_runs(&PartialPiece { piece: 0, size: 0, blocks: Vec::new() }).is_empty());
    }

    #[test]
    fn trackers_and_files() {
        let mut t = AnnounceEntry { url: "udp://t.example:1/announce".into(), ..Default::default() };
        assert_eq!(tracker_cell(&t, 1), "");
        t.status = TrackerStatus::Error;
        t.message = "timed out".into();
        assert_eq!(tracker_cell(&t, 1), "timed out");
        t.next_announce = Some(125);
        assert_eq!(tracker_cell(&t, 2), "2m 5s");

        let f = FileEntry {
            path: "a/b.iso".into(),
            size: 1000,
            offset: 0,
            first_piece: 2,
            last_piece: 5,
            pad_file: false,
            priority: DownloadPriority::NORMAL,
        };
        assert_eq!(file_cell(&f, Some(999), 3), "99.9 %");
        assert_eq!(file_cell(&f, None, 2), "");
        assert_eq!(file_cell(&f, None, 4), "4");
        assert_eq!(file_cell(&f, None, 6), "Normal");
        assert_eq!(priority_text(DownloadPriority::TOP), "High");
    }

    #[test]
    fn the_pieces_rows_follow_the_selected_torrent() {
        let mut d = Detail::default();
        d.select(Some(TorrentId::from_raw(1)));
        d.partial = vec![PartialPiece {
            piece: 12,
            size: 32 * 1024,
            blocks: vec![BlockState::Finished, BlockState::Requested(1)],
        }];
        let mut have = Bitfield::new(20, false);
        have.set(3, true);
        d.have = Some(have);
        d.availability = vec![7; 20];
        assert_eq!(d.rows(Tab::Pieces), 1);
        assert!(!Tab::Pieces.is_list(), "a drawn tab holding its own list");
        let cells: Vec<String> = (0..5).map(|c| d.cell(Tab::Pieces, 0, c)).collect();
        assert_eq!(cells, ["12", "32.0 kB", "2", "", "1"]);
        assert_eq!(d.piece_shade(3), PieceShade::Have);
        assert_eq!(d.piece_shade(12), PieceShade::Downloading);
        assert_eq!(d.piece_shade(0), PieceShade::Available(4));
        assert_eq!(d.pieces_summary(), tr!(PiecesSummaryDownloading, count = 20, done = 1, downloading = 1));
        d.select(Some(TorrentId::from_raw(2)));
        assert_eq!(d.rows(Tab::Pieces), 0);
        assert_eq!(d.pieces_summary(), "");
    }

    #[test]
    fn tabs_log_and_speed_scale() {
        for t in Tab::ALL {
            assert_eq!(Tab::from_key(t.key()), Some(t));
            assert_eq!(Tab::ALL[t.index()], t);
        }
        assert_eq!(Tab::from_key("nope"), None);
        let mut d = Detail::default();
        for i in 0..LOG_LINES + 5 {
            d.push_log(i.to_string());
        }
        assert_eq!(d.rows(Tab::Logger), LOG_LINES);
        assert_eq!(d.cell(Tab::Logger, 0, 0), "5");
        assert_eq!(speed_scale(&d.speed), 10 * 1024);
        d.push_speed(30 * 1024, 1);
        assert_eq!(speed_scale(&d.speed), 50 * 1024);
        d.push_speed(0, 101 * 1024);
        assert_eq!(speed_scale(&d.speed), 200 * 1024);
        assert_eq!(limit(0), "∞");
    }
}
