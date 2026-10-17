//! Simulated engine behind the real [`Session`](crate::Session) API (feature `mock`).
//!
//! It exists so frontends (Win32, nox, later GTK) and scale benchmarks can be built before the
//! real network engine lands in Phase 2. It honours the same contract: the queue and auto-management
//! limits from [`Settings`], magnets that fetch metadata, pieces completing out of order, global
//! rate limits, forced rechecks, and dirty-only status updates. No network or disk I/O happens.
//!
//! Inactive torrents carry only their status. File lists, trackers, peers and piece maps are
//! generated on demand from a per-torrent seed. That mirrors the lazy-metadata design the real
//! engine uses for loadability (docs/ARCHITECTURE.md).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::alert::{Alert, Protocol};
use crate::error::Error;
use crate::metainfo::{TorrentInfo, parse_magnet_uri};
use crate::params::{
    AddTorrentParams, DownloadPriority, QueueMove, RemoveFlags, TorrentFlags, TorrentSource,
};
use crate::session::{Command, Shared};
use crate::settings::Settings;
use crate::status::{
    AnnounceEntry, FileEntry, PeerFlags, PeerInfo, PeerSource, SessionStats, TorrentState, TorrentStatus,
    TrackerStatus,
};
use crate::types::{InfoHashes, Sha1Hash, TorrentId};
use crate::util::bitfield::Bitfield;

/// Knobs for the simulated engine.
#[derive(Clone, Debug)]
pub struct MockConfig {
    /// Synthetic torrents created at startup, for scale testing.
    pub initial_torrents: usize,
    /// Fraction of the synthetic torrents that are force-started and moving data.
    pub active_fraction: f64,
    /// PRNG seed; the same seed gives the same synthetic library.
    pub seed: u64,
    /// Per-torrent simulated speed ceiling, bytes per second.
    pub max_rate: u32,
    /// Save path reported for synthetic torrents. Nothing is ever written there.
    pub save_path: String,
}

impl Default for MockConfig {
    fn default() -> MockConfig {
        MockConfig {
            initial_torrents: 0,
            active_fraction: 0.02,
            seed: 0x5eed_1e55,
            max_rate: 4 << 20,
            save_path: std::env::temp_dir().join("patorrent-mock").to_string_lossy().into_owned(),
        }
    }
}

