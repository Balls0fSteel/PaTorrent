// Ported from libtorrent-rasterbar 2.0.11: src/torrent.cpp (state, file priorities, progress,
// status, resume data), src/torrent_status.cpp, src/file_progress.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: plain data plus helpers; the engine drives transitions. Metadata, the piece picker
// and storage exist only while the torrent runs (PLAN.md D5); a stopped torrent keeps its piece
// bitfield and the summary fields needed for listing.

//! One torrent's state.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use super::peer_list::PeerList;
use super::picker::PiecePicker;
use super::smart_ban::SmartBan;
use super::tracker::TrackerEntry;
use super::web_seed::WebSeeds;
use crate::disk::{BLOCK_SIZE, Storage};
use crate::metainfo::file_storage::FileFlags;
use crate::metainfo::merkle_tree::MerkleTree;
use crate::metainfo::{TorrentInfo, WebSeedKind};
use crate::net::Bucket;
use crate::params::{DownloadPriority, TorrentFlags};
use crate::proto::ut_metadata::MetadataFetch;
use crate::proto::ut_pex::TorrentPex;
use crate::resume::ResumeData;
use crate::status::{FileEntry, TorrentState, TorrentStatus};
use crate::types::{InfoHashes, TorrentId};
use crate::util::bitfield::Bitfield;
use crate::util::stat::Stat;

/// Where a running torrent announces (libtorrent `m_announce_to_dht`, `m_announce_to_trackers`,
/// `m_announce_to_lsd`): everywhere, unless the auto-manager's `active_dht_limit`,
/// `active_tracker_limit` or `active_lsd_limit` left an auto-managed torrent out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct AnnounceTo {
    pub dht: bool,
    pub trackers: bool,
    pub lsd: bool,
}

impl AnnounceTo {
    pub const ALL: AnnounceTo = AnnounceTo { dht: true, trackers: true, lsd: true };
}

/// Strings many torrents hold the same of (save paths, tracker URLs), shared as `Arc<str>`s: a
/// library of 100,000 torrents has a handful of download folders and trackers, not 100,000
/// copies of them. Strings no torrent holds any more are dropped when the pool has doubled
/// since the last sweep.
#[derive(Default)]
pub(crate) struct StrPool {
    set: HashSet<Arc<str>>,
    sweep_at: usize,
}

impl StrPool {
    pub fn intern(&mut self, s: &str) -> Arc<str> {
        if let Some(p) = self.set.get(s) {
            return p.clone();
        }
        if self.set.len() >= self.sweep_at.max(64) {
            self.set.retain(|p| Arc::strong_count(p) > 1);
            self.sweep_at = self.set.len() * 2;
        }
        let p: Arc<str> = s.into();
        self.set.insert(p.clone());
        p
    }
}

/// Every torrent of the session has one of these, stopped or not, so it is kept compact (PLAN.md
/// D21: 100,000 stopped torrents): the parts only a running torrent uses (the piece picker, the
/// metadata and piece-layer fetches, the PEX and smart-ban state) are boxed and absent until
/// needed, as libtorrent holds `m_picker` and `m_peer_list` behind pointers made on demand.
/// The engine keeps each `Torrent` in a box of its own, so its slot table stays small too.
pub(crate) struct Torrent {
    pub id: TorrentId,
    pub info_hashes: InfoHashes,
    pub name: String,
    /// Shared with the other torrents saved in the same place (`Engine::intern_save_path`).
    pub save_path: Arc<str>,
    /// Loaded while running (or when requested); `None` for stopped torrents.
    pub ti: Option<Arc<TorrentInfo>>,
    /// v2: one merkle tree per file with the root and piece layer, built from `ti` when a peer
    /// first asks for hashes; dropped with `ti`.
    pub merkle_trees: Option<Vec<MerkleTree>>,
    /// v2: the piece layers being fetched from peers, for a v2 torrent whose metadata came
    /// without them (a magnet); the torrent starts once they are complete.
    pub layer_fetch: Option<Box<super::v2::LayerFetch>>,
    pub total_size: u64,
    pub num_pieces: u32,
    pub piece_length: u32,
    pub flags: TorrentFlags,
    pub state: TorrentState,
    pub error: Option<String>,

