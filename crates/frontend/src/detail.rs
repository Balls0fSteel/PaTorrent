//! The detail tabs under the torrent list (General, Trackers, Peers, Pieces, Files, Speed,
//! Logger) in µTorrent 2.2.1's layout: what each GUI keeps for the selected torrent
//! ([`Detail`]), the General tab's fields, and the tables' column titles, default widths and
//! cell text. The drawing is the toolkit's.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use rookparcel_core::{
    AnnounceEntry, Bitfield, BlockState, DownloadPriority, FileEntry, PartialPiece, PeerFlags, PeerInfo,
    PeerSource, TorrentId, TorrentStatus, TrackerStatus,
};

use crate::add::FileTree;
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

/// The preference that keeps the Files tab's view (`1`: the folder tree, the default; `0`:
/// 2.2.1's plain list), read with `Prefs::get_bool(FILES_TREE_KEY, true)` (D62).
pub const FILES_TREE_KEY: &str = "ui.files_tree";

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
    /// The Files tab's rows: indices into `files` without pad files, only those
    /// [`Detail::file_filter`] lets through.
    pub file_rows: Vec<u32>,
    /// The Files tab's quick filter (D50). Kept while the selection moves to another torrent.
    pub file_filter: FileFilter,
    /// Completed bytes per file; set with [`Detail::set_file_done`].
    file_done: Vec<u64>,
    /// The Files tab shows the plain list of 2.2.1 instead of the folder tree (D62).
    flat_files: bool,
    /// The Files tab's folder tree, while it shows one and the file list is known.
    tree: Option<FileTree>,
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
            self.tree = None;
            self.have = None;
            self.availability.clear();
            self.partial.clear();
        }
    }

    /// The engine's file list of the selected torrent. A list of the same files again (after a
    /// priority change) refreshes the tree in place, so collapsed folders stay collapsed.
    pub fn set_files(&mut self, files: Arc<[FileEntry]>) {
        self.file_rows = self.file_filter.rows(&files);
        if let Some(t) = self.tree.as_mut().filter(|t| t.has_files(&files)) {
            t.set_file_states(&files);
        } else {
            self.tree = (!self.flat_files).then(|| self.build_tree(&files));
        }
        self.files = Some(files);
    }

    fn build_tree(&self, files: &[FileEntry]) -> FileTree {
        let mut t = FileTree::from_entries(files);
        t.set_done(&self.file_done);
        t.set_filter(self.file_filter.text());
        t
    }

    /// The engine's file progress: completed bytes per file.
    pub fn set_file_done(&mut self, done: Vec<u64>) {
        if let Some(t) = &mut self.tree {
            t.set_done(&done);
        }
        self.file_done = done;
    }

    /// Completed bytes per file, empty until the engine reported them.
    pub fn file_done(&self) -> &[u64] {
        &self.file_done
    }

    /// Whether the Files tab shows the folder tree (the default) rather than 2.2.1's plain
    /// list of paths (D62).
    pub fn files_tree(&self) -> bool {
        !self.flat_files
    }

    /// Switches the Files tab between the folder tree and the plain list; true when it changed
    /// (the view must reload its rows). The tree starts with every folder open.
    pub fn set_files_tree(&mut self, on: bool) -> bool {
        if self.flat_files != on {
            return false;
        }
        self.flat_files = !on;
        self.tree = match (&self.files, on) {
            (Some(files), true) => Some(self.build_tree(files)),
            _ => None,
        };
        true
    }

    /// The Files tab's tree while it shows one, for views that draw indents and expanders or
    /// keep their own expansion (macOS's outline view uses its nodes).
    pub fn file_tree(&self) -> Option<&FileTree> {
        self.tree.as_ref()
    }

    /// Row `row` of the Files tab as a tree row (name, depth, folder state); `None` in the
    /// plain list.
    pub fn file_tree_row(&self, row: usize) -> Option<crate::add::Row<'_>> {
        self.tree.as_ref()?.row(row)
    }

    /// Expands (`Some(true)`), collapses (`Some(false)`) or toggles (`None`) the folder in a
    /// Files tab row; true when the rows changed. See [`FileTree::set_expanded`].
    pub fn set_file_folder_expanded(&mut self, row: usize, to: Option<bool>) -> bool {
        self.tree.as_mut().is_some_and(|t| t.set_expanded(row, to))
    }

    /// The row of the folder holding a Files tab row (tree only), for Left on a file.
    pub fn file_parent_row(&self, row: usize) -> Option<usize> {
        self.tree.as_ref()?.parent_row(row)
    }

    /// Sets the Files tab's filter text; true when the visible rows may have changed.
    pub fn set_file_filter(&mut self, text: &str) -> bool {
        if !self.file_filter.set_text(text) {
            return false;
        }
        if let Some(files) = &self.files {
            self.file_rows = self.file_filter.rows(files);
        }
        if let Some(t) = &mut self.tree {
            t.set_filter(text);
        }
        true
    }

    /// The files (indices into the torrent's file list) in the given Files tab rows, ascending
    /// and once each: a folder row stands for its shown files. What the menu's priorities,
    /// Prioritize by File Order and Ctrl+A's selection act on.
    pub fn file_indexes(&self, rows: &[usize]) -> Vec<u32> {
        match &self.tree {
            Some(t) => t.files_under_rows(rows),
            None => {
                let mut v: Vec<u32> = rows.iter().filter_map(|&r| self.file_rows.get(r).copied()).collect();
                v.sort_unstable();
                v.dedup();
                v
            }
        }
    }

    /// The torrent's files without pad files, for "Prioritize by File Order", which orders the
    /// whole torrent whatever the filter shows.
    pub fn all_files(&self) -> Vec<u32> {
        self.files.as_deref().map(|f| FileFilter::default().rows(f)).unwrap_or_default()
    }

    /// The count beside the Files tab's filter box while a filter is on ("12 of 3,400
    /// files"), empty otherwise.
    pub fn file_filter_count(&self) -> String {
        match &self.files {
            Some(files) if self.file_filter.is_on() => {
                let total = files.iter().filter(|f| !f.pad_file).count();
                tr!(FilesFilterCount, shown = fmt::count(self.file_rows.len()), total = fmt::count(total))
            }
            _ => String::new(),
        }
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
            Tab::Files => self.tree.as_ref().map_or(self.file_rows.len(), FileTree::len),
            Tab::Logger => self.log.len(),
            _ => 0,
        }
    }

    pub fn cell(&self, tab: Tab, row: usize, col: usize) -> String {
        match tab {
            Tab::Trackers => self.trackers.get(row).map(|t| tracker_cell(t, col)),
            Tab::Peers => self.peers.get(row).map(|p| peer_cell(p, col)),
            Tab::Pieces => self.partial.get(row).map(|p| piece_cell(p, col)),
            Tab::Files => match &self.tree {
                Some(t) => t.row_node(row).map(|n| self.file_node_cell(n, col)),
                None => self.file_rows.get(row).and_then(|&i| {
                    let f = self.files.as_ref()?.get(i as usize)?;
                    Some(file_cell(f, self.file_done.get(i as usize).copied(), col))
                }),
            },
            Tab::Logger => self.log.get(row).cloned(),
            _ => None,
        }
        .unwrap_or_default()
    }

    /// A Files tab cell of tree node `node` ([`FileTree::children`]): a file's name (not its
    /// path) and columns, or a folder's sums (size, done, %), its one priority or "Mixed",
    /// and no piece columns.
    pub fn file_node_cell(&self, node: usize, col: usize) -> String {
        let Some(r) = self.tree.as_ref().and_then(|t| t.node(node)) else { return String::new() };
        let done = (!self.file_done.is_empty()).then_some(r.done);
        if let Some(fi) = r.file {
            let Some(f) = self.files.as_ref().and_then(|f| f.get(fi)) else { return String::new() };
            return match col {
                0 => r.name.to_owned(),
                _ => file_cell(f, done, col),
            };
        }
        match col {
            0 => r.name.to_owned(),
            1 => fmt::size(r.size),
            2 => done.map(fmt::size).unwrap_or_default(),
            3 => done.map(|d| percent_text(d, r.size)).unwrap_or_default(),
            6 => match r.priority {
                Some(p) => priority_text(p).to_owned(),
                None => tr!(AddMixed).to_owned(),
            },
            _ => String::new(),
        }
    }

    /// File index (into the torrent's file list) of a Files tab row; `None` for a folder.
    pub fn file_index(&self, row: usize) -> Option<u32> {
        match &self.tree {
            Some(t) => t.row(row)?.file.map(|f| f as u32),
            None => self.file_rows.get(row).copied(),
        }
    }

    /// Where a Files tab row's file (or folder, in the tree) is on disk, for Open and Open
    /// Containing Folder; see [`file_path`].
    pub fn file_path(&self, row: usize, save_path: &str) -> Option<PathBuf> {
        if let Some(t) = &self.tree {
            return file_path(save_path, &t.node_path(t.row_node(row)?));
        }
        let i = *self.file_rows.get(row)? as usize;
        file_path(save_path, &self.files.as_ref()?.get(i)?.path)
    }

    /// How many files the filter shows (all but pad files without one).
    pub fn shown_files(&self) -> usize {
        self.file_rows.len()
    }
}

