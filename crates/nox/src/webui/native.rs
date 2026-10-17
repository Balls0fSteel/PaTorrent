//! The native session protocol over the WebSocket (docs/WEBUI.md "Messages"): one [`Client`]
//! per socket holds that client's `frontend::model::TorrentList` (fed from the shared
//! [`State`] cache, never from the engine), its `view` subscription (filter, sort, columns,
//! window) and what it was last sent, and builds the `hello`, `rows` and `tick` messages.
//!
//! Cells are the strings `frontend::columns` gives the Win32 list, so the page shows exactly
//! what the desktop shows. Nothing here does I/O; `server.rs` moves the frames.

use std::collections::HashMap;

use patorrent_core::{TorrentId, VERSION};
use patorrent_frontend::columns::{Align, Column};
use patorrent_frontend::fmt;
use patorrent_frontend::model::{Filter, TorrentList};
use patorrent_frontend::status::{Category, status_class};

use super::json::{self, Json};
use super::{Entry, State};

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

fn category_from_id(id: &str) -> Option<Category> {
    Category::ALL.into_iter().find(|c| category_id(*c) == id)
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
    /// The detail subscription (torrent id, tab); content comes with slice 4.
    detail: Option<(String, String)>,
    /// The last version sent per torrent in the window.
    sent: HashMap<TorrentId, u64>,
    /// The window's order as last sent.
    last_ids: Vec<TorrentId>,
    /// The filtered, sorted view with the search applied.
    rows: Vec<TorrentId>,
    rows_stale: bool,
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
        }
    }

    pub fn detail(&self) -> Option<(&str, &str)> {
        self.detail.as_ref().map(|(id, tab)| (id.as_str(), tab.as_str()))
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
        Json::obj()
            .with("t", "hello")
            .with("version", VERSION)
            .with("columns", Json::Arr(columns))
            .with("categories", Json::Arr(categories))
            .with("labels", Json::Arr(labels))
    }

    /// Handles one client message; the replies to send back, in order.
    pub(crate) fn on_message(&mut self, text: &str, st: &State) -> Vec<Json> {
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
                let id = msg.get("id").and_then(Json::as_str).map(str::to_ascii_lowercase);
                let tab = msg.get("tab").and_then(Json::as_str).unwrap_or("general").to_owned();
                self.detail = id.map(|id| (id, tab));
                Vec::new()
            }
            Some("idle") => {
                self.idle = true;
                Vec::new()
            }
            Some(t) => vec![error(&format!("unknown message type {t:?}"))],
            None => vec![error("missing t")],
        }
    }

    /// Once per status tick: what changed for this client, or nothing while it is idle or has
    /// no view.
    pub(crate) fn on_tick(&mut self, st: &State) -> Option<Json> {
        if self.idle || self.view.is_none() {
            return None;
        }
        self.sync(st);
        Some(self.tick_message(st))
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
        out.set("status", status_bar(st));
        out
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

fn status_bar(st: &State) -> Json {
    let s = &st.stats;
    Json::obj()
        .with("dht", s.dht_nodes)
        .with("down", fmt::speed_always(s.download_rate))
        .with("up", fmt::speed_always(s.upload_rate))
        .with("down_total", fmt::size(s.recv_payload_bytes))
        .with("up_total", fmt::size(s.sent_payload_bytes))
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
    use patorrent_core::{TorrentFlags, TorrentStatus};

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
        let st = crate::webui::State::for_tests();
        let r = c.on_message("nope", &st);
        assert_eq!(r.len(), 1);
        assert!(r[0].get("msg").and_then(Json::as_str).unwrap().starts_with("bad json"));
        assert_eq!(c.on_message(r#"{"t":"zap"}"#, &st)[0].get("t"), Some(&Json::from("error")));
        assert_eq!(c.on_message(r#"{"t":"idle"}"#, &st), Vec::<Json>::new());
        assert!(c.is_idle());
        assert!(c.on_tick(&st).is_none(), "idle clients get nothing");
        assert!(c.on_message(r#"{"t":"detail","id":"ABCD","tab":"peers"}"#, &st).is_empty());
        assert_eq!(c.detail(), Some(("abcd", "peers")));
        assert!(c.on_message(r#"{"t":"detail","id":null}"#, &st).is_empty());
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
            &st,
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
        let t = c.on_tick(&st).unwrap();
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
        let t = c.on_tick(&st).unwrap();
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
            &st,
        );
        assert_eq!(r[0].get("ids").map(|v| matches!(v, Json::Arr(v) if v.len() == 3)), Some(true));
        let mut s = st.torrents[&TorrentId::from_raw(2)].status.clone();
        s.error = "disk full".into();
        st.update(s, now);
        let t = c.on_tick(&st).unwrap();
        assert!(t.get("ids").is_none(), "{}", t.to_json());
        let cells = t.get("cells").unwrap();
        assert_eq!(cells.entries().len(), 1);
        assert_eq!(
            cells.get(&h2).unwrap().get("c"),
            Some(&Json::Arr(vec!["t1".into(), "Error: disk full".into()]))
        );
        assert_eq!(cells.get(&h2).unwrap().get("state"), Some(&Json::from("error")));
        let t = c.on_tick(&st).unwrap();
        assert!(t.get("cells").is_none(), "sent once: {}", t.to_json());

        // a label through the category map bumps the version and shows in the cell
        let r = c.on_message(r#"{"t":"view","columns":["label"],"window":[0,10]}"#, &st);
        assert_eq!(r[0].get("cells").unwrap().get(&h1).unwrap().get("c"), Some(&Json::Arr(vec!["".into()])));
        st.torrent_categories.insert(h1.clone(), "linux".into());
        st.touch(&h1);
        let t = c.on_tick(&st).unwrap();
        assert_eq!(
            t.get("cells").unwrap().get(&h1).unwrap().get("c"),
            Some(&Json::Arr(vec!["linux".into()]))
        );
        // a search narrows the total
        let r = c.on_message(r#"{"t":"view","filter":{"search":"T1"},"columns":["name"]}"#, &st);
        assert_eq!(r[0].get("total"), Some(&Json::from(1i64)));
        // a removal leaves the list
        st.torrents.remove(&TorrentId::from_raw(1));
        st.by_hash.remove(&h1);
        st.removals += 1;
        let r = c.on_message(r#"{"t":"view","columns":["name"]}"#, &st);
        assert_eq!(r[0].get("total"), Some(&Json::from(2i64)));
    }
}
