//! The native session protocol over the WebSocket (docs/WEBUI.md "Messages"): one [`Client`]
//! per socket holds that client's `frontend::model::TorrentList` (fed from the shared
//! [`State`] cache, never from the engine), its `view` subscription (filter, sort, columns,
//! window) and what it was last sent, and builds the `hello`, `rows` and `tick` messages.
//!
//! Cells are the strings `frontend::columns` gives the Win32 list, so the page shows exactly
//! what the desktop shows. The `detail` subscription (docs/WEBUI.md "Detail tabs") works the
//! same way: the tab's content is built from the cache with `frontend::detail` and `fmt`, sent
//! whole once, then again on a tick only when it changed. Nothing here does I/O beyond posting
//! the tab's `post_*` requests through the [`SessionHandle`]; `server.rs` moves the frames.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use patorrent_core::{DownloadPriority, SessionHandle, TorrentId, VERSION};
use patorrent_frontend::columns::{Align, Column};
use patorrent_frontend::detail::{self, ColumnDef};
use patorrent_frontend::fmt;
use patorrent_frontend::model::{Filter, TorrentList};
use patorrent_frontend::status::{Category, status_class, status_text};

use super::json::{self, Json};
use super::{Entry, Reply, ReplyKind, SPEED_SAMPLES, State};

/// Most rows one window may ask for (the page shows about 60 and asks for some overscan).
pub const MAX_WINDOW: usize = 200;
/// The window a `view` without one gets.
const DEFAULT_WINDOW: usize = 60;

/// The stable wire id of a column (snake_case of the `Column` name).
pub fn column_id(c: Column) -> &'static str {
    match c {
        Column::Name => "name",
        Column::QueuePosition => "queue_position",
        Column::Size => "size",
        Column::Done => "done",
        Column::Status => "status",
        Column::Seeds => "seeds",
        Column::Peers => "peers",
        Column::DownSpeed => "down_speed",
        Column::UpSpeed => "up_speed",
        Column::Eta => "eta",
        Column::Uploaded => "uploaded",
        Column::Downloaded => "downloaded",
        Column::Ratio => "ratio",
        Column::Availability => "availability",
        Column::Label => "label",
        Column::AddedOn => "added_on",
        Column::CompletedOn => "completed_on",
        Column::Tracker => "tracker",
        Column::DownLimit => "down_limit",
        Column::UpLimit => "up_limit",
        Column::Remaining => "remaining",
        Column::SeedsPeers => "seeds_peers",
        Column::SaveAs => "save_as",
        Column::Wasted => "wasted",
    }
}

pub fn column_from_id(id: &str) -> Option<Column> {
    Column::ALL.into_iter().find(|c| column_id(*c) == id)
}

fn category_id(c: Category) -> &'static str {
    match c {
        Category::All => "all",
        Category::Downloading => "downloading",
        Category::Completed => "completed",
        Category::Active => "active",
        Category::Inactive => "inactive",
    }
}

pub(crate) fn category_from_id(id: &str) -> Option<Category> {
    Category::ALL.into_iter().find(|c| category_id(*c) == id)
}

/// The detail pane's tabs, in the 2.2.1 order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    pub fn id(self) -> &'static str {
        match self {
            Tab::General => "general",
            Tab::Trackers => "trackers",
            Tab::Peers => "peers",
            Tab::Pieces => "pieces",
            Tab::Files => "files",
            Tab::Speed => "speed",
            Tab::Logger => "logger",
        }
    }

    pub fn from_id(id: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.id() == id)
    }
}

/// The detail subscription: one torrent, one tab, and what was last sent for it.
struct Detail {
    id: TorrentId,
    hash: String,
    tab: Tab,
    /// A hash of the content last sent, so an unchanged tab sends nothing on a tick; for Speed
    /// and Logger just a mark that the first message went out.
    sent: Option<u64>,
    /// The newest speed sample sent (`SpeedRing::pushed`), 0 before the first message.
    speed_seq: u64,
    /// The speed axis top last sent, in bytes per second.
    scale: u32,
    /// The newest log line sent (`LogLine::seq`), 0 before the first message.
    log_seq: u64,
}

/// What a client is looking at.
#[derive(Clone, Debug, PartialEq)]
struct View {
    filter: Filter,
    /// Lower-case; empty for none.
    search: String,
    sort: Option<(Column, bool)>,
    columns: Vec<Column>,
    first: usize,
    count: usize,
}

/// One WebSocket client's state.
pub struct Client {
    list: TorrentList,
    /// `State::next_version` when the list was last synced: entries at or past it are new.
    synced: u64,
    removals: u64,
    view: Option<View>,
    idle: bool,
    detail: Option<Detail>,
    /// The last version sent per torrent in the window.
    sent: HashMap<TorrentId, u64>,
    /// The window's order as last sent.
    last_ids: Vec<TorrentId>,
    /// The filtered, sorted view with the search applied.
    rows: Vec<TorrentId>,
    rows_stale: bool,
    /// The label list and counts as last sent (`labels` and `no_label` in a tick).
    labels_sent: Option<(Json, usize)>,
    /// `State::rss_version` as last sent (`rss` in a tick).
    rss_sent: u64,
}

impl Default for Client {
    fn default() -> Client {
        Client::new()
    }
}

impl Client {
    pub fn new() -> Client {
        Client {
            list: TorrentList::new(),
            synced: 0,
            removals: 0,
            view: None,
            idle: false,
            detail: None,
            sent: HashMap::new(),
            last_ids: Vec::new(),
            rows: Vec::new(),
            rows_stale: true,
            labels_sent: None,
            rss_sent: 0,
        }
    }

    /// The detail subscription: (torrent hash, tab id).
    pub fn detail(&self) -> Option<(&str, &str)> {
        self.detail.as_ref().map(|d| (d.hash.as_str(), d.tab.id()))
    }

    pub fn is_idle(&self) -> bool {
        self.idle
    }

    /// The first message: the version, the column list, the categories and the labels.
    pub(crate) fn hello(&self, st: &State) -> Json {
        let columns: Vec<Json> = Column::ALL
            .iter()
            .map(|&c| {
                Json::obj()
                    .with("id", column_id(c))
                    .with("title", c.title())
                    .with("width", c.default_width() as i64)
                    .with("align", if c.align() == Align::Left { "left" } else { "right" })
                    .with("visible", c.default_visible())
                    .with("desc", c.default_descending())
            })
            .collect();
        let categories: Vec<Json> = Category::ALL
            .iter()
            .map(|&c| Json::obj().with("id", category_id(c)).with("title", c.label()))
            .collect();
        let labels: Vec<Json> = st.categories.keys().map(|l| Json::Str(l.clone())).collect();
        let defs = |cols: &[ColumnDef]| {
            Json::Arr(
                cols.iter()
                    .map(|&(title, width, right)| {
                        Json::obj().with("title", title).with("width", width as i64).with("right", right)
                    })
                    .collect(),
            )
        };
        let detail = Json::obj()
            .with("trackers", defs(&detail::TRACKER_COLUMNS))
            .with("peers", defs(&detail::PEER_COLUMNS))
            .with("pieces", defs(&detail::PIECE_COLUMNS))
            .with("files", defs(&detail::FILE_COLUMNS));
        Json::obj()
            .with("t", "hello")
            .with("version", VERSION)
            .with("columns", Json::Arr(columns))
            .with("categories", Json::Arr(categories))
            .with("labels", Json::Arr(labels))
            .with("detail", detail)
            .with("confirm_delete", st.prefs.confirm_delete)
    }