/// "99.9 %": rounded down, so only a complete file reads 100.
fn percent_text(done: u64, size: u64) -> String {
    if size > 0 {
        format!("{:.1} %", (done as f64 / size as f64 * 1000.0).floor() / 10.0)
    } else {
        "100.0 %".to_owned()
    }
}

/// The Files tab's quick filter (D50): the box's words, each matched case-insensitively
/// anywhere in a file's path as the tab shows it (folders included), all of them needed, as
/// the toolbar's torrent filter matches a name (D35).
#[derive(Clone, Default, Debug)]
pub struct FileFilter {
    /// The box's text as given.
    text: String,
    /// Its words, lowercased.
    words: Vec<String>,
}

impl FileFilter {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the filter hides anything (the box has a word).
    pub fn is_on(&self) -> bool {
        !self.words.is_empty()
    }

    /// Sets the box's text; true when the words changed, so the rows must be rebuilt.
    pub fn set_text(&mut self, text: &str) -> bool {
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        self.text = text.to_owned();
        if words == self.words {
            return false;
        }
        self.words = words;
        true
    }

    /// Whether a file's path passes.
    pub fn matches(&self, path: &str) -> bool {
        self.words.iter().all(|w| crate::model::name_matches(path, w))
    }

    /// The visible rows of `files`: the indices of the files that are not padding and pass, in
    /// the torrent's order.
    pub fn rows(&self, files: &[FileEntry]) -> Vec<u32> {
        let pass = |f: &FileEntry| !f.pad_file && self.matches(&f.path);
        (0..files.len() as u32).filter(|&i| pass(&files[i as usize])).collect()
    }
}