    pub storage: Option<Arc<Storage>>,
    /// The storage `move_storage` runs on while moves are pending (libtorrent
    /// `m_moving_storage`), and whether it was made for the move because the torrent had none
    /// (it is released when the last move is done).
    pub moving: Option<(Arc<Storage>, bool)>,
    pub moves_pending: u32,
    /// `rename_file` calls on the disk thread; a stopped torrent's renames use `moving` too.
    pub renames_pending: u32,
    /// Files stored under another name (libtorrent `renamed_files` / `mapped_files`), applied to
    /// `ti` whenever it is loaded.
    pub renamed_files: BTreeMap<u32, String>,
    /// Running without metadata (a magnet): the info dictionary being fetched from peers.
    pub fetch: Option<Box<MetadataFetch>>,
    /// BEP 11: the peer diff sent to every peer once a minute; made at the first tick with
    /// peers.
    pub pex: Option<Box<TorrentPex>>,
    /// Block hashes of failed pieces per peer, for banning the senders of bad blocks; made at
    /// the first failed piece.
    pub smart_ban: Option<Box<SmartBan>>,
    /// The name came from the user; metadata does not replace it.
    pub user_named: bool,
    pub picker: Option<Box<PiecePicker>>,
    /// Pieces we have while there is no picker (stopped, or before the check finished). Set it
    /// through `set_have`, which keeps a complete set as `have_all` instead.
    pub have: Option<Bitfield>,
    /// Every piece, held without a bitfield (libtorrent `m_have_all`): a stopped seed, the
    /// usual torrent of a large library, needs no bit per piece. `have` is `None` then.
    pub have_all: bool,
    /// Partial pieces while there is no picker: (piece, finished blocks), from resume data or
    /// saved when the torrent stopped. Restored into the picker once resume data is verified.
    pub unfinished: Vec<(u32, Bitfield)>,
    /// True once `have` was verified this session (by a check or by resume verification).
    pub checked: bool,
    pub checking: bool,
    pub checked_pieces: u32,
    pub file_priority: Vec<u8>,

    pub peers: Vec<usize>,
    pub peer_list: PeerList,
    pub trackers: Vec<TrackerEntry>,
    /// A hybrid's trackers once more, for announcing under its truncated v2 hash with their own
    /// state (libtorrent's second `announce_infohash` per endpoint); kept in step with
    /// `trackers` by `sync_v2_trackers`, not shown to frontends.
    pub trackers_v2: Vec<TrackerEntry>,
    /// BEP 19 / BEP 17 web seeds.
    pub web_seeds: WebSeeds,

    /// Traffic this session; made at the first tick or transfer, so a torrent that never ran
    /// holds none (`stat()` reads zeros then).
    pub stat: Option<Box<Stat>>,
    /// All-time totals from earlier sessions.
    pub prev_uploaded: u64,
    pub prev_downloaded: u64,
    pub failed_bytes: u64,
    pub redundant_bytes: u64,
    pub added_time: i64,
    pub completed_time: i64,
    pub active_secs: f64,
    pub finished_secs: f64,
    pub seeding_secs: f64,
    pub queue_position: Option<u32>,
    pub download_limit: Bucket,
    pub upload_limit: Bucket,
    pub max_connections: Option<u32>,
    pub max_uploads: Option<u32>,
    pub num_complete: Option<u32>,
    pub num_incomplete: Option<u32>,

    /// What the auto-manager last allowed (`announces`).
    pub announce: AnnounceTo,

    pub status_dirty: bool,
    pub resume_dirty: bool,
    /// Our piece set changed: recheck interest in every peer on the next tick.
    pub interest_dirty: bool,
    /// Downloaded payload in this session (decides the tracker `completed` event).
    pub downloaded_in_session: bool,
    pub started_at: Option<Instant>,
}

impl Torrent {
    pub fn new(
        id: TorrentId,
        info_hashes: InfoHashes,
        name: String,
        save_path: Arc<str>,
        flags: TorrentFlags,
    ) -> Torrent {
        Torrent {
            id,
            info_hashes,
            name,
            save_path,
            ti: None,
            merkle_trees: None,
            layer_fetch: None,
            total_size: 0,
            num_pieces: 0,
            piece_length: 0,
            flags,
            state: TorrentState::CheckingResumeData,
            error: None,
            storage: None,
            moving: None,
            moves_pending: 0,
            renames_pending: 0,
            renamed_files: BTreeMap::new(),
            fetch: None,
            pex: None,
            smart_ban: None,
            user_named: false,
            picker: None,
            have: None,
            have_all: false,
            unfinished: Vec::new(),
            checked: false,
            checking: false,
            checked_pieces: 0,
            file_priority: Vec::new(),
            peers: Vec::new(),
            peer_list: PeerList::default(),
            trackers: Vec::new(),
            trackers_v2: Vec::new(),
            web_seeds: WebSeeds::default(),
            stat: None,
            prev_uploaded: 0,
            prev_downloaded: 0,
            failed_bytes: 0,
            redundant_bytes: 0,
            added_time: crate::util::stat::now_unix(),
            completed_time: 0,
            active_secs: 0.0,
            finished_secs: 0.0,
            seeding_secs: 0.0,
            queue_position: None,
            download_limit: Bucket::new(0),
            upload_limit: Bucket::new(0),
            max_connections: None,
            max_uploads: None,
            num_complete: None,
            num_incomplete: None,
            announce: AnnounceTo::ALL,
            status_dirty: true,
            resume_dirty: true,
            interest_dirty: false,
            downloaded_in_session: false,
            started_at: None,
        }
    }