    /// Handles one client message; the replies to send back, in order.
    pub(crate) fn on_message(&mut self, text: &str, st: &mut State) -> Vec<Json> {
        let msg = match json::parse(text) {
            Ok(m @ Json::Obj(_)) => m,
            Ok(_) => return vec![error("expected an object")],
            Err(e) => return vec![error(&format!("bad json: {e}"))],
        };
        match msg.get("t").and_then(Json::as_str) {
            Some("view") => match parse_view(&msg) {
                Ok(view) => {
                    self.idle = false;
                    self.set_view(view);
                    self.sync(st);
                    vec![self.rows_message(st)]
                }
                Err(e) => vec![error(e)],
            },
            Some("detail") => {
                let Some(hash) = msg.get("id").and_then(Json::as_str).map(str::to_ascii_lowercase) else {
                    self.detail = None;
                    return Vec::new();
                };
                let Some(tab) = Tab::from_id(msg.get("tab").and_then(Json::as_str).unwrap_or("general"))
                else {
                    return vec![error("unknown tab")];
                };
                // a torrent the cache does not know (removed a moment ago): the pane stays empty
                let Some(&id) = st.by_hash.get(&hash) else {
                    self.detail = None;
                    return Vec::new();
                };
                if self.detail.as_ref().is_some_and(|d| d.id == id && d.tab == tab) {
                    return Vec::new();
                }
                self.detail = Some(Detail { id, hash, tab, sent: None, speed_seq: 0, scale: 0, log_seq: 0 });
                self.detail_message(st).into_iter().collect()
            }
            Some("idle") => {
                self.idle = true;
                Vec::new()
            }
            Some(t) => vec![error(&format!("unknown message type {t:?}"))],
            None => vec![error("missing t")],
        }
    }

    /// Once per status tick: what changed for this client (the `tick` for its view, then the
    /// detail tab when its content changed), or nothing while it is idle.
    pub(crate) fn on_tick(&mut self, st: &mut State) -> Vec<Json> {
        let mut out = Vec::new();
        if self.idle {
            return out;
        }
        if self.view.is_some() {
            self.sync(st);
            out.push(self.tick_message(st));
        }
        out.extend(self.detail_message(st));
        out
    }

    /// Posts the engine requests the subscribed tab's next update needs (`server.rs` calls it
    /// after a `detail` message and on every tick while subscribed).
    pub(crate) fn request_detail(&self, handle: &SessionHandle) {
        let Some(d) = &self.detail else { return };
        let t = handle.torrent(d.id);
        match d.tab {
            Tab::General | Tab::Trackers => t.post_trackers(),
            Tab::Peers => t.post_peer_info(),
            Tab::Pieces => t.post_piece_info(),
            Tab::Files => {
                t.post_file_list();
                t.post_file_progress();
            }
            Tab::Speed | Tab::Logger => {}
        }
    }

    /// The subscribed tab's message: its whole content the first time and whenever it changed
    /// since (Speed and Logger append instead); nothing when it did not, or when the tab's
    /// engine reply has not arrived yet.
    fn detail_message(&mut self, st: &mut State) -> Option<Json> {
        let d = self.detail.as_mut()?;
        let base = || Json::obj().with("t", "detail").with("id", d.hash.as_str()).with("tab", d.tab.id());
        if !st.torrents.contains_key(&d.id) {
            // removed while subscribed: say so once, then nothing more
            let msg = base().with("gone", true);
            self.detail = None;
            return Some(msg);
        }
        match d.tab {
            Tab::Speed => {
                let ring = st.watch_speed(d.id);
                if d.sent.is_some() && ring.pushed == d.speed_seq {
                    return None;
                }
                // the whole ring the first time, or when samples the client never saw have
                // already left the ring; else only the ones since
                let n = ring.samples.len();
                let missed = ring.pushed.saturating_sub(d.speed_seq);
                let all = d.sent.is_none() || missed > n as u64;
                let from = if all { 0 } else { n - missed as usize };
                let samples: Vec<Json> = ring
                    .samples
                    .iter()
                    .skip(from)
                    .map(|&(down, up)| Json::Arr(vec![down.into(), up.into()]))
                    .collect();
                let (down, up) = ring.samples.back().copied().unwrap_or((0, 0));
                let top = speed_scale(ring.samples.iter().map(|&(a, b)| a.max(b)).max().unwrap_or(0));
                let mut msg =
                    base().with("samples", Json::Arr(samples)).with("legend", speed_legend(down, up));
                if all {
                    msg.set("all", true);
                    msg.set("keep", SPEED_SAMPLES);
                }
                if all || top != d.scale {
                    msg.set("scale", speed_axis(top));
                    d.scale = top;
                }
                d.speed_seq = ring.pushed;
                d.sent = Some(0);
                Some(msg)
            }
            Tab::Logger => {
                let id = d.id;
                let lines: Vec<Json> = st
                    .log
                    .iter()
                    .filter(|l| l.seq > d.log_seq && l.torrent.is_none_or(|t| t == id))
                    .map(|l| Json::Arr(vec![l.time.into(), l.text.as_str().into()]))
                    .collect();
                let newest = st.log.back().map_or(0, |l| l.seq);
                if d.sent.is_some() && lines.is_empty() {
                    return None;
                }
                let all = d.sent.is_none();
                d.log_seq = newest;
                d.sent = Some(0);
                let mut msg = base().with("lines", Json::Arr(lines));
                if all {
                    msg.set("all", true);
                }
                Some(msg)
            }
            _ => {
                let e = &st.torrents[&d.id];
                let user_paused = self.list.extra(d.id).user_paused;
                let mut msg = base();
                let built = match d.tab {
                    Tab::General => general(e, st, user_paused, &mut msg),
                    Tab::Trackers => trackers(e, st, &mut msg),
                    Tab::Peers => peers(e, st, &mut msg),
                    Tab::Pieces => pieces(e, st, &mut msg),
                    Tab::Files => files(e, st, &mut msg),
                    Tab::Speed | Tab::Logger => unreachable!(),
                };
                if !built {
                    return None;
                }
                let mut h = DefaultHasher::new();
                msg.to_json().hash(&mut h);
                let digest = h.finish();
                if d.sent == Some(digest) {
                    return None;
                }
                d.sent = Some(digest);
                Some(msg)
            }
        }
    }

    fn set_view(&mut self, view: View) {
        self.list.set_filter(view.filter.clone());
        self.list.set_sort(view.sort);
        if self.view.as_ref().is_none_or(|v| v.search != view.search) {
            self.rows_stale = true;
        }
        self.view = Some(view);
    }

    /// Brings the list up to date with the cache: entries changed since the last sync (by
    /// `Entry::version`) and torrents removed since.
    fn sync(&mut self, st: &State) {
        if self.removals != st.removals {
            let gone: Vec<TorrentId> = self.list.ids().filter(|id| !st.torrents.contains_key(id)).collect();
            for id in gone {
                self.list.remove(id);
                self.sent.remove(&id);
            }
            self.removals = st.removals;
        }
        if self.synced != st.next_version {
            for (id, e) in &st.torrents {
                if e.version >= self.synced {
                    self.list.update(std::iter::once(e.status.clone()));
                    self.list.set_label(*id, st.torrent_categories.get(&e.hash).map_or("", String::as_str));
                    self.list.set_user_paused(*id, st.user_paused.contains(&e.hash));
                }
            }
            self.synced = st.next_version;
        }
        if self.list.refresh() || self.rows_stale {
            self.rows_stale = false;
            let search = self.view.as_ref().map_or("", |v| v.search.as_str());
            self.rows.clear();
            if search.is_empty() {
                self.rows.extend_from_slice(self.list.view());
            } else {
                let list = &self.list;
                self.rows.extend(
                    list.view()
                        .iter()
                        .copied()
                        .filter(|id| list.get(*id).is_some_and(|s| s.name.to_lowercase().contains(search))),
                );
            }
        }
    }

    /// The ids in the window right now.
    fn window(&self) -> Vec<TorrentId> {
        let Some(v) = &self.view else { return Vec::new() };
        let total = self.rows.len();
        let first = v.first.min(total);
        self.rows[first..(first + v.count).min(total)].to_vec()
    }