/// `<save path>/<path>` for a file's `/`-separated path in its torrent, or None when the save
/// path is not absolute or the path is anything but plain names, so what Open runs can never
/// be outside the save path (as Open Folder's target, SEC-18).
pub fn file_path(save_path: &str, path: &str) -> Option<PathBuf> {
    use std::path::{Component, Path};
    let mut full = PathBuf::from(save_path);
    if !full.is_absolute() || path.is_empty() {
        return None;
    }
    for part in path.split('/') {
        let mut c = Path::new(part).components();
        match (c.next(), c.next()) {
            (Some(Component::Normal(n)), None) => full.push(n),
            _ => return None,
        }
    }
    Some(full)
}

impl Detail {
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
        3 => done.map(|d| percent_text(d, f.size)).unwrap_or_default(),
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

/// The priorities the Files tab's menu offers, in menu order. As µTorrent 2.2.1's, the menu
/// has the first three in a Priority submenu by their short names ([`priority_text`]) and
/// Don't Download after it.
pub const PRIORITIES: [(S, DownloadPriority); 4] = [
    (S::PrioHighItem, DownloadPriority::TOP),
    (S::PrioNormalItem, DownloadPriority::NORMAL),
    (S::PrioLowItem, DownloadPriority::LOW),
    (S::PrioDontDownload, DownloadPriority::DONT_DOWNLOAD),
];

/// "Prioritize by File Order" for `files` (indices into the torrent's file list): the first in
/// the torrent gets the top priority and each later one the same or a lower one, down to Low,
/// so the piece picker finishes them in that order. More than seven files share the levels in
/// runs.
pub fn by_file_order(files: &[u32]) -> Vec<(u32, DownloadPriority)> {
    let mut files = files.to_vec();
    files.sort_unstable();
    files.dedup();
    let (n, top) = (files.len(), DownloadPriority::TOP.0 as usize);
    files.into_iter().enumerate().map(|(i, f)| (f, DownloadPriority((top - i * top / n) as u8))).collect()
}

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
    fn prioritize_by_file_order() {
        let levels =
            |files: &[u32]| by_file_order(files).into_iter().map(|(f, p)| (f, p.0)).collect::<Vec<_>>();
        assert_eq!(levels(&[]), []);
        assert_eq!(levels(&[5]), [(5, 7)]);
        assert_eq!(levels(&[9, 2, 4, 2]), [(2, 7), (4, 5), (9, 3)], "in the torrent's order, once each");
        let seven: Vec<u8> = levels(&[0, 1, 2, 3, 4, 5, 6]).into_iter().map(|(_, p)| p).collect();
        assert_eq!(seven, [7, 6, 5, 4, 3, 2, 1]);
        let many = levels(&(0..100).collect::<Vec<_>>());
        assert!(many.windows(2).all(|w| w[0].1 >= w[1].1));
        assert_eq!((many[0].1, many[99].1), (7, 1), "never Don't Download");
        assert_eq!(priority_text(DownloadPriority(6)), "High");
    }