    /// Store key: the info-hash in hex. Made when the store is used rather than kept, since
    /// every torrent would hold its 40 or 64 bytes for the few calls that need it.
    pub fn key(&self) -> String {
        self.info_hashes.key_hex()
    }

    /// This session's traffic (zeros for a torrent that has not run).
    pub fn stat(&self) -> Stat {
        self.stat.as_deref().copied().unwrap_or_default()
    }

    pub fn set_metadata(&mut self, ti: Arc<TorrentInfo>) {
        self.total_size = ti.total_size();
        self.num_pieces = ti.num_pieces();
        self.piece_length = ti.piece_length();
        if self.name.is_empty() {
            self.name = ti.name().to_owned();
        }
        let n = ti.num_files();
        if self.file_priority.len() != n {
            self.file_priority.resize(n, DownloadPriority::NORMAL.0);
        }
        self.ti = Some(ti);
        self.merkle_trees = None;
        self.apply_renames();
    }

    /// Gives `ti` the names in `renamed_files` (libtorrent `torrent::init`, "MAPPED FILES").
    /// Indices past the last file are ignored.
    pub fn apply_renames(&mut self) {
        let Some(ti) = &mut self.ti else { return };
        let n = ti.num_files();
        let fs = ti.files();
        let todo: Vec<_> = self
            .renamed_files
            .iter()
            .filter(|&(&i, name)| (i as usize) < n && fs.file_path(i as usize, "") != *name)
            .collect();
        if todo.is_empty() {
            return;
        }
        let ti = Arc::make_mut(ti);
        for (&i, name) in todo {
            ti.rename_file(i as usize, name);
        }
    }

    /// File `file` is now stored as `name`: records it (dropping the entry when it is the
    /// metadata's own name again) and renames it in `ti`.
    pub fn file_renamed(&mut self, file: u32, name: &str) {
        let Some(ti) = &mut self.ti else { return };
        if ti.orig_files().file_path(file as usize, "") == name {
            self.renamed_files.remove(&file);
        } else {
            self.renamed_files.insert(file, name.to_owned());
        }
        Arc::make_mut(ti).rename_file(file as usize, name);
    }

    /// Makes `trackers_v2` list the same trackers as `trackers` (a hybrid) or none, keeping the
    /// announce state of the ones it had.
    pub fn sync_v2_trackers(&mut self) {
        let hybrid = self.info_hashes.has_v1() && self.info_hashes.has_v2();
        if !hybrid {
            self.trackers_v2.clear();
            return;
        }
        let same = self.trackers_v2.len() == self.trackers.len()
            && self.trackers_v2.iter().zip(&self.trackers).all(|(a, b)| a.url == b.url && a.tier == b.tier);
        if same {
            return;
        }
        let mut old = std::mem::take(&mut self.trackers_v2);
        self.trackers_v2 = self
            .trackers
            .iter()
            .map(|t| match old.iter().position(|o| o.url == t.url) {
                Some(i) => {
                    let mut e = old.swap_remove(i);
                    e.tier = t.tier;
                    e
                }
                None => TrackerEntry::new(t.url.clone(), t.tier),
            })
            .collect();
    }

    /// Adds a tracker, keeping the list sorted by tier and free of duplicates; the URL comes
    /// from `urls`, shared with the other torrents. The list grows one entry at a time: a
    /// doubling `Vec` would hold four entries for the usual two trackers, in every stopped
    /// torrent.
    pub fn add_tracker(&mut self, url: &str, tier: u32, urls: &mut StrPool) {
        let url = url.trim();
        if url.is_empty() || self.trackers.iter().any(|t| &*t.url == url) {
            return;
        }
        let pos = self.trackers.iter().position(|t| t.tier > tier).unwrap_or(self.trackers.len());
        self.trackers.reserve_exact(1);
        self.trackers.insert(pos, TrackerEntry::new(urls.intern(url), tier));
    }

    /// libtorrent `replace_trackers`: the new list, ordered by tier (stably), without empty
    /// URLs and repeats. A URL that was in the list keeps its entry's state (next announce,
    /// `started` sent, scrape), as libtorrent keeps what a client passes back from `trackers()`;
    /// new ones are announced to on the next tick.
    pub fn replace_trackers(&mut self, list: &[(String, u32)], urls: &mut StrPool) {
        let mut old = std::mem::take(&mut self.trackers);
        let mut sorted: Vec<&(String, u32)> = list.iter().collect();
        sorted.sort_by_key(|(_, tier)| *tier);
        for (url, tier) in sorted {
            let url = url.trim();
            if url.is_empty() || self.trackers.iter().any(|t| &*t.url == url) {
                continue;
            }
            let entry = match old.iter().position(|t| &*t.url == url) {
                Some(i) => {
                    let mut e = old.swap_remove(i);
                    e.tier = *tier;
                    e
                }
                None => TrackerEntry::new(urls.intern(url), *tier),
            };
            self.trackers.push(entry);
        }
        self.trackers.shrink_to_fit();
    }