    fn cell(&self, e: &Entry) -> Json {
        let x = self.list.extra(e.status.id);
        let columns = self.view.as_ref().map_or(&[][..], |v| v.columns.as_slice());
        let cells: Vec<Json> = columns.iter().map(|c| Json::Str(c.text(&e.status, x))).collect();
        Json::obj()
            .with("v", e.version)
            .with("c", Json::Arr(cells))
            .with("done", (e.status.progress_ppm.min(1_000_000) / 1000) as i64)
            .with("state", status_class(&e.status, x.user_paused))
    }

    /// The whole window, answering a `view`.
    fn rows_message(&mut self, st: &State) -> Json {
        let ids = self.window();
        let first = self.view.as_ref().map_or(0, |v| v.first.min(self.rows.len()));
        let mut cells = Json::obj();
        self.sent.clear();
        for id in &ids {
            let e = &st.torrents[id];
            cells.set(&e.hash, self.cell(e));
            self.sent.insert(*id, e.version);
        }
        let hashes: Vec<Json> = ids.iter().map(|id| Json::Str(st.torrents[id].hash.clone())).collect();
        self.last_ids = ids;
        Json::obj()
            .with("t", "rows")
            .with("total", self.rows.len())
            .with("first", first)
            .with("ids", Json::Arr(hashes))
            .with("cells", cells)
    }

    /// What changed since the last message: the window's order only when it moved, the cells
    /// of rows whose version changed, then the counts and the status bar every time.
    fn tick_message(&mut self, st: &State) -> Json {
        let ids = self.window();
        let mut out = Json::obj().with("t", "tick").with("total", self.rows.len());
        if ids != self.last_ids {
            let first = self.view.as_ref().map_or(0, |v| v.first.min(self.rows.len()));
            let hashes: Vec<Json> = ids.iter().map(|id| Json::Str(st.torrents[id].hash.clone())).collect();
            out.set("first", first);
            out.set("ids", Json::Arr(hashes));
        }
        let mut cells = Json::obj();
        let mut sent = HashMap::with_capacity(ids.len());
        for id in &ids {
            let e = &st.torrents[id];
            if self.sent.get(id) != Some(&e.version) {
                cells.set(&e.hash, self.cell(e));
            }
            sent.insert(*id, e.version);
        }
        self.sent = sent;
        self.last_ids = ids;
        if !cells.entries().is_empty() {
            out.set("cells", cells);
        }
        out.set("counts", self.counts());
        if let Some((labels, no_label)) = self.labels_update(st) {
            out.set("labels", labels);
            out.set("no_label", no_label);
        }
        out.set("status", status_bar(st));
        // the RSS Downloader changed: the page reloads its feeds (GET /pa/rss)
        if st.rss_version != self.rss_sent {
            self.rss_sent = st.rss_version;
            out.set("rss", st.rss_version);
        }
        out
    }

    /// The Labels branch of the tree when it changed since the last tick: every label the
    /// daemon knows (the category map, so a label stays listed while no torrent has it) with
    /// its torrent count, sorted by name, and the count of torrents without a label.
    fn labels_update(&mut self, st: &State) -> Option<(Json, usize)> {
        let in_use = self.list.labels();
        let mut names: Vec<&str> = st.categories.keys().map(String::as_str).collect();
        names.extend(in_use.iter().map(|(l, _)| l.as_str()).filter(|l| !st.categories.contains_key(*l)));
        names.sort_unstable();
        let count = |l: &str| in_use.iter().find(|(x, _)| x == l).map_or(0, |x| x.1);
        let labels = Json::Arr(names.iter().map(|&l| Json::Arr(vec![l.into(), count(l).into()])).collect());
        let no_label = self.list.len() - in_use.iter().map(|x| x.1).sum::<usize>();
        let now = (labels, no_label);
        if self.labels_sent.as_ref() == Some(&now) {
            return None;
        }
        self.labels_sent = Some(now.clone());
        Some(now)
    }

    fn counts(&self) -> Json {
        let mut n = [0usize; Category::ALL.len()];
        for id in self.list.ids() {
            if let Some(s) = self.list.get(id) {
                for (i, c) in Category::ALL.iter().enumerate() {
                    if c.contains(s) {
                        n[i] += 1;
                    }
                }
            }
        }
        let mut out = Json::obj();
        for (i, c) in Category::ALL.iter().enumerate() {
            out.set(category_id(*c), n[i]);
        }
        out
    }
}