    fn file(path: &str, pad_file: bool) -> FileEntry {
        FileEntry {
            path: path.into(),
            size: 1,
            offset: 0,
            first_piece: 0,
            last_piece: 0,
            pad_file,
            priority: DownloadPriority::NORMAL,
        }
    }

    #[test]
    fn the_file_filter_matches_every_word_in_the_path() {
        let files: Arc<[FileEntry]> = Arc::from(vec![
            file("Show/Season 1/Show.S01E03.mkv", false),
            file("Show/.pad/0", true),
            file("Show/Subs/S01E03.ENG.srt", false),
            file("Show/Season 1/Show.S01E04.mkv", false),
            file("Show/Ünïcode Ä.txt", false),
        ]);
        let rows = |text: &str| {
            let mut f = FileFilter::default();
            f.set_text(text);
            f.rows(&files)
        };
        let none: [u32; 0] = [];
        assert_eq!(rows(""), [0, 2, 3, 4], "no filter: every file but the padding");
        assert_eq!(rows("   "), [0, 2, 3, 4]);
        assert_eq!(rows("s01e03"), [0, 2], "ignores case");
        assert_eq!(rows("subs/"), [2], "folders count");
        assert_eq!(rows("show mkv  e04"), [3], "every word must match, in any order");
        assert_eq!(rows("ä"), [4], "non-ASCII ignores case too");
        assert_eq!(rows("pad"), none, "padding never shows");
        assert_eq!(rows("nothing"), none);
        let mut f = FileFilter::default();
        assert!(!f.is_on() && !f.set_text(" "), "spaces are no filter");
        assert!(f.set_text("Show") && f.is_on() && f.text() == "Show");
        assert!(!f.set_text("show "), "the same words: nothing to rebuild");
    }

    #[test]
    fn the_files_tab_maps_rows_through_the_filter() {
        let files: Arc<[FileEntry]> =
            Arc::from(vec![file("a/one.mkv", false), file("pad", true), file("b/two.mkv", false)]);
        let mut d = Detail::default();
        d.set_files_tree(false);
        assert!(d.set_file_filter("TWO"), "a filter before any file list");
        d.set_files(files.clone());
        assert_eq!((d.rows(Tab::Files), d.file_index(0)), (1, Some(2)));
        assert_eq!(d.cell(Tab::Files, 0, 0), "b/two.mkv");
        assert_eq!(d.file_filter_count(), "1 of 2 files");
        assert_eq!(d.all_files(), [0, 2], "Prioritize by File Order takes them all");
        // the text stays with another torrent and applies to its files
        d.select(Some(TorrentId::from_raw(7)));
        assert_eq!((d.rows(Tab::Files), d.file_filter.text()), (0, "TWO"));
        d.set_files(files);
        assert_eq!(d.file_index(0), Some(2));
        assert!(d.set_file_filter(""));
        assert_eq!((d.rows(Tab::Files), d.file_index(1)), (2, Some(2)));
        assert_eq!(d.file_filter_count(), "", "no count without a filter");
        assert!(!d.set_file_filter(""));
    }

    fn entry(path: &str, size: u64, prio: u8, pad_file: bool) -> FileEntry {
        FileEntry {
            path: path.into(),
            size,
            offset: 0,
            first_piece: 0,
            last_piece: 0,
            pad_file,
            priority: DownloadPriority(prio),
        }
    }