    /// The info dictionary is known (it may still be in the store rather than loaded).
    pub fn has_metadata(&self) -> bool {
        self.num_pieces > 0
    }

    /// Started: checking or transferring, or fetching metadata.
    pub fn is_active(&self) -> bool {
        self.storage.is_some() || self.fetch.is_some() || self.layer_fetch.is_some()
    }

    pub fn is_paused(&self) -> bool {
        self.flags.contains(TorrentFlags::PAUSED)
    }

    /// The pieces we have: the picker's while there is one, else the stored set; `None` when
    /// unknown (the files must be checked).
    pub fn have_bits(&self) -> Option<HaveRef<'_>> {
        match (&self.picker, &self.have) {
            (Some(p), _) => Some(HaveRef::Bits(p.have())),
            (None, Some(h)) => Some(HaveRef::Bits(h)),
            (None, None) if self.have_all => Some(HaveRef::All(self.num_pieces as usize)),
            (None, None) => None,
        }
    }

    /// Stores the pieces we have while there is no picker; a complete set is kept as
    /// `have_all` (libtorrent drops a seed's bitfield the same way).
    pub fn set_have(&mut self, have: Option<Bitfield>) {
        let n = self.num_pieces as usize;
        self.have_all = have.as_ref().is_some_and(|h| n > 0 && h.len() == n && h.all_set());
        self.have = have.filter(|_| !self.have_all);
    }

    /// Takes the stored piece set (for the picker), as a bitfield even when it was `have_all`.
    pub fn take_have(&mut self) -> Option<Bitfield> {
        let all = std::mem::take(&mut self.have_all);
        self.have.take().or_else(|| all.then(|| Bitfield::new(self.num_pieces as usize, true)))
    }

    pub fn num_have(&self) -> u32 {
        self.have_bits().map_or(0, |b| b.count() as u32)
    }

    pub fn is_seed(&self) -> bool {
        self.num_pieces > 0 && self.num_have() == self.num_pieces
    }

    /// BEP 16 super seeding is in effect: the flag is set and we are a checked seed
    /// (libtorrent keeps `m_super_seeding` only for seeds, see `files_checked`).
    pub fn super_seeding(&self) -> bool {
        self.flags.contains(TorrentFlags::SUPER_SEEDING) && !self.checking && self.is_seed()
    }

    /// Every wanted piece is done (libtorrent `is_finished`).
    pub fn is_finished(&self) -> bool {
        match (&self.picker, self.have_bits(), &self.ti) {
            (Some(p), _, _) => p.is_finished(),
            (None, Some(have), Some(ti)) => {
                let prio = piece_priorities(ti, &self.file_priority);
                (0..have.len()).all(|i| prio[i] == 0 || have.get(i))
            }
            (None, Some(have), None) => {
                // Without metadata we can only tell for "download everything".
                have.all_set()
                    || (self.file_priority.iter().all(|&p| p == 0) && !self.file_priority.is_empty())
            }
            (None, None, _) => false,
        }
    }

    pub fn piece_size(&self, piece: u32) -> u64 {
        if piece + 1 == self.num_pieces {
            self.total_size - self.piece_length as u64 * (self.num_pieces as u64 - 1)
        } else {
            self.piece_length as u64
        }
    }

    /// Bytes on disk and verified, plus finished blocks of partial pieces.
    pub fn total_done(&self) -> u64 {
        let Some(have) = self.have_bits() else { return 0 };
        let mut done = have.count() as u64 * self.piece_length as u64;
        if self.num_pieces > 0 && have.get(self.num_pieces as usize - 1) {
            done = done - self.piece_length as u64 + self.piece_size(self.num_pieces - 1);
        }
        match &self.picker {
            Some(p) => done += p.partial_finished_blocks() as u64 * BLOCK_SIZE as u64,
            None => {
                let blocks: usize = self.unfinished.iter().map(|(_, b)| b.count()).sum();
                done += blocks as u64 * BLOCK_SIZE as u64;
            }
        }
        done.min(self.total_size)
    }

    /// (wanted bytes, wanted bytes done), at piece granularity.
    pub fn wanted(&self) -> (u64, u64) {
        let Some(ti) = &self.ti else {
            let done = self.total_done();
            return (self.total_size, done);
        };
        let prio = match &self.picker {
            Some(p) => (0..self.num_pieces).map(|i| p.piece_priority(i)).collect(),
            None => piece_priorities(ti, &self.file_priority),
        };
        let have = self.have_bits();
        let mut wanted = 0;
        let mut done = 0;
        for i in 0..self.num_pieces {
            if prio[i as usize] > 0 {
                let size = self.piece_size(i);
                wanted += size;
                if have.is_some_and(|h| h.get(i as usize)) {
                    done += size;
                }
            }
        }
        (wanted, done)
    }

    /// The bytes we still need, for tracker `left`. Without metadata the size is unknown;
    /// libtorrent then reports 16 KiB, so trackers don't take us for a seed.
    pub fn bytes_left(&self) -> u64 {
        if !self.has_metadata() {
            return 16 * 1024;
        }
        self.total_size.saturating_sub(self.total_done())
    }

    pub fn settled_state(&self) -> TorrentState {
        if !self.has_metadata() {
            TorrentState::DownloadingMetadata
        } else if self.is_seed() {
            TorrentState::Seeding
        } else if self.is_finished() {
            TorrentState::Finished
        } else {
            TorrentState::Downloading
        }
    }

    /// Where it announces while running: everywhere when not auto-managed (forced), else what
    /// the auto-manager allowed.
    pub fn announces(&self) -> AnnounceTo {
        if self.flags.contains(TorrentFlags::AUTO_MANAGED) { self.announce } else { AnnounceTo::ALL }
    }

    pub fn status(
        &self,
        num_peers: u32,
        num_seeds: u32,
        rates: (u32, u32, u32, u32),
        now: Instant,
    ) -> TorrentStatus {
        let (wanted, wanted_done) = self.wanted();
        let total_done = self.total_done();
        let finished = self.is_finished();
        let progress_ppm = if let Some(f) = &self.fetch {
            // libtorrent reports the metadata download as progress in this state.
            let (have, total) = f.progress();
            ppm(have as u64, total as u64)
        } else if self.checking || self.state == TorrentState::CheckingFiles {
            ppm(self.checked_pieces as u64, self.num_pieces as u64)
        } else if finished {
            1_000_000
        } else {
            ppm(wanted_done, wanted)
        };
        let next_announce = self
            .trackers
            .iter()
            .filter_map(TrackerEntry::next_announce)
            .min()
            .map(|t| t.saturating_duration_since(now).as_secs() as u32);
        TorrentStatus {
            id: self.id,
            info_hashes: self.info_hashes,
            name: self.name.clone(),
            save_path: self.save_path.to_string(),
            state: self.state,
            flags: self.flags,
            error: self.error.clone().unwrap_or_default(),
            has_metadata: self.has_metadata(),
            is_finished: finished,
            is_seeding: self.is_seed(),
            super_seeding: self.super_seeding(),
            moving_storage: self.moves_pending > 0,
            progress_ppm,
            queue_position: self.queue_position,
            download_rate: rates.0,
            upload_rate: rates.1,
            download_payload_rate: rates.2,
            upload_payload_rate: rates.3,
            num_seeds,
            num_peers,
            list_seeds: self.peer_list.num_seeds() as u32,
            list_peers: self.peer_list.len() as u32,
            num_complete: self.num_complete,
            num_incomplete: self.num_incomplete,
            num_connections: num_peers,
            distributed_copies: self.picker.as_ref().map(|p| p.distributed_copies()),
            total_download: self.stat().total_download(),
            total_upload: self.stat().total_upload(),
            total_payload_download: self.stat().download_payload.total,
            total_payload_upload: self.stat().upload_payload.total,
            total_failed_bytes: self.failed_bytes,
            total_redundant_bytes: self.redundant_bytes,
            all_time_download: self.prev_downloaded + self.stat().download_payload.total,
            all_time_upload: self.prev_uploaded + self.stat().upload_payload.total,
            total: self.total_size,
            total_done,
            total_wanted: wanted,
            total_wanted_done: wanted_done,
            num_pieces: self.num_have(),
            pieces_total: self.num_pieces,
            piece_length: self.piece_length,
            added_time: self.added_time,
            completed_time: self.completed_time,
            last_seen_complete: 0,
            active_duration: self.active_secs as u64,
            finished_duration: self.finished_secs as u64,
            seeding_duration: self.seeding_secs as u64,
            current_tracker: self
                .trackers
                .iter()
                .find(|t| t.working())
                .map(|t| t.url.to_string())
                .unwrap_or_default(),
            next_announce,
            num_trackers: self.trackers.len() as u32,
            announce_interval: self.trackers.iter().filter_map(TrackerEntry::interval).max(),
            announcing_to_trackers: !self.is_paused() && self.announces().trackers,
            announcing_to_lsd: !self.is_paused() && self.announces().lsd,
            announcing_to_dht: !self.is_paused() && self.announces().dht,
            download_limit: self.download_limit.limit,
            upload_limit: self.upload_limit.limit,
            connections_limit: self.max_connections,
            uploads_limit: self.max_uploads,
        }
    }

    pub fn resume_data(&self) -> ResumeData {
        let mut trackers: Vec<Vec<String>> = Vec::new();
        for t in &self.trackers {
            match trackers.last_mut() {
                Some(tier)
                    if self
                        .trackers
                        .iter()
                        .find(|x| *x.url == *tier[0])
                        .is_some_and(|x| x.tier == t.tier) =>
                {
                    tier.push(t.url.to_string())
                }
                _ => trackers.push(vec![t.url.to_string()]),
            }
        }
        ResumeData {
            info_hashes: self.info_hashes,
            name: self.name.clone(),
            save_path: self.save_path.to_string(),
            // Only verified pieces are written, so a crash never makes us trust unchecked data.
            pieces: if self.checked || self.picker.is_none() {
                self.have_bits().map(HaveRef::to_bitfield)
            } else {
                None
            },
            unfinished: match &self.picker {
                Some(p) if self.checked => p.unfinished_pieces(),
                Some(_) => Vec::new(),
                None if self.have.is_some() || self.have_all => self.unfinished.clone(),
                None => Vec::new(),
            },
            total_uploaded: self.prev_uploaded + self.stat().upload_payload.total,
            total_downloaded: self.prev_downloaded + self.stat().download_payload.total,
            active_time: self.active_secs as u64,
            finished_time: self.finished_secs as u64,
            seeding_time: self.seeding_secs as u64,
            added_time: self.added_time,
            completed_time: self.completed_time,
            paused: self.is_paused(),
            auto_managed: self.flags.contains(TorrentFlags::AUTO_MANAGED),
            sequential_download: self.flags.contains(TorrentFlags::SEQUENTIAL_DOWNLOAD),
            super_seeding: self.flags.contains(TorrentFlags::SUPER_SEEDING),
            disable_dht: self.flags.contains(TorrentFlags::DISABLE_DHT),
            disable_lsd: self.flags.contains(TorrentFlags::DISABLE_LSD),
            disable_pex: self.flags.contains(TorrentFlags::DISABLE_PEX),
            file_priority: self.file_priority.clone(),
            mapped_files: self.renamed_files.clone(),
            trackers,
            peers: self.peer_list.resume_peers(100),
            url_seeds: self.web_seeds.resume_urls(WebSeedKind::UrlSeed),
            http_seeds: self.web_seeds.resume_urls(WebSeedKind::HttpSeed),
            upload_rate_limit: self.upload_limit.limit,
            download_rate_limit: self.download_limit.limit,
            max_connections: self.max_connections,
            max_uploads: self.max_uploads,
            queue_position: self.queue_position,
            total_size: self.total_size,
            num_pieces: self.num_pieces,
            piece_length: self.piece_length,
            info: None,
        }
    }

    pub fn file_entries(&self) -> Vec<FileEntry> {
        let Some(ti) = &self.ti else { return Vec::new() };
        let fs = ti.files();
        (0..fs.num_files())
            .map(|f| {
                let (first, end) = crate::metainfo::file_storage::file_piece_range_inclusive(fs, f);
                let last = end.saturating_sub(1).max(first);
                FileEntry {
                    path: fs.file_path(f, ""),
                    size: fs.file_size(f),
                    offset: fs.file_offset(f),
                    first_piece: first,
                    last_piece: last,
                    pad_file: fs.pad_file_at(f),
                    priority: DownloadPriority(self.file_priority.get(f).copied().unwrap_or(4)),
                }
            })
            .collect()
    }

    /// Completed bytes per file, at piece granularity (libtorrent `file_progress` with
    /// `piece_granularity`).
    pub fn file_progress(&self) -> Vec<u64> {
        let (Some(ti), Some(have)) = (&self.ti, self.have_bits()) else { return Vec::new() };
        let fs = ti.files();
        let mut out = vec![0u64; fs.num_files()];
        for p in (0..have.len()).filter(|&p| have.get(p)) {
            for s in fs.map_block(p as u32, 0, ti.piece_size(p as u32) as u64) {
                out[s.file_index] += s.size;
            }
        }
        out
    }
}