pub(crate) fn run(rx: mpsc::Receiver<Command>, shared: Arc<Shared>, settings: Settings, config: MockConfig) {
    let tick = Duration::from_millis(settings.tick_interval.max(50) as u64);
    let mut mock = Mock::new(shared, settings, config);
    mock.start();
    let mut next_tick = Instant::now() + tick;
    'run: loop {
        match rx.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(cmd) => {
                mock.handle(cmd);
                // Drain the whole batch before re-running the queue logic, so adding 10k torrents
                // costs O(n) auto-manage passes instead of O(n²).
                while let Ok(cmd) = rx.try_recv() {
                    if matches!(cmd, Command::Shutdown) {
                        break 'run;
                    }
                    mock.handle(cmd);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if mock.needs_auto_manage {
            mock.auto_manage();
        }
        let now = Instant::now();
        if now >= next_tick {
            mock.tick(tick);
            next_tick += tick;
            if next_tick < now {
                next_tick = now + tick;
            }
        }
        mock.flush();
    }
}

struct Torrent {
    st: TorrentStatus,
    /// Real metadata, for torrents added from .torrent files.
    ti: Option<Arc<TorrentInfo>>,
    /// Drives every lazily generated detail (files, trackers, peers, piece order).
    seed: u64,
    max_rate: u32,
    /// Materialised once the torrent downloads in this session; otherwise derived from progress.
    have: Option<Bitfield>,
    file_prio: Option<Vec<DownloadPriority>>,
    extra_trackers: Vec<String>,
    done_bytes: f64,
    active_ms: u64,
    seeding_ms: u64,
    check_ticks: u32,
    metadata_ticks: u32,
    dirty: bool,
}

struct Slot {
    generation: u32,
    torrent: Option<Torrent>,
}

struct Mock {
    shared: Arc<Shared>,
    settings: Settings,
    config: MockConfig,
    slots: Vec<Slot>,
    free: Vec<u32>,
    by_hash: HashMap<Sha1Hash, TorrentId>,
    /// Download queue: unfinished torrents in queue order.
    queue: Vec<TorrentId>,
    out: Vec<Alert>,
    session_paused: bool,
    needs_auto_manage: bool,
    ticks: u64,
    stats: SessionStats,
}

const CHECK_TICKS: u32 = 6;
const METADATA_TICKS: u32 = 4;
const CHECKING_STEP_PPM: u32 = 1_000_000 / CHECK_TICKS;

impl Mock {
    fn new(shared: Arc<Shared>, settings: Settings, config: MockConfig) -> Mock {
        Mock {
            shared,
            settings,
            config,
            slots: Vec::new(),
            free: Vec::new(),
            by_hash: HashMap::new(),
            queue: Vec::new(),
            out: Vec::new(),
            session_paused: false,
            needs_auto_manage: false,
            ticks: 0,
            stats: SessionStats::default(),
        }
    }

    fn start(&mut self) {
        let port = listen_port(&self.settings.listen_interfaces);
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        self.out.push(Alert::ListenSucceeded { address: addr, protocol: Protocol::Tcp });
        self.out.push(Alert::ListenSucceeded { address: addr, protocol: Protocol::Udp });
        for i in 0..self.config.initial_torrents {
            self.add_synthetic(i);
        }
        self.out.push(Alert::Log {
            id: None,
            message: format!("mock engine started with {} synthetic torrents", self.config.initial_torrents),
        });
        self.auto_manage();
        self.flush();
    }

    fn flush(&mut self) {
        for alert in self.out.drain(..) {
            self.shared.alerts.post(alert);
        }
    }

    fn get(&mut self, id: TorrentId) -> Option<&mut Torrent> {
        slot_get(&mut self.slots, id)
    }

    fn insert(&mut self, t: Torrent) -> TorrentId {
        let index = match self.free.pop() {
            Some(i) => i,
            None => {
                self.slots.push(Slot { generation: 0, torrent: None });
                (self.slots.len() - 1) as u32
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.generation += 1;
        let id = TorrentId::new(index, slot.generation);
        let mut t = t;
        t.st.id = id;
        if !t.st.is_finished {
            t.st.queue_position = Some(self.queue.len() as u32);
            self.queue.push(id);
        }
        self.by_hash.insert(t.st.info_hashes.best_v1_compatible(), id);
        slot.torrent = Some(t);
        self.needs_auto_manage = true;
        id
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::AddTorrent(p) => self.add_torrent(*p),
            Command::RemoveTorrent(id, flags) => self.remove_torrent(id, flags),
            Command::Pause(id) => {
                if let Some(t) = self.get(id).filter(|t| !t.st.flags.contains(TorrentFlags::PAUSED)) {
                    t.st.flags.insert(TorrentFlags::PAUSED);
                    t.dirty = true;
                    self.out.push(Alert::TorrentPaused { id });
                    self.needs_auto_manage = true;
                }
            }
            Command::Resume(id) => {
                if let Some(t) = self.get(id).filter(|t| t.st.flags.contains(TorrentFlags::PAUSED)) {
                    t.st.flags.remove(TorrentFlags::PAUSED);
                    t.dirty = true;
                    self.out.push(Alert::TorrentResumed { id });
                    self.needs_auto_manage = true;
                }
            }
            Command::SetFlags { id, flags, mask } => {
                if let Some(t) = self.get(id) {
                    let was_paused = t.st.flags.contains(TorrentFlags::PAUSED);
                    t.st.flags = (t.st.flags & !mask) | (flags & mask);
                    t.dirty = true;
                    match (was_paused, t.st.flags.contains(TorrentFlags::PAUSED)) {
                        (false, true) => self.out.push(Alert::TorrentPaused { id }),
                        (true, false) => self.out.push(Alert::TorrentResumed { id }),
                        _ => {}
                    }
                    self.needs_auto_manage = true;
                }
            }
            Command::Queue(id, m) => self.move_in_queue(id, m),
            Command::ForceRecheck(id) => {
                if let Some(t) = self.get(id) {
                    let prev = t.st.state;
                    t.st.state = TorrentState::CheckingFiles;
                    t.st.progress_ppm = 0;
                    t.check_ticks = CHECK_TICKS;
                    t.dirty = true;
                    self.out.push(Alert::StateChanged {
                        id,
                        state: TorrentState::CheckingFiles,
                        prev_state: prev,
                    });
                }
            }
            Command::ForceReannounce(id) => {
                if let Some(t) = self.get(id) {
                    let url = trackers_for(t).into_iter().next().unwrap_or_default();
                    let num_peers = (t.seed % 50) as u32;
                    self.out.push(Alert::TrackerReply { id, url, num_peers });
                }
            }
            Command::SetFilePriority { id, file, priority } => {
                if let Some(t) = self.get(id) {
                    let n = files_for(t).len();
                    let prio = t.file_prio.get_or_insert_with(|| vec![DownloadPriority::NORMAL; n]);
                    if let Some(p) = prio.get_mut(file as usize) {
                        *p = priority;
                    }
                    recompute_wanted(t);
                }
            }
            Command::PrioritizeFiles(id, prios) => {
                if let Some(t) = self.get(id) {
                    let n = files_for(t).len();
                    let mut prios = prios;
                    prios.resize(n, DownloadPriority::NORMAL);
                    t.file_prio = Some(prios);
                    recompute_wanted(t);
                }
            }
            Command::SetDownloadLimit(id, limit) => {
                if let Some(t) = self.get(id) {
                    t.st.download_limit = limit;
                    t.dirty = true;
                }
            }
            Command::SetUploadLimit(id, limit) => {
                if let Some(t) = self.get(id) {
                    t.st.upload_limit = limit;
                    t.dirty = true;
                }
            }
            Command::MoveStorage(id, path) => {
                if let Some(t) = self.get(id) {
                    t.st.save_path = path.clone();
                    t.dirty = true;
                    self.out.push(Alert::Log { id: Some(id), message: format!("storage moved to {path}") });
                }
            }
            // The simulated swarm has no real peers to connect to.
            Command::ConnectPeer(..) | Command::SetIpFilter(_) | Command::SetPortFilter(_) => {}
            Command::PostTorrentUpdates => {
                let mut changed = Vec::new();
                for t in self.slots.iter_mut().filter_map(|s| s.torrent.as_mut()) {
                    if t.dirty {
                        t.dirty = false;
                        changed.push(t.st.clone());
                    }
                }
                self.out.push(Alert::StateUpdate(changed));
            }
            Command::PostSessionStats => {
                let mut s = self.stats;
                s.num_torrents = self.by_hash.len() as u32;
                self.out.push(Alert::SessionStats(s));
            }
            Command::PostPeerInfo(id) => {
                if let Some(t) = self.get(id) {
                    let peers = peers_for(t);
                    self.out.push(Alert::PeerInfo { id, peers });
                }
            }
            Command::PostFileList(id) => {
                if let Some(t) = self.get(id) {
                    let files: Arc<[FileEntry]> = files_for(t).into();
                    self.out.push(Alert::FileList { id, files });
                }
            }
            Command::PostFileProgress(id) => {
                if let Some(t) = self.get(id) {
                    let have = have_for(t);
                    let done = files_for(t)
                        .iter()
                        .map(|f| file_done(f, &have, t.st.piece_length as u64, t.st.total))
                        .collect();
                    self.out.push(Alert::FileProgress { id, done });
                }
            }
            Command::PostTrackers(id) => {
                if let Some(t) = self.get(id) {
                    let trackers = tracker_entries(t);
                    self.out.push(Alert::TrackerList { id, trackers });
                }
            }
            Command::PostPieceInfo(id) => {
                if let Some(t) = self.get(id) {
                    let have = have_for(t);
                    let mut rng = Rng::new(t.seed ^ 0xa7a1);
                    let base = (t.seed % 12) as u32;
                    let availability = (0..have.len()).map(|_| base + rng.below(8) as u32).collect();
                    self.out.push(Alert::PieceInfo { id, have, availability });
                }
            }
            Command::ApplySettings(s) => {
                self.settings = *s;
                self.needs_auto_manage = true;
            }
            Command::PauseSession => self.session_paused = true,
            Command::ResumeSession => self.session_paused = false,
            Command::Shutdown => {}
        }
    }

    fn add_torrent(&mut self, p: AddTorrentParams) {
        let parsed = match &p.source {
            TorrentSource::TorrentFile(b) | TorrentSource::Resume(b) => parse_torrent_file(b),
            TorrentSource::Magnet(uri) => parse_magnet(uri),
            TorrentSource::InfoHash(ih) => {
                Ok(Parsed { info_hashes: *ih, name: None, trackers: Vec::new(), ti: None })
            }
        };
        let display = p.name.clone().unwrap_or_else(|| match &p.source {
            TorrentSource::Magnet(m) => m.chars().take(60).collect(),
            _ => "torrent".to_owned(),
        });
        let parsed = match parsed {
            Ok(x) => x,
            Err(error) => {
                self.out.push(Alert::AddTorrentFailed { name: display, error });
                return;
            }
        };
        let key = parsed.info_hashes.best_v1_compatible();
        if self.by_hash.contains_key(&key) {
            let name = parsed.name.or(p.name).unwrap_or(display);
            self.out.push(Alert::AddTorrentFailed { name, error: Error::DuplicateTorrent });
            return;
        }

        let seed = u64::from_le_bytes(key.0[..8].try_into().unwrap());
        let name = p.name.clone().or(parsed.name).unwrap_or_else(|| key.to_hex());
        let mut rng = Rng::new(seed);
        let max_rate = (self.config.max_rate as f64 * (0.2 + 0.8 * rng.unit())) as u32;
        let mut st = TorrentStatus {
            info_hashes: parsed.info_hashes,
            name,
            save_path: p.save_path.clone(),
            flags: p.flags,
            added_time: now_unix(),
            download_limit: p.download_limit,
            upload_limit: p.upload_limit,
            ..TorrentStatus::default()
        };
        let mut metadata_ticks = 0;
        match &parsed.ti {
            Some(ti) => set_metadata_from(&mut st, ti),
            None => {
                st.state = TorrentState::DownloadingMetadata;
                metadata_ticks = METADATA_TICKS;
            }
        }
        let mut extra_trackers = parsed.trackers;
        extra_trackers.extend(p.trackers.iter().cloned());
        let t = Torrent {
            st,
            ti: parsed.ti,
            seed,
            max_rate,
            have: None,
            file_prio: (!p.file_priorities.is_empty()).then(|| p.file_priorities.clone()),
            extra_trackers,
            done_bytes: 0.0,
            active_ms: 0,
            seeding_ms: 0,
            check_ticks: 0,
            metadata_ticks,
            dirty: true,
        };
        let info_hashes = t.st.info_hashes;
        let name = t.st.name.clone();
        let id = self.insert(t);
        if let Some(t) = self.get(id) {
            if t.file_prio.is_some() {
                recompute_wanted(t);
            }
        }
        self.out.push(Alert::TorrentAdded { id, info_hashes, name });
    }

    fn add_synthetic(&mut self, i: usize) {
        let mut rng = Rng::new(self.config.seed ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut hash = [0u8; 20];
        for chunk in hash.chunks_mut(8) {
            let bytes = rng.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        let key = Sha1Hash(hash);
        let seed = u64::from_le_bytes(hash[..8].try_into().unwrap());
        let total = log_uniform(&mut rng, 30e6, 30e9) as u64;
        let mut st = TorrentStatus {
            info_hashes: InfoHashes::v1(key),
            name: synthetic_name(&mut rng, i),
            save_path: self.config.save_path.clone(),
            added_time: now_unix() - rng.below(3 * 365 * 86_400) as i64,
            all_time_upload: rng.below(total * 3),
            ..TorrentStatus::default()
        };
        set_metadata(&mut st, total);

        let roll = rng.unit();
        let active = roll < self.config.active_fraction;
        let downloading = if active { rng.chance(0.5) } else { roll > 0.85 };
        if downloading {
            let done = (total as f64 * rng.unit() * 0.9) as u64;
            st.total_wanted_done = done;
            st.total_done = done;
            st.all_time_download = done;
            st.progress_ppm = ppm(done, total);
        } else {
            mark_complete(&mut st);
            st.completed_time = st.added_time + rng.below(86_400) as i64;
            st.all_time_download = total;
        }
        st.flags = if active {
            TorrentFlags::empty() // force-started: runs regardless of queue limits
        } else if !downloading && roll < 0.6 {
            TorrentFlags::AUTO_MANAGED | TorrentFlags::PAUSED // queued seed
        } else {
            TorrentFlags::PAUSED // stopped
        };
        let max_rate = (self.config.max_rate as f64 * (0.2 + 0.8 * rng.unit())) as u32;
        let done_bytes = st.total_wanted_done as f64;
        self.insert(Torrent {
            st,
            ti: None,
            seed,
            max_rate,
            have: None,
            file_prio: None,
            extra_trackers: Vec::new(),
            done_bytes,
            active_ms: 0,
            seeding_ms: 0,
            check_ticks: 0,
            metadata_ticks: 0,
            dirty: true,
        });
    }

    fn remove_torrent(&mut self, id: TorrentId, flags: RemoveFlags) {
        let Some(slot) = self.slots.get_mut(id.index() as usize) else { return };
        if slot.generation != id.generation() {
            return;
        }
        let Some(t) = slot.torrent.take() else { return };
        self.free.push(id.index());
        let info_hashes = t.st.info_hashes;
        self.by_hash.remove(&info_hashes.best_v1_compatible());
        self.remove_from_queue(id);
        self.out.push(Alert::TorrentRemoved { id, info_hashes });
        if flags.contains(RemoveFlags::DELETE_FILES) {
            self.out.push(Alert::TorrentDeleted { id, info_hashes });
        }
        self.needs_auto_manage = true;
    }

    fn remove_from_queue(&mut self, id: TorrentId) {
        if let Some(pos) = self.queue.iter().position(|&q| q == id) {
            self.queue.remove(pos);
            self.renumber_queue(pos);
        }
    }

    fn renumber_queue(&mut self, from: usize) {
        for pos in from..self.queue.len() {
            let qid = self.queue[pos];
            if let Some(t) = self.get(qid) {
                t.st.queue_position = Some(pos as u32);
                t.dirty = true;
            }
        }
    }

    fn move_in_queue(&mut self, id: TorrentId, m: QueueMove) {
        let Some(pos) = self.queue.iter().position(|&q| q == id) else { return };
        let last = self.queue.len() - 1;
        let target = match m {
            QueueMove::Up => pos.saturating_sub(1),
            QueueMove::Down => (pos + 1).min(last),
            QueueMove::Top => 0,
            QueueMove::Bottom => last,
        };
        if target == pos {
            return;
        }
        let item = self.queue.remove(pos);
        self.queue.insert(target, item);
        self.renumber_queue(pos.min(target));
        self.needs_auto_manage = true;
    }

    /// libtorrent's queueing, simplified: auto-managed torrents are started in queue order up to
    /// `active_downloads` / `active_seeds` / `active_limit` and paused beyond that.
    fn auto_manage(&mut self) {
        self.needs_auto_manage = false;
        let limit = |v: i32| if v < 0 { usize::MAX } else { v as usize };
        let (dl_limit, seed_limit, total_limit) = (
            limit(self.settings.active_downloads),
            limit(self.settings.active_seeds),
            limit(self.settings.active_limit),
        );
        let mut active_total = 0;
        let mut active_dl = 0;
        for &id in &self.queue {
            let Some(t) = slot_get(&mut self.slots, id) else { continue };
            if !t.st.flags.contains(TorrentFlags::AUTO_MANAGED) || t.check_ticks > 0 {
                continue;
            }
            let run = active_dl < dl_limit && active_total < total_limit;
            if run {
                active_dl += 1;
                active_total += 1;
            }
            set_paused(t, !run, &mut self.out);
        }
        let mut active_seeds = 0;
        for slot in &mut self.slots {
            let Some(t) = slot.torrent.as_mut() else { continue };
            if !t.st.is_finished || !t.st.flags.contains(TorrentFlags::AUTO_MANAGED) {
                continue;
            }
            let run = active_seeds < seed_limit && active_total < total_limit;
            if run {
                active_seeds += 1;
                active_total += 1;
            }
            set_paused(t, !run, &mut self.out);
        }
    }

    fn tick(&mut self, dt: Duration) {
        self.ticks += 1;
        let dt_ms = dt.as_millis() as u64;
        let secs = dt.as_secs_f64();
        let mut finished = Vec::new();
        let mut want_down = Vec::new();
        let mut want_up = Vec::new();

        // Pass 1: state transitions and the rates each running torrent wants.
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let Some(t) = slot.torrent.as_mut() else { continue };
            let id = t.st.id;
            if t.check_ticks > 0 {
                t.check_ticks -= 1;
                t.st.progress_ppm = (CHECK_TICKS - t.check_ticks) * CHECKING_STEP_PPM;
                if t.check_ticks == 0 {
                    let state = settled_state(&t.st);
                    t.st.state = state;
                    t.st.progress_ppm = ppm(t.st.total_wanted_done, t.st.total_wanted);
                    self.out.push(Alert::TorrentChecked { id });
                    self.out.push(Alert::StateChanged { id, state, prev_state: TorrentState::CheckingFiles });
                }
                t.dirty = true;
                continue;
            }
            let running = !t.st.flags.contains(TorrentFlags::PAUSED) && !self.session_paused;
            if !running {
                if t.st.download_payload_rate != 0 || t.st.upload_payload_rate != 0 || t.st.num_peers != 0 {
                    zero_activity(&mut t.st);
                    t.dirty = true;
                }
                continue;
            }
            t.active_ms += dt_ms;
            t.st.active_duration = t.active_ms / 1000;
            let mut rng = Rng::new(t.seed ^ self.ticks);
            jitter_swarm(&mut t.st, t.seed, &mut rng);
            match t.st.state {
                TorrentState::DownloadingMetadata => {
                    t.metadata_ticks = t.metadata_ticks.saturating_sub(1);
                    if t.metadata_ticks == 0 {
                        let total = log_uniform(&mut rng, 50e6, 8e9) as u64;
                        set_metadata(&mut t.st, total);
                        t.st.state = TorrentState::Downloading;
                        self.out.push(Alert::MetadataReceived { id });
                        self.out.push(Alert::StateChanged {
                            id,
                            state: TorrentState::Downloading,
                            prev_state: TorrentState::DownloadingMetadata,
                        });
                    }
                }
                TorrentState::Downloading => {
                    let cap = if t.st.download_limit > 0 { t.st.download_limit } else { u32::MAX };
                    let rate = walk(t.st.download_payload_rate, t.max_rate, &mut rng).min(cap);
                    want_down.push((index, rate));
                    want_up.push((index, (rate as f64 * 0.15 * rng.unit()) as u32));
                }
                TorrentState::Seeding | TorrentState::Finished => {
                    t.seeding_ms += dt_ms;
                    t.st.seeding_duration = t.seeding_ms / 1000;
                    t.st.finished_duration = t.st.seeding_duration;
                    let rate = if rng.chance(0.6) {
                        walk(t.st.upload_payload_rate, t.max_rate / 4, &mut rng)
                    } else {
                        0
                    };
                    want_up.push((index, rate));
                }
                TorrentState::CheckingFiles | TorrentState::CheckingResumeData => {}
            }
            t.dirty = true;
        }

        // Pass 2: global rate limits scale everyone down proportionally, then apply.
        scale_to_limit(&mut want_down, self.settings.download_rate_limit);
        scale_to_limit(&mut want_up, self.settings.upload_rate_limit);
        let mut down_sum = 0u64;
        let mut up_sum = 0u64;
        for &(index, rate) in &want_up {
            let t = self.slots[index].torrent.as_mut().unwrap();
            let up = if t.st.upload_limit > 0 { rate.min(t.st.upload_limit) } else { rate };
            t.st.upload_payload_rate = up;
            t.st.upload_rate = up + up / 30;
            let bytes = (up as f64 * secs) as u64;
            t.st.total_payload_upload += bytes;
            t.st.total_upload += bytes + bytes / 30;
            t.st.all_time_upload += bytes;
            up_sum += up as u64;
        }
        for &(index, rate) in &want_down {
            let t = self.slots[index].torrent.as_mut().unwrap();
            t.st.download_payload_rate = rate;
            t.st.download_rate = rate + rate / 30;
            down_sum += rate as u64;
            let bytes = rate as f64 * secs;
            t.done_bytes = (t.done_bytes + bytes).min(t.st.total_wanted as f64);
            t.st.total_payload_download += bytes as u64;
            t.st.total_download += bytes as u64 + bytes as u64 / 30;
            t.st.all_time_download += bytes as u64;
            advance_pieces(t);
            if t.st.total_wanted_done >= t.st.total_wanted {
                finished.push(t.st.id);
            }
        }

        for id in finished {
            if let Some(t) = self.get(id) {
                mark_complete_wanted(&mut t.st);
                t.st.download_payload_rate = 0;
                t.st.download_rate = 0;
                let state = t.st.state;
                self.out.push(Alert::TorrentFinished { id });
                self.out.push(Alert::StateChanged { id, state, prev_state: TorrentState::Downloading });
            }
            self.remove_from_queue(id);
            self.needs_auto_manage = true;
        }
        if self.needs_auto_manage {
            self.auto_manage();
        }

        let s = &mut self.stats;
        s.download_rate = down_sum.min(u32::MAX as u64) as u32;
        s.upload_rate = up_sum.min(u32::MAX as u64) as u32;
        s.recv_payload_bytes += (down_sum as f64 * secs) as u64;
        s.sent_payload_bytes += (up_sum as f64 * secs) as u64;
        s.recv_bytes = s.recv_payload_bytes + s.recv_payload_bytes / 30;
        s.sent_bytes = s.sent_payload_bytes + s.sent_payload_bytes / 30;
        s.dht_nodes = (s.dht_nodes + 1 + (self.ticks % 7) as u32).min(320);
        s.has_incoming_connections = self.ticks > 10;
        let (mut peers, mut dl, mut seeding) = (0, 0, 0);
        for t in self.slots.iter().filter_map(|s| s.torrent.as_ref()) {
            if !t.st.flags.contains(TorrentFlags::PAUSED) && !self.session_paused {
                peers += t.st.num_peers;
                if t.st.is_seeding { seeding += 1 } else { dl += 1 }
            }
        }
        (s.num_peers_connected, s.num_downloading, s.num_seeding) = (peers, dl, seeding);
    }
}

// --- per-torrent helpers (free functions so they can borrow a torrent and `out` together) ---

fn slot_get(slots: &mut [Slot], id: TorrentId) -> Option<&mut Torrent> {
    let slot = slots.get_mut(id.index() as usize)?;
    if slot.generation != id.generation() {
        return None;
    }
    slot.torrent.as_mut()
}

fn set_paused(t: &mut Torrent, paused: bool, out: &mut Vec<Alert>) {
    if t.st.flags.contains(TorrentFlags::PAUSED) == paused {
        return;
    }
    t.st.flags.set(TorrentFlags::PAUSED, paused);
    t.dirty = true;
    let id = t.st.id;
    out.push(if paused { Alert::TorrentPaused { id } } else { Alert::TorrentResumed { id } });
}

fn zero_activity(st: &mut TorrentStatus) {
    st.download_rate = 0;
    st.upload_rate = 0;
    st.download_payload_rate = 0;
    st.upload_payload_rate = 0;
    st.num_peers = 0;
    st.num_seeds = 0;
    st.num_connections = 0;
}

fn jitter_swarm(st: &mut TorrentStatus, seed: u64, rng: &mut Rng) {
    let base = 2 + (seed % 60) as u32;
    st.list_peers = base * 3;
    st.list_seeds = base;
    st.num_complete = Some(base * 2 + (seed % 7) as u32);
    st.num_incomplete = Some(base + (seed % 11) as u32);
    st.num_peers = (base / 2 + rng.below(base as u64 / 2 + 1) as u32).max(1);
    st.num_seeds = if st.is_seeding { 0 } else { st.num_peers / 3 };
    st.num_connections = st.num_peers;
    st.distributed_copies = Some(1.0 + (seed % 900) as f32 / 100.0);
    st.current_tracker = format!("udp://tracker{}.example:6969/announce", seed % 3);
    st.next_announce = Some(1800 - (rng.below(1800) as u32));
}

fn set_metadata(st: &mut TorrentStatus, total: u64) {
    st.has_metadata = true;
    st.total = total;
    st.total_wanted = total;
    st.piece_length = piece_length_for(total);
    st.pieces_total = total.div_ceil(st.piece_length as u64) as u32;
}

fn set_metadata_from(st: &mut TorrentStatus, ti: &TorrentInfo) {
    st.has_metadata = true;
    st.total = ti.total_size();
    st.total_wanted = ti.total_size();
    st.piece_length = ti.piece_length();
    st.pieces_total = ti.num_pieces();
}

fn mark_complete(st: &mut TorrentStatus) {
    st.total_wanted_done = st.total_wanted;
    st.total_done = st.total_wanted;
    st.num_pieces = st.pieces_total;
    mark_complete_wanted(st);
}

fn mark_complete_wanted(st: &mut TorrentStatus) {
    st.total_wanted_done = st.total_wanted;
    st.progress_ppm = 1_000_000;
    st.is_finished = true;
    st.is_seeding = st.total_wanted == st.total;
    st.state = settled_state(st);
    st.queue_position = None;
    if st.completed_time == 0 {
        st.completed_time = now_unix();
    }
}

fn settled_state(st: &TorrentStatus) -> TorrentState {
    match (st.is_finished, st.is_seeding) {
        (true, true) => TorrentState::Seeding,
        (true, false) => TorrentState::Finished,
        _ if !st.has_metadata => TorrentState::DownloadingMetadata,
        _ => TorrentState::Downloading,
    }
}

fn recompute_wanted(t: &mut Torrent) {
    let files = files_for(t);
    t.st.total_wanted = files.iter().filter(|f| f.priority.0 > 0).map(|f| f.size).sum();
    t.done_bytes = t.done_bytes.min(t.st.total_wanted as f64);
    t.st.total_wanted_done = t.st.total_wanted_done.min(t.st.total_wanted);
    t.st.progress_ppm = ppm(t.st.total_wanted_done, t.st.total_wanted);
    t.dirty = true;
}

/// Turns accumulated bytes into completed pieces, picked in a scattered order like rarest-first.
fn advance_pieces(t: &mut Torrent) {
    let pl = t.st.piece_length.max(1) as u64;
    let target = ((t.done_bytes as u64) / pl).min(t.st.pieces_total as u64) as usize;
    let have = t.have.get_or_insert_with(|| derived_have(&t.st, t.seed));
    let mut count = have.count();
    let mut rng = Rng::new(t.seed ^ count as u64);
    while count < target {
        let n = have.len();
        let mut i = rng.below(n as u64) as usize;
        for _ in 0..n {
            if !have.get(i) {
                break;
            }
            i = (i + 1) % n;
        }
        have.set(i, true);
        count += 1;
    }
    t.st.num_pieces = count as u32;
    let done = if count == have.len() { t.st.total } else { count as u64 * pl };
    t.st.total_done = done;
    t.st.total_wanted_done = (t.done_bytes as u64).max(done.min(t.st.total_wanted)).min(t.st.total_wanted);
    t.st.progress_ppm = ppm(t.st.total_wanted_done, t.st.total_wanted);
}

fn have_for(t: &Torrent) -> Bitfield {
    t.have.clone().unwrap_or_else(|| derived_have(&t.st, t.seed))
}

/// Piece map consistent with the reported progress, for torrents that have not moved data in
/// this session (nothing is stored for them).
fn derived_have(st: &TorrentStatus, seed: u64) -> Bitfield {
    let n = st.pieces_total as usize;
    if st.progress_ppm >= 1_000_000 || st.total_done >= st.total {
        return Bitfield::new(n, true);
    }
    let mut bf = Bitfield::new(n, false);
    let fraction = st.total_done as f64 / st.total.max(1) as f64;
    let threshold = (fraction * u32::MAX as f64) as u64;
    for i in 0..n {
        if (mix(seed ^ i as u64) & 0xffff_ffff) < threshold {
            bf.set(i, true);
        }
    }
    bf
}

fn files_for(t: &Torrent) -> Vec<FileEntry> {
    let st = &t.st;
    if !st.has_metadata {
        return Vec::new();
    }
    let prio = |i: usize| t.file_prio.as_ref().and_then(|p| p.get(i).copied()).unwrap_or_default();
    if let Some(ti) = &t.ti {
        let fs = ti.files();
        return (0..fs.num_files())
            .map(|i| {
                let first = fs.map_file(i, 0, 0).piece;
                let last = fs
                    .map_file(i, fs.file_size(i).saturating_sub(1), 0)
                    .piece
                    .min(fs.num_pieces().saturating_sub(1));
                FileEntry {
                    path: fs.file_path(i, ""),
                    size: fs.file_size(i),
                    offset: fs.file_offset(i),
                    first_piece: first.min(last),
                    last_piece: last,
                    pad_file: fs.pad_file_at(i),
                    priority: prio(i),
                }
            })
            .collect();
    }
    let mut rng = Rng::new(t.seed ^ 0xf11e);
    let n = if rng.chance(0.4) { 1 } else { 2 + rng.below(7) as usize };
    let weights: Vec<u64> = (0..n).map(|_| 1 + rng.below(100)).collect();
    let wsum: u64 = weights.iter().sum();
    let ext = ["mkv", "flac", "bin", "iso", "pdf", "zip"][rng.below(6) as usize];
    let pl = st.piece_length.max(1) as u64;
    let mut offset = 0;
    let mut files = Vec::with_capacity(n);
    for (i, w) in weights.iter().enumerate() {
        let size = if i + 1 == n { st.total - offset } else { st.total * w / wsum };
        let path = if n == 1 { st.name.clone() } else { format!("{}/Part {:02}.{ext}", st.name, i + 1) };
        let last = if size == 0 { offset / pl } else { (offset + size - 1) / pl };
        let priority = t.file_prio.as_ref().and_then(|p| p.get(i).copied()).unwrap_or_default();
        files.push(FileEntry {
            path,
            size,
            offset,
            first_piece: (offset / pl) as u32,
            last_piece: last as u32,
            pad_file: false,
            priority,
        });
        offset += size;
    }
    files
}

fn file_done(f: &FileEntry, have: &Bitfield, piece_length: u64, total: u64) -> u64 {
    if f.size == 0 || have.is_empty() {
        return 0;
    }
    let end = f.offset + f.size;
    (f.first_piece..=f.last_piece)
        .filter(|&p| (p as usize) < have.len() && have.get(p as usize))
        .map(|p| {
            let start = p as u64 * piece_length;
            let stop = (start + piece_length).min(total);
            stop.min(end).saturating_sub(start.max(f.offset))
        })
        .sum()
}

fn trackers_for(t: &Torrent) -> Vec<String> {
    let mut v: Vec<String> =
        (0..1 + t.seed % 3).map(|i| format!("udp://tracker{i}.example:6969/announce")).collect();
    v.extend(t.extra_trackers.iter().cloned());
    v
}

fn tracker_entries(t: &Torrent) -> Vec<AnnounceEntry> {
    let running = !t.st.flags.contains(TorrentFlags::PAUSED);
    trackers_for(t)
        .into_iter()
        .enumerate()
        .map(|(i, url)| {
            let broken = (t.seed >> i) % 5 == 0;
            AnnounceEntry {
                url,
                tier: i as u32,
                status: match (running, broken) {
                    (false, _) => TrackerStatus::NotContacted,
                    (true, true) => TrackerStatus::Error,
                    (true, false) => TrackerStatus::Working,
                },
                message: if broken && running { "connection timed out".to_owned() } else { String::new() },
                scrape_complete: t.st.num_complete,
                scrape_incomplete: t.st.num_incomplete,
                scrape_downloaded: Some((t.seed % 5000) as u32),
                next_announce: running.then(|| t.st.next_announce.unwrap_or(1800)),
                fails: if broken && running { 1 + (t.seed % 4) as u32 } else { 0 },
            }
        })
        .collect()
}

fn peers_for(t: &Torrent) -> Vec<PeerInfo> {
    const CLIENTS: [&str; 6] = [
        "libtorrent 2.0.11",
        "Transmission 4.0.6",
        "qBittorrent 5.0.2",
        "Deluge 2.1.1",
        "BiglyBT 3.6.0",
        "PaTorrent 0.1.0",
    ];
    let st = &t.st;
    let n = st.num_peers as usize;
    let mut rng = Rng::new(t.seed ^ 0x9ee5);
    (0..n)
        .map(|i| {
            // RFC 5737 documentation ranges only.
            let net = [[192, 0, 2], [198, 51, 100], [203, 0, 113]][i % 3];
            let ip = Ipv4Addr::new(net[0], net[1], net[2], 1 + rng.below(254) as u8);
            let seed_peer = i < st.num_seeds as usize;
            let share = (n as u32).max(1);
            let mut flags = PeerFlags::SUPPORTS_EXTENSIONS;
            if rng.chance(0.5) {
                flags |= PeerFlags::OUTGOING_CONNECTION;
            }
            if rng.chance(0.4) {
                flags |= PeerFlags::UTP_SOCKET;
            }
            if rng.chance(0.3) {
                flags |= PeerFlags::RC4_ENCRYPTED;
            }
            if seed_peer {
                flags |= PeerFlags::SEED;
            }
            if !st.is_finished {
                flags |= PeerFlags::INTERESTING;
            }
            if rng.chance(0.5) {
                flags |= PeerFlags::REMOTE_INTERESTED;
            } else {
                flags |= PeerFlags::CHOKED;
            }
            let source = [PeerSource::TRACKER, PeerSource::DHT, PeerSource::PEX, PeerSource::INCOMING][i % 4];
            PeerInfo {
                ip: SocketAddr::new(IpAddr::V4(ip), 1024 + rng.below(60_000) as u16),
                client: CLIENTS[rng.below(CLIENTS.len() as u64) as usize].to_owned(),
                flags,
                source,
                progress_ppm: if seed_peer { 1_000_000 } else { rng.below(1_000_000) as u32 },
                down_speed: st.download_payload_rate / share,
                up_speed: st.upload_payload_rate / share,
                total_download: st.total_payload_download / share as u64,
                total_upload: st.total_payload_upload / share as u64,
                download_queue_length: rng.below(32) as u32,
                upload_queue_length: rng.below(8) as u32,
                num_hashfails: 0,
                is_web_seed: false,
            }
        })
        .collect()
}

// --- source parsing: the real metadata parsers ---

struct Parsed {
    info_hashes: InfoHashes,
    name: Option<String>,
    trackers: Vec<String>,
    /// Present for .torrent files: the real file layout.
    ti: Option<Arc<TorrentInfo>>,
}

fn parse_torrent_file(b: &[u8]) -> Result<Parsed, Error> {
    let ti = TorrentInfo::from_bytes(b)?;
    Ok(Parsed {
        info_hashes: ti.info_hashes(),
        name: Some(ti.name().to_owned()),
        trackers: ti.trackers().iter().map(|t| t.url.clone()).collect(),
        ti: Some(Arc::new(ti)),
    })
}

fn parse_magnet(uri: &str) -> Result<Parsed, Error> {
    let m = parse_magnet_uri(uri)?;
    Ok(Parsed {
        info_hashes: m.info_hashes,
        name: (!m.name.is_empty()).then_some(m.name),
        trackers: m.trackers,
        ti: None,
    })
}

// --- numbers ---

fn listen_port(interfaces: &str) -> u16 {
    interfaces
        .split(',')
        .next()
        .and_then(|s| s.rsplit(':').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or(6881)
}

/// libtorrent-like automatic piece size: about 1500 pieces, power of two, 16 KiB..16 MiB.
fn piece_length_for(total: u64) -> u32 {
    (total / 1500).next_power_of_two().clamp(16 << 10, 16 << 20) as u32
}

fn ppm(done: u64, total: u64) -> u32 {
    if total == 0 { 1_000_000 } else { ((done as u128 * 1_000_000) / total as u128).min(1_000_000) as u32 }
}

/// Random walk toward a ceiling, so speeds look alive without jumping around.
fn walk(current: u32, ceiling: u32, rng: &mut Rng) -> u32 {
    let ceiling = ceiling.max(1) as f64;
    let cur = if current == 0 { ceiling * 0.3 * rng.unit() } else { current as f64 };
    let next = cur + ceiling * 0.15 * (rng.unit() * 2.0 - 1.0);
    next.clamp(0.0, ceiling) as u32
}

fn scale_to_limit(want: &mut [(usize, u32)], limit: u32) {
    if limit == 0 {
        return;
    }
    let sum: u64 = want.iter().map(|&(_, r)| r as u64).sum();
    if sum > limit as u64 {
        for (_, r) in want.iter_mut() {
            *r = (*r as u64 * limit as u64 / sum) as u32;
        }
    }
}

fn log_uniform(rng: &mut Rng, lo: f64, hi: f64) -> f64 {
    (lo.ln() + (hi.ln() - lo.ln()) * rng.unit()).exp()
}

fn synthetic_name(rng: &mut Rng, i: usize) -> String {
    const A: [&str; 12] = [
        "Open", "Public", "Libre", "Creative", "Archive", "Field", "Studio", "Night", "Northern", "Analog",
        "Free", "Civic",
    ];
    const B: [&str; 12] = [
        "Recordings",
        "Dataset",
        "Footage",
        "Collection",
        "Lectures",
        "Sampler",
        "Atlas",
        "Sessions",
        "Mixtapes",
        "Distro",
        "Scans",
        "Broadcasts",
    ];
    const C: [&str; 8] =
        ["[1080p]", "(FLAC)", "x64 ISO", "Vol. 2", "(2009)", "Complete", "Remastered", "[CC-BY]"];
    let a = A[rng.below(A.len() as u64) as usize];
    let b = B[rng.below(B.len() as u64) as usize];
    let c = C[rng.below(C.len() as u64) as usize];
    format!("{a} {b} {c} #{i:05}")
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// splitmix64 finaliser.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// xorshift64* — fast, deterministic, plenty for a simulation. Not for anything security related.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(mix(seed) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, SessionHandle};

    fn session(n: usize, settings: Settings) -> Session {
        let config = MockConfig { initial_torrents: n, ..MockConfig::default() };
        Session::start_mock(Settings { tick_interval: 50, ..settings }, config)
    }

    /// Polls alerts until `f` returns `Some`, failing after a timeout.
    fn wait_for<T>(h: &SessionHandle, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut alerts = Vec::new();
        while Instant::now() < deadline {
            h.pop_alerts(&mut alerts);
            if let Some(v) = alerts.iter().find_map(&mut f) {
                return v;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for alert");
    }

    const MAGNET: &str = "magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=Big+Buck+Bunny&tr=udp%3A%2F%2Ftracker.example%3A1337";

    #[test]
    fn magnet_add_metadata_and_duplicate() {
        let s = session(0, Settings::default());
        let h = s.handle();
        h.add_torrent(AddTorrentParams::new(TorrentSource::Magnet(MAGNET.into()), "/dl"));
        let id = wait_for(&h, |a| match a {
            Alert::TorrentAdded { id, name, .. } => {
                assert_eq!(name, "Big Buck Bunny");
                Some(*id)
            }
            _ => None,
        });
        h.add_torrent(AddTorrentParams::new(TorrentSource::Magnet(MAGNET.into()), "/dl"));
        wait_for(&h, |a| {
            matches!(a, Alert::AddTorrentFailed { error: Error::DuplicateTorrent, .. }).then_some(())
        });
        wait_for(&h, |a| matches!(a, Alert::MetadataReceived { id: x } if *x == id).then_some(()));
        h.torrent(id).post_trackers();
        let trackers = wait_for(&h, |a| match a {
            Alert::TrackerList { trackers, .. } => Some(trackers.clone()),
            _ => None,
        });
        assert!(trackers.iter().any(|t| t.url == "udp://tracker.example:1337"));
    }

    #[test]
    fn rejects_bad_sources() {
        let s = session(0, Settings::default());
        let h = s.handle();
        h.add_torrent(AddTorrentParams::new(TorrentSource::Magnet("http://nope".into()), "/dl"));
        wait_for(&h, |a| {
            matches!(a, Alert::AddTorrentFailed { error: Error::Metainfo(_), .. }).then_some(())
        });
        h.add_torrent(AddTorrentParams::new(TorrentSource::TorrentFile(b"garbage".to_vec()), "/dl"));
        wait_for(&h, |a| {
            matches!(a, Alert::AddTorrentFailed { error: Error::Metainfo(_), .. }).then_some(())
        });
    }

    #[test]
    fn queue_respects_active_downloads() {
        let settings = Settings { active_downloads: 2, ..Settings::default() };
        let s = session(0, settings);
        let h = s.handle();
        for i in 0..5u8 {
            let magnet = format!("magnet:?xt=urn:btih:{}", Sha1Hash([i + 1; 20]).to_hex());
            h.add_torrent(AddTorrentParams::new(TorrentSource::Magnet(magnet), "/dl"));
        }
        let mut seen: HashMap<TorrentId, (Option<u32>, TorrentFlags)> = HashMap::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut alerts = Vec::new();
        let running = |seen: &HashMap<TorrentId, (Option<u32>, TorrentFlags)>| {
            seen.values().filter(|(_, f)| !f.contains(TorrentFlags::PAUSED)).count()
        };
        // Updates are eventually consistent: keep merging until the queue has settled.
        while (seen.len() < 5 || running(&seen) != 2) && Instant::now() < deadline {
            h.post_torrent_updates();
            std::thread::sleep(Duration::from_millis(20));
            h.pop_alerts(&mut alerts);
            for a in &alerts {
                if let Alert::StateUpdate(v) = a {
                    for st in v {
                        seen.insert(st.id, (st.queue_position, st.flags));
                    }
                }
            }
        }
        assert_eq!(seen.len(), 5);
        assert_eq!(running(&seen), 2, "only active_downloads torrents may run");
        // The two running ones are the head of the queue.
        for (q, f) in seen.values() {
            assert_eq!(!f.contains(TorrentFlags::PAUSED), q.unwrap() < 2);
        }
        let mut positions: Vec<u32> = seen.values().filter_map(|(q, _)| *q).collect();
        positions.sort();
        assert_eq!(positions, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn state_updates_only_carry_changed_torrents() {
        // 1,000 synthetic torrents, ~2% force-started: after the first full update, later updates
        // must only contain the torrents that are actually moving.
        let s = session(1000, Settings::default());
        let h = s.handle();
        h.post_torrent_updates();
        let first = wait_for(&h, |a| match a {
            Alert::StateUpdate(v) => Some(v.len()),
            _ => None,
        });
        assert_eq!(first, 1000);
        std::thread::sleep(Duration::from_millis(200));
        h.post_torrent_updates();
        let later = wait_for(&h, |a| match a {
            Alert::StateUpdate(v) => Some(v.len()),
            _ => None,
        });
        assert!(later < 150, "expected only active torrents, got {later}");
    }

    #[test]
    fn remove_frees_id_and_reports() {
        let s = session(3, Settings::default());
        let h = s.handle();
        h.post_torrent_updates();
        let id = wait_for(&h, |a| match a {
            Alert::StateUpdate(v) => v.first().map(|st| st.id),
            _ => None,
        });
        h.remove_torrent(id, RemoveFlags::DELETE_FILES);
        wait_for(&h, |a| matches!(a, Alert::TorrentDeleted { id: x, .. } if *x == id).then_some(()));
        h.torrent(id).post_peer_info();
        h.post_session_stats();
        let n = wait_for(&h, |a| match a {
            Alert::SessionStats(s) => Some(s.num_torrents),
            Alert::PeerInfo { .. } => panic!("stale id must be ignored"),
            _ => None,
        });
        assert_eq!(n, 2);
    }

    #[test]
    fn file_progress_matches_pieces() {
        let st = TorrentStatus {
            has_metadata: true,
            name: "x".into(),
            total: 10 * 16384 + 100,
            ..TorrentStatus::default()
        };
        let mut t = Torrent {
            st,
            ti: None,
            seed: 42,
            max_rate: 1,
            have: None,
            file_prio: None,
            extra_trackers: vec![],
            done_bytes: 0.0,
            active_ms: 0,
            seeding_ms: 0,
            check_ticks: 0,
            metadata_ticks: 0,
            dirty: false,
        };
        let total = t.st.total;
        set_metadata(&mut t.st, total);
        let files = files_for(&t);
        assert_eq!(files.iter().map(|f| f.size).sum::<u64>(), t.st.total);
        let full = Bitfield::new(t.st.pieces_total as usize, true);
        let pl = t.st.piece_length as u64;
        let done: u64 = files.iter().map(|f| file_done(f, &full, pl, t.st.total)).sum();
        assert_eq!(done, t.st.total);
    }
}