/// The General tab: the 2.2.1 Transfer and General groups as label-value pairs, formatted as
/// the Win32 tab formats them (`win32::details::general`), with the Downloaded and
/// Availability bars first.
fn general(e: &Entry, st: &State, user_paused: bool, msg: &mut Json) -> bool {
    let s = &e.status;
    let meta = st.meta.get(&e.hash);
    let limit = |v: u32| if v == 0 { "\u{221e}".to_owned() } else { fmt::speed_always(v) };
    let pair = |label: &str, value: String| Json::Arr(vec![label.into(), value.into()]);
    let remaining = s.total_wanted.saturating_sub(s.total_wanted_done);
    let trackers = match st.cached(s.id, ReplyKind::Trackers) {
        Some(Reply::Trackers(t)) => t.len().to_string(),
        _ => String::new(),
    };
    let pieces = if s.piece_length > 0 {
        format!("{} x {} (have {})", s.pieces_total, fmt::size(s.piece_length as u64), s.num_pieces)
    } else {
        String::new()
    };
    let bars = vec![
        Json::Arr(vec![
            "Downloaded:".into(),
            fmt::percent(s.progress_ppm).into(),
            (s.progress_ppm.min(1_000_000) / 1000).into(),
        ]),
        Json::Arr(vec![
            "Availability:".into(),
            fmt::availability(s.distributed_copies).into(),
            ((s.distributed_copies.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as i64).into(),
        ]),
    ];
    let transfer = vec![
        pair("Time Elapsed:", fmt::duration(s.active_duration)),
        pair("Remaining:", if s.is_finished { String::new() } else { fmt::eta(s.eta()) }),
        pair("Wasted:", fmt::size(s.total_failed_bytes + s.total_redundant_bytes)),
        pair("Downloaded:", fmt::size(s.all_time_download)),
        pair("Uploaded:", fmt::size(s.all_time_upload)),
        pair("Seeds:", format!("{} of {} connected", s.num_seeds, s.num_complete.unwrap_or(s.list_seeds))),
        pair("Download Speed:", fmt::speed_always(s.download_payload_rate)),
        pair("Upload Speed:", fmt::speed_always(s.upload_payload_rate)),
        pair(
            "Peers:",
            format!(
                "{} of {} connected",
                s.num_peers.saturating_sub(s.num_seeds),
                s.num_incomplete.unwrap_or(s.list_peers.saturating_sub(s.list_seeds))
            ),
        ),
        pair("Down Limit:", limit(s.download_limit)),
        pair("Up Limit:", limit(s.upload_limit)),
        pair("Share Ratio:", fmt::ratio(s.ratio())),
        pair("Status:", status_text(s, user_paused)),
    ];
    let general = vec![
        pair("Save As:", s.save_path.clone()),
        pair("Remaining:", fmt::size(remaining)),
        pair("Total Size:", fmt::size(s.total)),
        pair("Added On:", fmt::date_time(s.added_time)),
        pair("Pieces:", pieces),
        pair("Completed On:", fmt::date_time(s.completed_time)),
        pair("Created On:", meta.and_then(|m| m.creation_date).map(fmt::date_time).unwrap_or_default()),
        pair("Created By:", meta.map(|m| m.created_by.clone()).unwrap_or_default()),
        pair("Hash:", s.info_hashes.key_hex().to_uppercase()),
        pair("Trackers:", trackers),
        pair("Comment:", meta.map(|m| m.comment.clone()).unwrap_or_default()),
    ];
    msg.set("bars", Json::Arr(bars));
    msg.set(
        "groups",
        Json::Arr(vec![
            Json::obj().with("title", "Transfer").with("rows", Json::Arr(transfer)),
            Json::obj().with("title", "General").with("rows", Json::Arr(general)),
        ]),
    );
    true
}

/// A table row: the cells, and a sort key per column (a number for the numeric ones, null to
/// sort by the cell text).
fn row(cells: Vec<String>, keys: Vec<Json>) -> Json {
    Json::obj().with("c", Json::Arr(cells.into_iter().map(Json::Str).collect())).with("k", Json::Arr(keys))
}

fn opt(v: Option<u32>) -> Json {
    v.map_or(Json::Null, Json::from)
}

fn trackers(e: &Entry, st: &State, msg: &mut Json) -> bool {
    let Some(Reply::Trackers(list)) = st.cached(e.status.id, ReplyKind::Trackers) else { return false };
    let rows: Vec<Json> = list
        .iter()
        .map(|t| {
            let cells = (0..detail::TRACKER_COLUMNS.len()).map(|c| detail::tracker_cell(t, c)).collect();
            let keys = vec![
                Json::Null,
                Json::Null,
                opt(t.next_announce),
                opt(t.scrape_complete),
                opt(t.scrape_incomplete),
                opt(t.scrape_downloaded),
            ];
            row(cells, keys)
        })
        .collect();
    msg.set("rows", Json::Arr(rows));
    true
}

fn peers(e: &Entry, st: &State, msg: &mut Json) -> bool {
    let Some(Reply::Peers(list)) = st.cached(e.status.id, ReplyKind::Peers) else { return false };
    let rows: Vec<Json> = list
        .iter()
        .map(|p| {
            let cells = (0..detail::PEER_COLUMNS.len()).map(|c| detail::peer_cell(p, c)).collect();
            let keys = vec![
                Json::Null,
                Json::Null,
                Json::Null,
                p.progress_ppm.into(),
                p.down_speed.into(),
                p.up_speed.into(),
                p.download_queue_length.into(),
                p.total_download.into(),
                p.total_upload.into(),
            ];
            row(cells, keys)
        })
        .collect();
    msg.set("rows", Json::Arr(rows));
    true
}

/// The piece bitfield run-length encoded as alternating run lengths, a run of missing pieces
/// first (possibly empty), plus the pieces being downloaded: their indices for the bar and the
/// list's rows, each with its blocks as one letter per block (`detail::BlockKind::letter`).
fn pieces(e: &Entry, st: &State, msg: &mut Json) -> bool {
    let Some(Reply::Pieces(have, _, downloading)) = st.cached(e.status.id, ReplyKind::Pieces) else {
        return false;
    };
    let mut runs: Vec<Json> = Vec::new();
    let mut value = false;
    let mut run = 0usize;
    for bit in have.iter() {
        if bit == value {
            run += 1;
        } else {
            runs.push(run.into());
            value = bit;
            run = 1;
        }
    }
    runs.push(run.into());
    msg.set("n", have.len());
    msg.set("have", have.count());
    msg.set("rle", Json::Arr(runs));
    msg.set("downloading", Json::Arr(downloading.iter().map(|p| Json::from(p.piece)).collect()));
    let rows: Vec<Json> = downloading
        .iter()
        .map(|p| {
            let cells = (0..detail::PIECE_COLUMNS.len()).map(|c| detail::piece_cell(p, c)).collect();
            let keys = vec![
                p.piece.into(),
                p.size.into(),
                p.blocks.len().into(),
                p.finished().into(),
                p.finished().into(),
            ];
            let blocks: String = p.blocks.iter().map(|&b| detail::block_kind(b).letter()).collect();
            row(cells, keys).with("b", blocks)
        })
        .collect();
    msg.set("rows", Json::Arr(rows));
    true
}

/// The file list with the Files tab's columns, each file's progress in thousandths and its
/// priority, pad files left out; the index is the engine's, for the priority command.
fn files(e: &Entry, st: &State, msg: &mut Json) -> bool {
    let Some(Reply::Files(list)) = st.cached(e.status.id, ReplyKind::Files) else { return false };
    let done: &[u64] = match st.cached(e.status.id, ReplyKind::Progress) {
        Some(Reply::Progress(d)) => d,
        _ => &[],
    };
    let rows: Vec<Json> = list
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.pad_file)
        .map(|(i, f)| {
            let d = done.get(i).copied();
            let cells = (0..detail::FILE_COLUMNS.len()).map(|c| detail::file_cell(f, d, c)).collect();
            let ppm = match d {
                Some(d) if f.size > 0 => ((d as u128 * 1_000_000 / f.size as u128) as u64).min(1_000_000),
                Some(_) => 1_000_000,
                None => 0,
            };
            let keys = vec![
                Json::Null,
                f.size.into(),
                d.map_or(Json::Null, Json::from),
                ppm.into(),
                (f.last_piece - f.first_piece + 1).into(),
                f.first_piece.into(),
                priority_rank(f.priority).into(),
            ];
            row(cells, keys).with("i", i).with("p", ppm / 1000).with("prio", f.priority.0 as i64)
        })
        .collect();
    msg.set("rows", Json::Arr(rows));
    true
}

/// The Files tab's priority order for sorting: Don't Download, Low, Normal, High.
fn priority_rank(p: DownloadPriority) -> i64 {
    match p.0 {
        0 => 0,
        1..=3 => 1,
        4 => 2,
        _ => 3,
    }
}

/// The Speed graph's axis top for a peak rate: 1, 2 or 5 x 10^n kB/s, at least 10 kB/s (the
/// Win32 tab's rule).
fn speed_scale(max: u32) -> u32 {
    let max = max.max(10 * 1024) as u64;
    let mut base = 1024u64;
    loop {
        for m in [1, 2, 5] {
            let top = base * m;
            if top >= max {
                return top.min(u32::MAX as u64) as u32;
            }
        }
        base *= 10;
    }
}

/// The axis top and its labels at the top and at three quarters, half and a quarter.
fn speed_axis(top: u32) -> Json {
    let labels: Vec<Json> =
        (0..4).map(|i| Json::from(fmt::speed_always((top as u64 * (4 - i) / 4) as u32))).collect();
    Json::obj().with("top", top).with("labels", Json::Arr(labels))
}

fn speed_legend(down: u32, up: u32) -> Json {
    Json::Arr(vec![
        format!("Download: {}", fmt::speed_always(down)).into(),
        format!("Upload: {}", fmt::speed_always(up)).into(),
    ])
}

fn status_bar(st: &State) -> Json {
    let s = &st.stats;
    Json::obj()
        .with("dht", s.dht_nodes)
        .with("down", fmt::speed_always(s.download_rate))
        .with("up", fmt::speed_always(s.upload_rate))
        .with("down_total", fmt::size(s.recv_payload_bytes))
        .with("up_total", fmt::size(s.sent_payload_bytes))
        // the global limits in force (empty when unlimited), shown in brackets
        .with("down_limit", fmt::speed(st.settings.download_rate_limit))
        .with("up_limit", fmt::speed(st.settings.upload_rate_limit))
}

fn error(msg: &str) -> Json {
    Json::obj().with("t", "error").with("msg", msg)
}

fn parse_view(msg: &Json) -> Result<View, &'static str> {
    let filter = msg.get("filter");
    let label = filter.and_then(|f| f.get("label")).and_then(Json::as_str);
    let filter_kind = match label {
        Some(l) => Filter::Label(l.to_owned()),
        None => {
            let id = filter.and_then(|f| f.get("category")).and_then(Json::as_str).unwrap_or("all");
            Filter::Category(category_from_id(id).ok_or("unknown category")?)
        }
    };
    let search =
        filter.and_then(|f| f.get("search")).and_then(Json::as_str).unwrap_or("").trim().to_lowercase();
    let sort = match msg.get("sort") {
        None | Some(Json::Null) => None,
        Some(Json::Arr(v)) if v.len() == 2 => {
            let col = v[0].as_str().and_then(column_from_id).ok_or("unknown sort column")?;
            Some((col, v[1].as_bool().unwrap_or(false)))
        }
        _ => return Err("sort must be [column, descending]"),
    };
    let columns = match msg.get("columns") {
        None | Some(Json::Null) => Column::ALL.into_iter().filter(|c| c.default_visible()).collect(),
        Some(Json::Arr(v)) => v
            .iter()
            .map(|c| c.as_str().and_then(column_from_id).ok_or("unknown column"))
            .collect::<Result<Vec<Column>, _>>()?,
        _ => return Err("columns must be a list"),
    };
    let (first, count) = match msg.get("window") {
        None | Some(Json::Null) => (0, DEFAULT_WINDOW),
        Some(Json::Arr(v)) if v.len() == 2 => {
            let first = v[0].as_i64().filter(|&n| n >= 0).ok_or("bad window")? as usize;
            let count = v[1].as_i64().filter(|&n| n >= 0).ok_or("bad window")? as usize;
            (first, count.min(MAX_WINDOW))
        }
        _ => return Err("window must be [first, count]"),
    };
    Ok(View { filter: filter_kind, search, sort, columns, first, count })
}