/// A torrent's piece set as `Torrent::have_bits` finds it: a bitfield, or every one of `len`
/// pieces (`Torrent::have_all`).
#[derive(Clone, Copy)]
pub(crate) enum HaveRef<'a> {
    Bits(&'a Bitfield),
    All(usize),
}

impl HaveRef<'_> {
    pub fn len(self) -> usize {
        match self {
            HaveRef::Bits(b) => b.len(),
            HaveRef::All(n) => n,
        }
    }

    pub fn get(self, i: usize) -> bool {
        match self {
            HaveRef::Bits(b) => b.get(i),
            HaveRef::All(n) => i < n,
        }
    }

    pub fn count(self) -> usize {
        match self {
            HaveRef::Bits(b) => b.count(),
            HaveRef::All(n) => n,
        }
    }

    pub fn all_set(self) -> bool {
        match self {
            HaveRef::Bits(b) => b.all_set(),
            HaveRef::All(_) => true,
        }
    }

    pub fn to_bitfield(self) -> Bitfield {
        match self {
            HaveRef::Bits(b) => b.clone(),
            HaveRef::All(n) => Bitfield::new(n, true),
        }
    }
}

/// libtorrent `torrent::update_piece_priorities`: a piece gets the highest priority of the
/// files it overlaps (pad files don't count).
pub(crate) fn piece_priorities(ti: &TorrentInfo, file_priority: &[u8]) -> Vec<u8> {
    let fs = ti.files();
    let mut prio = vec![0u8; ti.num_pieces() as usize];
    for f in 0..fs.num_files() {
        if fs.file_size(f) == 0 || fs.file_flags(f).contains(FileFlags::PAD_FILE) {
            continue;
        }
        let p = file_priority.get(f).copied().unwrap_or(4);
        if p == 0 {
            continue;
        }
        // libtorrent's "inclusive" range includes partially covered pieces; `end` is exclusive.
        let (first, end) = crate::metainfo::file_storage::file_piece_range_inclusive(fs, f);
        for piece in first..end.min(ti.num_pieces()) {
            prio[piece as usize] = prio[piece as usize].max(p);
        }
    }
    prio
}