    /// Show/Season 1/{e01,e02}.mkv, Show/Subs/e01.srt, a pad file, Show/info.nfo.
    fn show() -> Arc<[FileEntry]> {
        Arc::from(vec![
            entry("Show/Season 1/e01.mkv", 1000, 4, false),
            entry("Show/Season 1/e02.mkv", 3000, 7, false),
            entry("Show/.pad/12", 12, 0, true),
            entry("Show/Subs/e01.srt", 10, 0, false),
            entry("Show/info.nfo", 5, 2, false),
        ])
    }

    fn names(d: &Detail) -> Vec<String> {
        (0..d.rows(Tab::Files)).map(|r| d.cell(Tab::Files, r, 0)).collect()
    }

    #[test]
    fn the_files_tab_shows_a_folder_tree_by_default() {
        let mut d = Detail::default();
        assert!(d.files_tree());
        d.set_files(show());
        // the torrent's folder is left out, as in the Add dialog
        assert_eq!(names(&d), ["Season 1", "e01.mkv", "e02.mkv", "Subs", "e01.srt", "info.nfo"]);
        let season = d.file_tree_row(0).unwrap();
        assert_eq!((season.folder, season.depth), (Some(true), 0));
        assert_eq!(d.file_tree_row(1).unwrap().depth, 1);
        assert_eq!(d.cell(Tab::Files, 0, 1), fmt::size(4000));
        assert_eq!(d.cell(Tab::Files, 0, 2), "", "no progress yet");
        assert_eq!(d.cell(Tab::Files, 0, 4), "", "no piece columns for folders");
        assert_eq!(d.cell(Tab::Files, 0, 6), "Mixed", "Normal and High");
        assert_eq!(d.cell(Tab::Files, 3, 6), "Don't Download");
        assert_eq!(d.cell(Tab::Files, 5, 6), "Low");
        assert_eq!(d.cell(Tab::Files, 1, 4), "1", "files keep their columns");
        assert_eq!((d.file_index(0), d.file_index(2), d.file_index(5)), (None, Some(1), Some(4)));

        d.set_file_done(vec![500, 3000, 0, 10, 0]);
        assert_eq!(d.file_done(), [500, 3000, 0, 10, 0]);
        assert_eq!(d.cell(Tab::Files, 0, 2), fmt::size(3500));
        assert_eq!(d.cell(Tab::Files, 0, 3), "87.5 %");
        assert_eq!(d.cell(Tab::Files, 1, 3), "50.0 %");

        // actions on a folder take its files
        assert_eq!(d.file_indexes(&[0]), [0, 1]);
        assert_eq!(d.file_indexes(&[5, 0, 1]), [0, 1, 4], "ascending, once each");
        let all: Vec<usize> = (0..d.rows(Tab::Files)).collect();
        assert_eq!(d.file_indexes(&all), d.all_files());
        let save = if cfg!(windows) { r"C:\dl" } else { "/dl" };
        assert_eq!(d.file_path(0, save), Some(PathBuf::from(save).join("Show").join("Season 1")));
        assert_eq!(d.file_path(4, save), Some(PathBuf::from(save).join("Show").join("Subs").join("e01.srt")));
        assert_eq!(d.file_parent_row(4), Some(3));
    }

    #[test]
    fn the_tree_keeps_collapsed_folders_when_priorities_change() {
        let mut d = Detail::default();
        d.set_files(show());
        assert!(d.set_file_folder_expanded(0, Some(false)));
        assert_eq!(names(&d), ["Season 1", "Subs", "e01.srt", "info.nfo"]);
        // the engine reads the list back after Normal on the folder: same files, new priorities
        let mut again = show().to_vec();
        again[1].priority = DownloadPriority::NORMAL;
        d.set_files(again.clone().into());
        assert_eq!(names(&d), ["Season 1", "Subs", "e01.srt", "info.nfo"], "still collapsed");
        assert_eq!(d.cell(Tab::Files, 0, 6), "Normal");
        // levels that read alike are not mixed (Prioritize by File Order gives 7, 6, 5)
        again[0].priority = DownloadPriority(7);
        again[1].priority = DownloadPriority(6);
        d.set_files(again.clone().into());
        assert_eq!(d.cell(Tab::Files, 0, 6), "High");
        // a renamed file (Relocate) is another list: the tree starts again, all open
        again[3].path = "Show/Subs/e01.eng.srt".into();
        d.set_files(again.into());
        assert_eq!(names(&d)[..3], ["Season 1", "e01.mkv", "e02.mkv"]);
        assert_eq!(d.cell(Tab::Files, 4, 0), "e01.eng.srt");
        // another torrent starts again
        d.set_file_folder_expanded(0, Some(false));
        d.select(Some(TorrentId::from_raw(3)));
        assert_eq!(d.rows(Tab::Files), 0);
        d.set_files(show());
        assert_eq!(d.rows(Tab::Files), 6);
    }