#[cfg(test)]
mod tests {
    use super::*;
    use patorrent_core::{BlockState, PartialPiece, TorrentFlags, TorrentStatus};

    /// A partial piece of `blocks` blocks, the first `done` on disk, the next one requested.
    fn partial(piece: u32, blocks: usize, done: usize) -> PartialPiece {
        let blocks = (0..blocks)
            .map(|b| match b {
                b if b < done => BlockState::Finished,
                b if b == done => BlockState::Requested(1),
                _ => BlockState::Free,
            })
            .collect();
        PartialPiece { piece, size: 2_000_000, blocks }
    }

    #[test]
    fn column_ids_are_unique_and_snake_case() {
        for c in Column::ALL {
            let id = column_id(c);
            assert!(id.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{id}");
            assert_eq!(column_from_id(id), Some(c));
            assert_eq!(Column::ALL.iter().filter(|d| column_id(**d) == id).count(), 1);
        }
        assert_eq!(column_from_id("dlspeed"), None);
        for c in Category::ALL {
            assert_eq!(category_from_id(category_id(c)), Some(c));
        }
    }

    #[test]
    fn parses_views() {
        let v = json::parse(
            r#"{"t":"view","filter":{"category":"downloading","label":null,"search":" ISO "},
"sort":["down_speed",true],"columns":["name","size"],"window":[10,500]}"#,
        )
        .unwrap();
        let view = parse_view(&v).unwrap();
        assert_eq!(view.filter, Filter::Category(Category::Downloading));
        assert_eq!(view.search, "iso");
        assert_eq!(view.sort, Some((Column::DownSpeed, true)));
        assert_eq!(view.columns, [Column::Name, Column::Size]);
        assert_eq!((view.first, view.count), (10, MAX_WINDOW));
        let v = json::parse(r#"{"t":"view","filter":{"label":"tv"}}"#).unwrap();
        let view = parse_view(&v).unwrap();
        assert_eq!(view.filter, Filter::Label("tv".into()));
        assert_eq!(view.sort, None);
        assert_eq!(view.columns.len(), Column::ALL.iter().filter(|c| c.default_visible()).count());
        assert_eq!((view.first, view.count), (0, DEFAULT_WINDOW));
        for bad in [
            r#"{"filter":{"category":"nope"}}"#,
            r#"{"sort":["dlspeed",true]}"#,
            r#"{"sort":"name"}"#,
            r#"{"columns":["name","x"]}"#,
            r#"{"window":[-1,5]}"#,
            r#"{"window":[0]}"#,
        ] {
            assert!(parse_view(&json::parse(bad).unwrap()).is_err(), "{bad}");
        }
    }

    #[test]
    fn messages_without_a_state() {
        // Only the parts that need no cache: errors.
        let mut c = Client::new();
        let mut st = crate::webui::State::for_tests();
        let r = c.on_message("nope", &mut st);
        assert_eq!(r.len(), 1);
        assert!(r[0].get("msg").and_then(Json::as_str).unwrap().starts_with("bad json"));
        assert_eq!(c.on_message(r#"{"t":"zap"}"#, &mut st)[0].get("t"), Some(&Json::from("error")));
        assert_eq!(c.on_message(r#"{"t":"idle"}"#, &mut st), Vec::<Json>::new());
        assert!(c.is_idle());
        assert!(c.on_tick(&mut st).is_empty(), "idle clients get nothing");
        assert!(c.on_message(r#"{"t":"detail","id":"ABCD","tab":"peers"}"#, &mut st).is_empty());
        assert_eq!(c.detail(), None, "an unknown torrent is not subscribed");
        assert!(c.on_message(r#"{"t":"detail","id":null}"#, &mut st).is_empty());
        assert_eq!(c.detail(), None);
        let hello = c.hello(&st);
        assert_eq!(hello.get("t"), Some(&Json::from("hello")));
        assert_eq!(hello.get("columns").map(|c| matches!(c, Json::Arr(v) if v.len() == 24)), Some(true));
    }

    #[test]
    fn rows_and_ticks_diff() {
        let mut st = crate::webui::State::for_tests();
        let now = 1_792_000_000;
        for i in 0..3u64 {
            let mut s = TorrentStatus {
                id: TorrentId::from_raw(i + 1),
                info_hashes: patorrent_core::InfoHashes::v1(patorrent_core::Sha1Hash([i as u8 + 1; 20])),
                name: format!("t{i}"),
                total_wanted: 1000 * (i + 1),
                download_payload_rate: 100 * (3 - i as u32),
                flags: TorrentFlags::AUTO_MANAGED,
                ..TorrentStatus::default()
            };
            if i == 2 {
                s.is_finished = true;
                s.download_payload_rate = 50;
            }
            st.update(s, now);
        }
        st.tick += 1;
        let mut c = Client::new();
        let r = c.on_message(
            r#"{"t":"view","sort":["down_speed",true],"columns":["name","size","status","down_speed"],"window":[0,2]}"#,
            &mut st,
        );
        assert_eq!(r.len(), 1);
        let rows = &r[0];
        assert_eq!(rows.get("t"), Some(&Json::from("rows")));
        assert_eq!(rows.get("total"), Some(&Json::from(3i64)));
        let Some(Json::Arr(ids)) = rows.get("ids") else { panic!("{}", rows.to_json()) };
        assert_eq!(ids.len(), 2);
        let h1 = patorrent_core::Sha1Hash([1; 20]).to_hex();
        let h2 = patorrent_core::Sha1Hash([2; 20]).to_hex();
        let h3 = patorrent_core::Sha1Hash([3; 20]).to_hex();
        assert_eq!(ids[0], Json::from(h1.as_str()), "fastest first");
        assert_eq!(ids[1], Json::from(h2.as_str()));
        let cell = rows.get("cells").unwrap().get(&h1).unwrap();
        assert_eq!(
            cell.get("c"),
            Some(&Json::Arr(vec!["t0".into(), "1000 B".into(), "Downloading".into(), "0.3 kB/s".into()]))
        );
        assert_eq!(cell.get("state"), Some(&Json::from("downloading")));
        assert!(rows.get("cells").unwrap().get(&h3).is_none(), "outside the window");

        // nothing changed: a tick without ids or cells
        let t = c.on_tick(&mut st).remove(0);
        assert_eq!(t.get("t"), Some(&Json::from("tick")));
        assert!(t.get("ids").is_none() && t.get("cells").is_none(), "{}", t.to_json());
        let counts = t.get("counts").unwrap();
        assert_eq!(counts.get("all"), Some(&Json::from(3i64)));
        assert_eq!(counts.get("completed"), Some(&Json::from(1i64)));
        assert_eq!(counts.get("active"), Some(&Json::from(3i64)));
        assert!(t.get("status").unwrap().get("down").is_some());

        // one row changes and drops out of the window: the order is re-sent, and the cells
        // hold only the row that is new to the window
        let mut s = st.torrents[&TorrentId::from_raw(2)].status.clone();
        s.flags = TorrentFlags::PAUSED;
        s.download_payload_rate = 0;
        st.update(s, now);
        let t = c.on_tick(&mut st).remove(0);
        assert_eq!(
            t.get("ids"),
            Some(&Json::Arr(vec![h1.as_str().into(), h3.as_str().into()])),
            "{}",
            t.to_json()
        );
        let cells = t.get("cells").unwrap();
        assert_eq!(cells.entries().len(), 1);
        assert_eq!(cells.get(&h3).unwrap().get("state"), Some(&Json::from("seeding")));
        assert_eq!(t.get("counts").unwrap().get("active"), Some(&Json::from(2i64)));

        // a wider window: a change to one row sends only that row, and no ids when the order
        // stays
        let r = c.on_message(
            r#"{"t":"view","sort":["down_speed",true],"columns":["name","status"],"window":[0,10]}"#,
            &mut st,
        );
        assert_eq!(r[0].get("ids").map(|v| matches!(v, Json::Arr(v) if v.len() == 3)), Some(true));
        let mut s = st.torrents[&TorrentId::from_raw(2)].status.clone();
        s.error = "disk full".into();
        st.update(s, now);
        let t = c.on_tick(&mut st).remove(0);
        assert!(t.get("ids").is_none(), "{}", t.to_json());
        let cells = t.get("cells").unwrap();
        assert_eq!(cells.entries().len(), 1);
        assert_eq!(
            cells.get(&h2).unwrap().get("c"),
            Some(&Json::Arr(vec!["t1".into(), "Error: disk full".into()]))
        );
        assert_eq!(cells.get(&h2).unwrap().get("state"), Some(&Json::from("error")));
        let t = c.on_tick(&mut st).remove(0);
        assert!(t.get("cells").is_none(), "sent once: {}", t.to_json());

        // a label through the category map bumps the version and shows in the cell
        let r = c.on_message(r#"{"t":"view","columns":["label"],"window":[0,10]}"#, &mut st);
        assert_eq!(r[0].get("cells").unwrap().get(&h1).unwrap().get("c"), Some(&Json::Arr(vec!["".into()])));
        st.torrent_categories.insert(h1.clone(), "linux".into());
        st.touch(&h1);
        let t = c.on_tick(&mut st).remove(0);
        assert_eq!(
            t.get("cells").unwrap().get(&h1).unwrap().get("c"),
            Some(&Json::Arr(vec!["linux".into()]))
        );
        // the tree's Labels branch: counts with the tick that changed them, then nothing
        let pair = |l: &str, n: usize| Json::Arr(vec![l.into(), n.into()]);
        assert_eq!(t.get("labels"), Some(&Json::Arr(vec![pair("linux", 1)])));
        assert_eq!(t.get("no_label"), Some(&Json::from(2usize)));
        assert!(c.on_tick(&mut st)[0].get("labels").is_none());
        // a label from the category map with no torrent is listed with 0
        st.categories.insert("tv".into(), String::new());
        let t = c.on_tick(&mut st).remove(0);
        assert_eq!(t.get("labels"), Some(&Json::Arr(vec![pair("linux", 1), pair("tv", 0)])));
        // the "No Label" filter
        let r = c.on_message(r#"{"t":"view","filter":{"label":""},"columns":["name"]}"#, &mut st);
        assert_eq!(r[0].get("total"), Some(&Json::from(2i64)));
        // a search narrows the total
        let r = c.on_message(r#"{"t":"view","filter":{"search":"T1"},"columns":["name"]}"#, &mut st);
        assert_eq!(r[0].get("total"), Some(&Json::from(1i64)));
        // a removal leaves the list
        st.torrents.remove(&TorrentId::from_raw(1));
        st.by_hash.remove(&h1);
        st.removals += 1;
        let r = c.on_message(r#"{"t":"view","columns":["name"]}"#, &mut st);
        assert_eq!(r[0].get("total"), Some(&Json::from(2i64)));
    }

    fn rows_of(m: &Json) -> Vec<Vec<String>> {
        let Some(Json::Arr(rows)) = m.get("rows") else { panic!("no rows: {}", m.to_json()) };
        rows.iter()
            .map(|r| match r.get("c") {
                Some(Json::Arr(c)) => c.iter().map(|x| x.as_str().unwrap().to_owned()).collect(),
                _ => panic!("{}", r.to_json()),
            })
            .collect()
    }

    /// A cache with one torrent and every reply the tabs read, built by hand.
    fn detail_state() -> (crate::webui::State, String) {
        use patorrent_core::{
            AnnounceEntry, Bitfield, DownloadPriority, FileEntry, PeerInfo, PeerSource, TrackerStatus,
        };
        let mut st = crate::webui::State::for_tests();
        let id = TorrentId::from_raw(7);
        let s = TorrentStatus {
            id,
            info_hashes: patorrent_core::InfoHashes::v1(patorrent_core::Sha1Hash([0xab; 20])),
            name: "debian.iso".into(),
            save_path: "C:\\dl".into(),
            has_metadata: true,
            progress_ppm: 250_000,
            download_payload_rate: 2048,
            upload_payload_rate: 512,
            num_seeds: 3,
            num_peers: 8,
            list_seeds: 10,
            list_peers: 40,
            all_time_download: 4_000_000,
            all_time_upload: 1_000_000,
            total: 16_000_000,
            total_wanted: 16_000_000,
            total_wanted_done: 4_000_000,
            num_pieces: 2,
            pieces_total: 8,
            piece_length: 2_000_000,
            added_time: 1_792_075_632,
            active_duration: 125,
            flags: TorrentFlags::AUTO_MANAGED,
            ..TorrentStatus::default()
        };
        st.update(s, 1_792_075_700);
        let hash = patorrent_core::Sha1Hash([0xab; 20]).to_hex();
        st.meta.insert(
            hash.clone(),
            crate::webui::Meta {
                comment: "a comment".into(),
                created_by: "mktorrent".into(),
                creation_date: Some(951_782_400),
                private: false,
            },
        );
        let files: Vec<FileEntry> = vec![
            FileEntry {
                path: "d/a.iso".into(),
                size: 10_000_000,
                offset: 0,
                first_piece: 0,
                last_piece: 4,
                pad_file: false,
                priority: DownloadPriority::NORMAL,
            },
            FileEntry {
                path: ".pad/1".into(),
                size: 100,
                offset: 10_000_000,
                first_piece: 4,
                last_piece: 5,
                pad_file: true,
                priority: DownloadPriority::NORMAL,
            },
            FileEntry {
                path: "d/b.txt".into(),
                size: 6_000_000,
                offset: 10_000_100,
                first_piece: 5,
                last_piece: 7,
                pad_file: false,
                priority: DownloadPriority::DONT_DOWNLOAD,
            },
        ];
        st.reply(id, ReplyKind::Files, Reply::Files(files.into()));
        st.reply(id, ReplyKind::Progress, Reply::Progress(vec![4_000_000, 0, 0]));
        st.reply(
            id,
            ReplyKind::Trackers,
            Reply::Trackers(vec![
                AnnounceEntry {
                    url: "udp://t.example:1/announce".into(),
                    status: TrackerStatus::Working,
                    next_announce: Some(125),
                    scrape_complete: Some(12),
                    scrape_incomplete: Some(3),
                    ..AnnounceEntry::default()
                },
                AnnounceEntry {
                    url: "http://b.example/announce".into(),
                    status: TrackerStatus::Error,
                    message: "timed out".into(),
                    ..AnnounceEntry::default()
                },
            ]),
        );
        let mut have = Bitfield::new(8, false);
        have.set(1, true);
        have.set(2, true);
        st.reply(
            id,
            ReplyKind::Pieces,
            Reply::Pieces(have, vec![1; 8], vec![partial(0, 4, 1), partial(3, 4, 3)]),
        );
        st.reply(
            id,
            ReplyKind::Peers,
            Reply::Peers(vec![PeerInfo {
                ip: "192.0.2.5:6881".parse().unwrap(),
                client: "qBittorrent 5.0.2".into(),
                flags: patorrent_core::PeerFlags::INTERESTING | patorrent_core::PeerFlags::RC4_ENCRYPTED,
                source: PeerSource::DHT,
                progress_ppm: 500_000,
                down_speed: 1024,
                up_speed: 0,
                total_download: 2048,
                total_upload: 10,
                download_queue_length: 3,
                upload_queue_length: 0,
                num_hashfails: 0,
                is_web_seed: false,
            }]),
        );
        (st, hash)
    }

    #[test]
    fn hello_carries_the_detail_columns() {
        let st = crate::webui::State::for_tests();
        let hello = Client::new().hello(&st);
        let d = hello.get("detail").unwrap();
        let Some(Json::Arr(peers)) = d.get("peers") else { panic!() };
        assert_eq!(peers.len(), detail::PEER_COLUMNS.len());
        assert_eq!(peers[0].get("title"), Some(&Json::from("IP")));
        assert_eq!(peers[3].get("right"), Some(&Json::from(true)));
        assert_eq!(d.get("trackers").map(|t| matches!(t, Json::Arr(v) if v.len() == 6)), Some(true));
        assert_eq!(d.get("files").map(|t| matches!(t, Json::Arr(v) if v.len() == 7)), Some(true));
    }

    #[test]
    fn general_tab_pairs() {
        let (mut st, hash) = detail_state();
        let mut c = Client::new();
        let r = c.on_message(&format!(r#"{{"t":"detail","id":"{hash}"}}"#), &mut st);
        assert_eq!(r.len(), 1, "{r:?}");
        let m = &r[0];
        assert_eq!(m.get("t"), Some(&Json::from("detail")));
        assert_eq!(m.get("id"), Some(&Json::from(hash.as_str())));
        assert_eq!(m.get("tab"), Some(&Json::from("general")));
        let Some(Json::Arr(bars)) = m.get("bars") else { panic!() };
        assert_eq!(bars[0], Json::Arr(vec!["Downloaded:".into(), "25.0 %".into(), 250i64.into()]));
        let Some(Json::Arr(groups)) = m.get("groups") else { panic!() };
        assert_eq!(groups[0].get("title"), Some(&Json::from("Transfer")));
        assert_eq!(groups[1].get("title"), Some(&Json::from("General")));
        let pairs = |g: &Json| -> Vec<(String, String)> {
            let Some(Json::Arr(rows)) = g.get("rows") else { panic!() };
            rows.iter()
                .map(|r| match r {
                    Json::Arr(v) => (v[0].as_str().unwrap().to_owned(), v[1].as_str().unwrap().to_owned()),
                    _ => panic!(),
                })
                .collect()
        };
        let t = pairs(&groups[0]);
        let get =
            |p: &[(String, String)], l: &str| p.iter().find(|(k, _)| k == l).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(get(&t, "Time Elapsed:"), "2m 5s");
        assert_eq!(get(&t, "Remaining:"), "1h 37m", "12 MB left at 2 kB/s");
        assert_eq!(get(&t, "Downloaded:"), "3.81 MB");
        assert_eq!(get(&t, "Uploaded:"), "977 kB");
        assert_eq!(get(&t, "Seeds:"), "3 of 10 connected");
        assert_eq!(get(&t, "Peers:"), "5 of 30 connected");
        assert_eq!(get(&t, "Download Speed:"), "2.0 kB/s");
        assert_eq!(get(&t, "Share Ratio:"), "0.250");
        assert_eq!(get(&t, "Down Limit:"), "\u{221e}");
        assert_eq!(get(&t, "Status:"), "Downloading");
        let g = pairs(&groups[1]);
        assert_eq!(get(&g, "Save As:"), "C:\\dl");
        assert_eq!(get(&g, "Total Size:"), "15.3 MB");
        assert_eq!(get(&g, "Remaining:"), "11.4 MB");
        assert_eq!(get(&g, "Pieces:"), "8 x 1.91 MB (have 2)");
        assert_eq!(get(&g, "Added On:"), "2026-10-15 14:47:12");
        assert_eq!(get(&g, "Created On:"), "2000-02-29 00:00:00");
        assert_eq!(get(&g, "Created By:"), "mktorrent");
        assert_eq!(get(&g, "Comment:"), "a comment");
        assert_eq!(get(&g, "Hash:"), hash.to_uppercase());
        assert_eq!(get(&g, "Trackers:"), "2");
        // unchanged: a tick carries no detail message
        assert!(c.on_tick(&mut st).is_empty(), "no view, nothing changed");
        // a status change: sent again, once
        let mut s = st.torrents[&TorrentId::from_raw(7)].status.clone();
        s.upload_payload_rate = 4096;
        st.update(s, 1_792_075_701);
        let r = c.on_tick(&mut st);
        assert_eq!(r.len(), 1);
        let t =
            pairs(&r[0].get("groups").unwrap().get("rows").map(|_| unreachable!()).unwrap_or_else(|| {
                let Some(Json::Arr(g)) = r[0].get("groups") else { panic!() };
                g[0].clone()
            }));
        assert_eq!(get(&t, "Upload Speed:"), "4.0 kB/s");
        assert!(c.on_tick(&mut st).is_empty());
        // the same subscription again: nothing (the page has it); another tab: content at once
        assert!(
            c.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"general"}}"#), &mut st).is_empty()
        );
        let r = c.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"pieces"}}"#), &mut st);
        assert_eq!(r[0].get("tab"), Some(&Json::from("pieces")));
        assert_eq!(c.detail(), Some((hash.as_str(), "pieces")));
        // unsubscribe
        assert!(c.on_message(r#"{"t":"detail","id":null}"#, &mut st).is_empty());
        assert_eq!(c.detail(), None);
        assert!(c.on_tick(&mut st).is_empty());
        assert_eq!(
            c.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"nope"}}"#), &mut st)[0].get("t"),
            Some(&Json::from("error"))
        );
    }

    #[test]
    fn table_tabs_rows_and_keys() {
        let (mut st, hash) = detail_state();
        let mut c = Client::new();
        let r = c.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"trackers"}}"#), &mut st);
        let rows = rows_of(&r[0]);
        assert_eq!(rows[0], ["udp://t.example:1/announce", "Working", "2m 5s", "12", "3", ""]);
        assert_eq!(rows[1][1], "timed out");
        let Some(Json::Arr(k)) = r[0].get("rows").unwrap().get("k").or_else(|| {
            let Json::Arr(v) = r[0].get("rows").unwrap() else { panic!() };
            v[0].get("k")
        }) else {
            panic!()
        };
        assert_eq!(k, &vec![Json::Null, Json::Null, 125u32.into(), 12u32.into(), 3u32.into(), Json::Null]);

        let r = c.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"peers"}}"#), &mut st);
        let rows = rows_of(&r[0]);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0],
            [
                "192.0.2.5:6881",
                "qBittorrent 5.0.2",
                "DEH",
                "50.0",
                "1.0 kB/s",
                "",
                "3 | 0",
                "2.00 kB",
                "10 B"
            ]
        );
        assert!(c.on_tick(&mut st).is_empty(), "the same peers again: nothing");
        st.reply(TorrentId::from_raw(7), ReplyKind::Peers, Reply::Peers(Vec::new()));
        let r = c.on_tick(&mut st);
        assert_eq!(rows_of(&r[0]).len(), 0, "the peer left: sent once");
        assert!(c.on_tick(&mut st).is_empty());

        let r = c.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"files"}}"#), &mut st);
        let rows = rows_of(&r[0]);
        assert_eq!(rows.len(), 2, "pad files are left out");
        assert_eq!(rows[0], ["d/a.iso", "9.54 MB", "3.81 MB", "40.0 %", "5", "0", "Normal"]);
        assert_eq!(rows[1][6], "Don't Download");
        let Json::Arr(raw) = r[0].get("rows").unwrap() else { panic!() };
        assert_eq!(raw[0].get("i"), Some(&Json::from(0usize)));
        assert_eq!(raw[1].get("i"), Some(&Json::from(2usize)), "the engine's index, pads counted");
        assert_eq!(raw[0].get("p"), Some(&Json::from(400u64)));
        assert_eq!(raw[1].get("prio"), Some(&Json::from(0i64)));
        assert_eq!(
            raw[0].get("k").unwrap(),
            &Json::Arr(vec![
                Json::Null,
                10_000_000u64.into(),
                4_000_000u64.into(),
                400_000u64.into(),
                5u32.into(),
                0u32.into(),
                2i64.into()
            ])
        );
        // a reply not in the cache yet: nothing until it lands
        let mut c2 = Client::new();
        let mut st2 = crate::webui::State::for_tests();
        st2.update(st.torrents[&TorrentId::from_raw(7)].status.clone(), 1);
        assert!(
            c2.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"files"}}"#), &mut st2).is_empty()
        );
        assert_eq!(c2.detail(), Some((hash.as_str(), "files")));
        assert!(c2.on_tick(&mut st2).is_empty());
        st2.reply(TorrentId::from_raw(7), ReplyKind::Files, Reply::Files(Vec::new().into()));
        let r = c2.on_tick(&mut st2);
        assert_eq!(rows_of(&r[0]).len(), 0);
    }

    #[test]
    fn pieces_run_length_and_downloading() {
        let (mut st, hash) = detail_state();
        let mut c = Client::new();
        let r = c.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"pieces"}}"#), &mut st);
        let m = &r[0];
        assert_eq!(m.get("n"), Some(&Json::from(8usize)));
        assert_eq!(m.get("have"), Some(&Json::from(2usize)));
        assert_eq!(m.get("rle"), Some(&Json::Arr(vec![1usize.into(), 2usize.into(), 5usize.into()])));
        assert_eq!(m.get("downloading"), Some(&Json::Arr(vec![0u32.into(), 3u32.into()])));
        // the list: the frontend's cells, sort keys and one letter per block
        assert_eq!(rows_of(m), [["0", "1.91 MB", "4", "", "1"], ["3", "1.91 MB", "4", "", "3"]]);
        let Some(Json::Arr(rows)) = m.get("rows") else { panic!() };
        assert_eq!(rows[0].get("b"), Some(&Json::from("fr--")));
        assert_eq!(rows[1].get("b"), Some(&Json::from("fffr")));
        assert!(c.on_tick(&mut st).is_empty());
        let mut all = patorrent_core::Bitfield::new(8, true);
        all.set(7, false);
        st.reply(
            TorrentId::from_raw(7),
            ReplyKind::Pieces,
            Reply::Pieces(all, vec![1; 8], vec![partial(7, 3, 0)]),
        );
        let r = c.on_tick(&mut st);
        assert_eq!(r[0].get("rle"), Some(&Json::Arr(vec![0usize.into(), 7usize.into(), 1usize.into()])));
        assert_eq!(rows_of(&r[0]), [["7", "1.91 MB", "3", "", "0"]]);
    }

    #[test]
    fn speed_ring_whole_then_appended() {
        let (mut st, hash) = detail_state();
        let id = TorrentId::from_raw(7);
        let mut c = Client::new();
        // the ring starts with the subscription: empty, with the axis
        let r = c.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"speed"}}"#), &mut st);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].get("all"), Some(&Json::from(true)));
        assert_eq!(r[0].get("samples"), Some(&Json::Arr(vec![])));
        assert_eq!(r[0].get("keep"), Some(&Json::from(SPEED_SAMPLES)));
        let scale = r[0].get("scale").unwrap();
        assert_eq!(scale.get("top"), Some(&Json::from(10 * 1024u32)));
        assert_eq!(scale.get("labels").unwrap().get("x"), None);
        let Some(Json::Arr(labels)) = scale.get("labels") else { panic!() };
        assert_eq!(labels[0], Json::from("10.0 kB/s"));
        assert_eq!(labels[2], Json::from("5.0 kB/s"));
        assert!(st.speed.contains_key(&id));
        assert!(c.on_tick(&mut st).is_empty(), "no sample yet");
        // a status tick samples the rates: one appended sample per tick
        st.tick += 1;
        st.sample_speeds();
        let r = c.on_tick(&mut st);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].get("all"), None);
        assert_eq!(
            r[0].get("samples"),
            Some(&Json::Arr(vec![Json::Arr(vec![2048u32.into(), 512u32.into()])]))
        );
        assert_eq!(r[0].get("scale"), None, "the axis did not change");
        assert_eq!(
            r[0].get("legend"),
            Some(&Json::Arr(vec!["Download: 2.0 kB/s".into(), "Upload: 0.5 kB/s".into()]))
        );
        // a peak rescales the axis
        let mut s = st.torrents[&id].status.clone();
        s.download_payload_rate = 300 * 1024;
        st.update(s, 2);
        st.tick += 1;
        st.sample_speeds();
        let r = c.on_tick(&mut st);
        assert_eq!(r[0].get("scale").unwrap().get("top"), Some(&Json::from(500 * 1024u32)));
        // a second subscriber gets the whole ring
        let mut c2 = Client::new();
        let r = c2.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"speed"}}"#), &mut st);
        assert_eq!(r[0].get("samples").map(|v| matches!(v, Json::Arr(v) if v.len() == 2)), Some(true));
        // the ring is dropped once nobody looked for 10 minutes of ticks
        c.on_message(r#"{"t":"detail","id":null}"#, &mut st);
        for _ in 0..600 {
            st.tick += 1;
            st.sample_speeds();
        }
        assert!(
            st.speed.contains_key(&id),
            "c2 did not look again, but the idle count starts at its last look"
        );
        st.tick += 1;
        st.sample_speeds();
        assert!(!st.speed.contains_key(&id));
        assert_eq!(speed_scale(0), 10 * 1024);
        assert_eq!(speed_scale(10 * 1024 + 1), 20 * 1024);
        assert_eq!(speed_scale(3 * 1024 * 1024), 5 * 1_024_000, "5000 kB/s");
    }

    #[test]
    fn logger_tail_then_new_lines() {
        let (mut st, hash) = detail_state();
        let id = TorrentId::from_raw(7);
        st.push_log(None, "listening on 0.0.0.0:6881".into(), 100);
        st.push_log(Some(id), "debian.iso: added".into(), 101);
        st.push_log(Some(TorrentId::from_raw(9)), "other: added".into(), 102);
        let mut c = Client::new();
        let r = c.on_message(&format!(r#"{{"t":"detail","id":"{hash}","tab":"logger"}}"#), &mut st);
        assert_eq!(r[0].get("all"), Some(&Json::from(true)));
        assert_eq!(
            r[0].get("lines"),
            Some(&Json::Arr(vec![
                Json::Arr(vec![100i64.into(), "listening on 0.0.0.0:6881".into()]),
                Json::Arr(vec![101i64.into(), "debian.iso: added".into()]),
            ])),
            "session-wide lines and the torrent's own, not another torrent's"
        );
        assert!(c.on_tick(&mut st).is_empty());
        st.push_log(Some(id), "debian.iso: finished".into(), 103);
        let r = c.on_tick(&mut st);
        assert_eq!(r[0].get("all"), None);
        assert_eq!(
            r[0].get("lines"),
            Some(&Json::Arr(vec![Json::Arr(vec![103i64.into(), "debian.iso: finished".into()])]))
        );
        assert!(c.on_tick(&mut st).is_empty());
        // the ring keeps LOG_LINES lines
        for i in 0..crate::webui::LOG_LINES + 5 {
            st.push_log(None, format!("line {i}"), 200);
        }
        assert_eq!(st.log.len(), crate::webui::LOG_LINES);
        assert_eq!(st.log.front().unwrap().text, "line 5");
        // a torrent removed while subscribed: said once
        st.torrents.remove(&id);
        st.by_hash.remove(&hash);
        st.removals += 1;
        let r = c.on_tick(&mut st);
        assert_eq!(r[0].get("gone"), Some(&Json::from(true)));
        assert_eq!(c.detail(), None);
    }
}