fn ppm(done: u64, total: u64) -> u32 {
    if total == 0 { 0 } else { (done as u128 * 1_000_000 / total as u128).min(1_000_000) as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::tests::make_torrent;

    #[test]
    fn priorities_progress_and_resume() {
        // Pieces of 16 KiB: file a covers pieces 0-1, b covers 1-3, c covers 3.
        let (bytes, _) = make_torrent(&[("a", 20_000), ("b", 30_000), ("c", 10_000)], 16 * 1024);
        let ti = Arc::new(TorrentInfo::from_bytes(&bytes).unwrap());
        assert_eq!(ti.num_pieces(), 4);
        assert_eq!(piece_priorities(&ti, &[4, 0, 0]), [4, 4, 0, 0]);
        assert_eq!(piece_priorities(&ti, &[0, 0, 7]), [0, 0, 0, 7]);
        assert_eq!(piece_priorities(&ti, &[1, 7, 0]), [1, 7, 7, 7]);

        let mut t = Torrent::new(
            TorrentId::default(),
            ti.info_hashes(),
            String::new(),
            "/x".into(),
            TorrentFlags::DEFAULT,
        );
        t.set_metadata(ti.clone());
        assert_eq!(t.name, "test");
        assert_eq!(t.file_priority, [4, 4, 4]);
        let mut have = Bitfield::new(4, false);
        have.set(0, true);
        have.set(3, true);
        t.have = Some(have);
        assert_eq!(t.total_done(), 16384 + (60_000 - 3 * 16384));
        assert!(!t.is_finished());
        t.file_priority = vec![4, 0, 0];
        assert!(!t.is_finished(), "piece 1 is still wanted");
        t.file_priority = vec![0, 0, 4];
        assert!(t.is_finished());
        assert_eq!(t.settled_state(), TorrentState::Finished);
        // Piece 3 (49152..60000) holds the last 848 bytes of b and all of c.
        assert_eq!(t.file_progress(), [16384, 848, 10_000]);

        let urls = &mut StrPool::default();
        t.add_tracker("udp://b", 1, urls);
        t.add_tracker("udp://a", 0, urls);
        t.add_tracker("udp://c", 1, urls);
        t.add_tracker("udp://a", 5, urls);
        let r = t.resume_data();
        assert_eq!(
            r.trackers,
            [vec!["udp://a".to_owned()], vec!["udp://b".to_owned(), "udp://c".to_owned()]]
        );
        assert_eq!(r.pieces.unwrap().count(), 2);

        // replace_trackers: sorted by tier, repeats and blanks dropped, kept entries keep state
        t.trackers[1].run_mut().start_sent = true;
        t.trackers[1].run_mut().message = "kept".into();
        t.replace_trackers(
            &[
                ("udp://new".into(), 2),
                (" udp://b ".into(), 0),
                ("".into(), 0),
                ("udp://d".into(), 1),
                ("udp://new".into(), 0),
            ],
            urls,
        );
        let list: Vec<(&str, u32)> = t.trackers.iter().map(|e| (&*e.url, e.tier)).collect();
        // (a repeated URL keeps its lowest tier)
        assert_eq!(list, [("udp://b", 0), ("udp://new", 0), ("udp://d", 1)]);
        assert!(t.trackers[0].start_sent() && t.trackers[0].message() == "kept");
        assert!(!t.trackers[1].start_sent());
        // the URLs are shared through the pool
        assert!(Arc::ptr_eq(&t.trackers[1].url, &urls.intern("udp://new")));
        assert_eq!(
            t.resume_data().trackers,
            [vec!["udp://b".to_owned(), "udp://new".to_owned()], vec!["udp://d".to_owned()]]
        );
        // the status counts the trackers and carries the longest announce interval known
        let st = t.status(0, 0, (0, 0, 0, 0), Instant::now());
        assert_eq!((st.num_trackers, st.announce_interval), (3, None));
        let now = Instant::now();
        t.trackers[0].announced(None, None, 1800, None, 0, now);
        t.trackers[2].announced(None, None, 2700, None, 0, now);
        let st = t.status(0, 0, (0, 0, 0, 0), Instant::now());
        assert_eq!((st.num_trackers, st.announce_interval), (3, Some(2700)));
        t.replace_trackers(&[], urls);
        assert!(t.trackers.is_empty());
        let st = t.status(0, 0, (0, 0, 0, 0), Instant::now());
        assert_eq!((st.num_trackers, st.announce_interval), (0, None));

        // the per-torrent switches and limits are saved
        t.flags |= TorrentFlags::DISABLE_DHT | TorrentFlags::DISABLE_LSD;
        t.max_uploads = Some(3);
        let r = t.resume_data();
        assert!(r.disable_dht && r.disable_lsd && !r.disable_pex);
        assert_eq!(r.max_uploads, Some(3));
        let st = t.status(0, 0, (0, 0, 0, 0), Instant::now());
        assert_eq!((st.uploads_limit, st.connections_limit), (Some(3), None));
    }

    /// A stopped torrent keeps none of the running parts (PLAN.md D21: 100,000 of them): no
    /// picker, fetches, PEX or smart-ban state, traffic counters or peer list until it runs.
    #[test]
    fn a_stopped_torrent_stays_compact() {
        let size = std::mem::size_of::<Torrent>();
        assert!(size <= 664, "Torrent grew to {size} bytes: box what a stopped torrent does not use");
        let mut t = Torrent::new(
            TorrentId::default(),
            InfoHashes::v1(crate::types::Sha1Hash([1; 20])),
            "a".into(),
            "/x".into(),
            TorrentFlags::DEFAULT,
        );
        assert_eq!(t.key(), "01".repeat(20));
        assert!(t.picker.is_none() && t.fetch.is_none() && t.layer_fetch.is_none());
        assert!(t.pex.is_none() && t.smart_ban.is_none() && t.stat.is_none());
        assert_eq!(t.peer_list.len(), 0);
        let st = t.status(0, 0, (0, 0, 0, 0), Instant::now());
        assert_eq!((st.total_download, st.all_time_upload, st.download_rate), (0, 0, 0));
        // trackers take exactly the room they need
        let urls = &mut StrPool::default();
        t.add_tracker("udp://a", 0, urls);
        t.add_tracker("udp://b", 1, urls);
        assert_eq!(t.trackers.capacity(), 2);
        t.replace_trackers(&[("udp://c".into(), 0)], urls);
        assert_eq!(t.trackers.capacity(), 1);
    }

    /// A complete piece set without a picker is kept as `have_all` (libtorrent `m_have_all`),
    /// and reads, saves and starts like the bitfield it stands for.
    #[test]
    fn a_stopped_seed_keeps_no_bitfield() {
        let (bytes, _) = make_torrent(&[("a", 20_000), ("b", 30_000), ("c", 10_000)], 16 * 1024);
        let ti = Arc::new(TorrentInfo::from_bytes(&bytes).unwrap());
        let mut t = Torrent::new(
            TorrentId::default(),
            ti.info_hashes(),
            String::new(),
            "/x".into(),
            TorrentFlags::DEFAULT,
        );
        t.set_metadata(ti);
        t.set_have(Some(Bitfield::new(4, true)));
        assert!(t.have_all && t.have.is_none());
        assert!(t.is_seed() && t.is_finished());
        assert_eq!((t.num_have(), t.total_done(), t.wanted()), (4, 60_000, (60_000, 60_000)));
        assert_eq!(t.file_progress(), [20_000, 30_000, 10_000]);
        assert_eq!(t.settled_state(), TorrentState::Seeding);
        assert_eq!(t.resume_data().pieces, Some(Bitfield::new(4, true)));
        // the picker gets the full set back
        assert_eq!(t.take_have(), Some(Bitfield::new(4, true)));
        assert!(!t.have_all && t.have_bits().is_none());
        // a partial set stays a bitfield; a set of the wrong length is not taken for all
        let mut some = Bitfield::new(4, true);
        some.set(2, false);
        t.set_have(Some(some.clone()));
        assert!(!t.have_all && t.have == Some(some));
        t.set_have(Some(Bitfield::new(3, true)));
        assert!(!t.have_all && t.have.is_some());
        t.set_have(Some(Bitfield::new(4, true)));
        t.set_have(None);
        assert!(!t.have_all && t.have_bits().is_none());
    }
}