    #[test]
    fn the_filter_and_the_toggle_in_the_tree() {
        let mut d = Detail::default();
        d.set_files(show());
        d.set_file_folder_expanded(3, Some(false));
        assert!(d.set_file_filter("e01"));
        assert_eq!(names(&d), ["Season 1", "e01.mkv", "Subs", "e01.srt"], "folders open while filtering");
        assert_eq!(d.file_filter_count(), "2 of 4 files");
        assert_eq!(d.file_indexes(&[0]), [0], "a folder stands for its shown files");
        // the torrent's folder is part of every path, as in the plain list
        d.set_file_filter("show nfo");
        assert_eq!(names(&d), ["info.nfo"]);
        d.set_file_filter("");
        assert_eq!(names(&d), ["Season 1", "e01.mkv", "e02.mkv", "Subs", "info.nfo"], "Subs stays collapsed");

        // the plain list of 2.2.1
        assert!(d.set_files_tree(false) && !d.set_files_tree(false));
        assert!(!d.files_tree() && d.file_tree_row(0).is_none());
        assert_eq!(
            names(&d),
            ["Show/Season 1/e01.mkv", "Show/Season 1/e02.mkv", "Show/Subs/e01.srt", "Show/info.nfo"]
        );
        assert_eq!(d.file_indexes(&[3, 0, 0]), [0, 4]);
        // the list comes back with the filter, the progress and every folder open
        d.set_file_done(vec![1000, 0, 0, 0, 0]);
        d.set_file_filter("mkv");
        assert!(d.set_files_tree(true));
        assert_eq!(names(&d), ["Season 1", "e01.mkv", "e02.mkv"]);
        assert_eq!(d.cell(Tab::Files, 1, 3), "100.0 %");
        // no tree is built in the plain list
        d.set_files_tree(false);
        d.set_files(show());
        assert!(d.file_tree().is_none());
    }

    #[test]
    fn single_file_and_flat_torrents_in_the_tree() {
        let mut d = Detail::default();
        d.set_files(Arc::from(vec![entry("ubuntu.iso", 7, 4, false)]));
        assert_eq!(names(&d), ["ubuntu.iso"]);
        let save = if cfg!(windows) { r"C:\dl" } else { "/dl" };
        assert_eq!(d.file_path(0, save), Some(PathBuf::from(save).join("ubuntu.iso")));
        // files under different top folders keep them all
        d.select(Some(TorrentId::from_raw(1)));
        d.set_files(Arc::from(vec![entry("a/x", 1, 4, false), entry("b/y", 1, 4, false)]));
        assert_eq!(names(&d), ["a", "x", "b", "y"]);
        assert_eq!(d.file_path(3, save), Some(PathBuf::from(save).join("b").join("y")));
    }

    /// Open stays inside the save path whatever the torrent's file paths are.
    #[test]
    fn file_paths_stay_in_the_save_path() {
        let save = if cfg!(windows) { r"C:\Downloads" } else { "/downloads" };
        let p = file_path(save, "Show/Season 1/e01.mkv").unwrap();
        assert_eq!(p, PathBuf::from(save).join("Show").join("Season 1").join("e01.mkv"));
        for bad in ["", "..", "a/../b", "a//b", "/x", "a/", "./a", r"\\host\share\x", r"C:\x", "C:x", r"a\b"]
        {
            // a backslash and a colon are plain name characters off Windows
            if cfg!(windows) || !bad.contains(['\\', ':']) {
                assert_eq!(file_path(save, bad), None, "{bad}");
            }
        }
        assert_eq!(file_path("relative", "a"), None);

        let mut d = Detail::default();
        let entry = |path: &str, pad_file| FileEntry {
            path: path.into(),
            size: 1,
            offset: 0,
            first_piece: 0,
            last_piece: 0,
            pad_file,
            priority: DownloadPriority::NORMAL,
        };
        d.set_files(vec![entry("T/.pad/1", true), entry("T/a.txt", false)].into());
        assert_eq!(d.file_path(0, save), Some(PathBuf::from(save).join("T").join("a.txt")));
        assert_eq!(d.file_path(1, save), None);
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
