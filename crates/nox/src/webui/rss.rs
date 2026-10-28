//! The RSS Downloader in nox: µTorrent 2.2.1's downloader (`patorrent_frontend::rss`) run on
//! the status tick, `rss.dat` in the config directory, the native `/pa/rss...` endpoints the
//! page's RSS Downloader dialog uses, and the qBittorrent shim's `rss/*` endpoints mapped onto
//! the same model (written from qBittorrent's published WebAPI documentation only, D15).
//!
//! Threads and locks: the downloader has its own lock ([`Shared::rss`]). Feeds and `.torrent`
//! files are fetched on the worker's threads; the alert loop's status tick collects them
//! ([`tick`]) and adds what came back through [`commands::add`]. Neither the tick nor a
//! request ever fetches while holding a lock, and the order is always the RSS lock, then the
//! State lock (never the other way round). Every change bumps `State::rss_version`, which the
//! WebSocket's `tick` carries (`rss`), so an open dialog reloads.
//!
//! Hosted by a desktop client (`Config::hosted`, D40) there is no downloader here: the RSS
//! Downloader is the host's, the native endpoints and the shim's `rss/*` answer 404
//! ([`HOSTED_NO_RSS`]), the tick does not run and carries no `rss`, and the page hides its
//! entry points (`hello.hosted`).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, MutexGuard};

use patorrent_core::fetch::FetchProxy;
use patorrent_frontend::rss::view::{self, quality_text};
use patorrent_frontend::rss::worker::{Fetcher, Net};
use patorrent_frontend::rss::{
    AddRequest, Download, Feed, FeedId, FilterId, MIN_INTERVALS, Quality, RssDownloader, RssFilter, Worker,
    split_cookie,
};
use patorrent_frontend::{lang, tr};

use super::api::Request;
use super::commands::{self, AddOptions, CmdError, Source, valid_label};
use super::http::{Response, parse_urlencoded};
use super::json::{self, Json};
use super::{Shared, unix_now};

/// The 404's reason while a desktop client hosts the Web UI.
pub(crate) const HOSTED_NO_RSS: &str = "the RSS Downloader is the desktop client's";

/// The downloader and what nox keeps around it.
pub(crate) struct Rss {
    pub d: RssDownloader,
    pub worker: Worker,
    /// `rss.dat`; `None` without a config directory (nothing is saved).
    pub file: Option<PathBuf>,
    pub dirty: bool,
    /// Articles the shim's `rss/markAsRead` marked, as (feed, item id); not saved.
    pub read: HashSet<(FeedId, String)>,
}

impl Rss {
    pub fn load(file: Option<PathBuf>) -> Rss {
        let d = file.as_deref().map(RssDownloader::load).unwrap_or_default();
        // the tick collects the results; nothing needs waking
        let worker = Worker::with_fetcher(Arc::new(Quiet), || {});
        Rss { d, worker, file, dirty: false, read: HashSet::new() }
    }

    fn save(&mut self) -> Option<String> {
        self.dirty = false;
        let file = self.file.as_ref()?;
        self.d.save(file).err().map(|e| tr!(LogNotSaved, file = file.display(), error = e))
    }
}

/// The downloader's fetches ([`Net`]) with one error text whatever went wrong: a feed's error
/// and the Logger's lines show in the Web UI, and the detail (refused, timed out, an HTTP
/// status, not a feed, a redirect's target) would tell a login which addresses and ports
/// answer (SEC-05, SEC-26). The detail goes to the daemon's own output.
struct Quiet;

impl Fetcher for Quiet {
    fn feed(&self, url: &str, proxy: Option<FetchProxy>) -> Result<Feed, String> {
        Net.feed(url, proxy).map_err(|e| not_fetched(split_cookie(url).0, &e))
    }

    fn download(&self, req: &AddRequest, proxy: Option<FetchProxy>) -> Result<Download, String> {
        Net.download(req, proxy).map_err(|e| not_fetched(&req.url, &e))
    }
}

fn not_fetched(url: &str, why: &str) -> String {
    eprintln!("! RSS: could not fetch {url}: {why}");
    format!("could not fetch {url}")
}

fn err(status: u16, msg: impl Into<String>) -> CmdError {
    CmdError { status, msg: msg.into() }
}

fn proxy(shared: &Shared) -> Option<FetchProxy> {
    FetchProxy::from_settings(&shared.lock().settings)
}

/// Lines for the Logger, and a bump of the version the page watches.
fn after_change(shared: &Shared, log: Vec<String>, changed: bool) {
    if log.is_empty() && !changed {
        return;
    }
    let mut st = shared.lock();
    let now = unix_now();
    for line in log {
        st.push_log(None, line, now);
    }
    if changed {
        st.rss_version += 1;
    }
}

/// Once per status tick: the finished fetches into the downloader, the refreshes that are
/// due, `rss.dat` when it changed, and the torrents to add.
pub(crate) fn tick(shared: &Shared) {
    let now = unix_now();
    let proxy = proxy(shared);
    let mut guard = shared.rss();
    let r = &mut *guard;
    let c = r.worker.collect(&mut r.d, now, proxy.as_ref());
    let mut changed = c.changed;
    let mut log = c.log;
    if r.d.next_due().is_some_and(|t| t <= now) {
        changed |= r.worker.refresh_due(&r.d, now, proxy.as_ref()) > 0;
    }
    r.dirty |= c.changed;
    if r.dirty {
        log.extend(r.save());
    }
    drop(guard);
    after_change(shared, log, changed);
    for (req, d) in c.adds {
        add(shared, req, d);
    }
}

/// Adds a torrent the downloader asked for, with its label, "highest priority" (top of the
/// queue) and "paused state" (the page's Pause).
fn add(shared: &Shared, req: AddRequest, d: Download) {
    let now = unix_now();
    let source = match d {
        Download::Magnet(m) => Source::Magnet(m),
        Download::Torrent(b) => match Source::from_torrent_bytes(b) {
            Ok(s) => s,
            Err(e) => {
                let line = tr!(RssLogError, name = req.title, error = e.msg);
                shared.lock().push_log(None, line, now);
                return;
            }
        },
    };
    let hash = source.hash();
    if shared.lock().by_hash.contains_key(&hash) {
        shared.lock().push_log(None, format!("RSS: {}: already in the torrent list", req.title), now);
        return;
    }
    let o = AddOptions {
        save: Some(req.save_path.trim().to_owned()).filter(|s| !s.is_empty()),
        // a feed named after its URL makes no label
        label: if valid_label(req.label.trim()) { req.label.trim().to_owned() } else { String::new() },
        start: req.paused.then_some(false),
        top: req.top_priority,
        name: matches!(source, Source::Magnet(_)).then(|| req.title.clone()),
        ..AddOptions::default()
    };
    match commands::add(shared, source, &o) {
        Ok(hash) if req.paused => {
            let mut st = shared.lock();
            st.user_paused.insert(hash);
            shared.save(&mut st);
        }
        Ok(_) => {}
        Err(e) => shared.lock().push_log(None, format!("RSS: {}: {}", req.title, e.msg), now),
    }
}

/// Starts the downloads `reqs` ask for; the magnet links are added at once.
fn download(shared: &Shared, mut r: MutexGuard<'_, Rss>, reqs: Vec<AddRequest>) {
    let proxy = proxy(shared);
    let log: Vec<String> = reqs
        .iter()
        .map(|q| {
            let feed = r.d.feed(q.feed).map(|f| f.name().to_owned()).unwrap_or_default();
            format!("RSS: {feed}: downloading {}", q.title)
        })
        .collect();
    let now = r.worker.download(reqs, proxy.as_ref());
    r.dirty = true;
    drop(r);
    after_change(shared, log, true);
    for (req, d) in now {
        add(shared, req, d);
    }
}

/// Runs the filters over every feed's current items (after the filters changed).
fn process_all(shared: &Shared) {
    let mut r = shared.rss();
    let now = unix_now();
    let ids: Vec<FeedId> = r.d.feeds.iter().map(|f| f.id).collect();
    let reqs: Vec<AddRequest> = ids.into_iter().flat_map(|id| r.d.process(id, now)).collect();
    if reqs.is_empty() {
        return;
    }
    download(shared, r, reqs);
}

fn refresh(shared: &Shared, feed: Option<FeedId>) {
    let proxy = proxy(shared);
    let mut r = shared.rss();
    let rss = &mut *r;
    let ids: Vec<FeedId> = match feed {
        Some(id) => vec![id],
        None => rss.d.feeds.iter().filter(|f| f.enabled).map(|f| f.id).collect(),
    };
    for id in ids {
        rss.worker.refresh(&rss.d, id, proxy.as_ref());
    }
    drop(r);
    after_change(shared, Vec::new(), true);
}

// ---- the native endpoints ----------------------------------------------------------------

fn filter_json(f: &RssFilter) -> Json {
    Json::obj()
        .with("id", f.id)
        .with("name", f.name.as_str())
        .with("enabled", f.enabled)
        .with("pattern", f.pattern.as_str())
        .with("not", f.not.as_str())
        .with("save_in", f.save_in.as_str())
        .with("feed", f.feed.unwrap_or(0))
        .with("quality", f.quality.0)
        .with("quality_text", quality_text(f.quality))
        .with("episodes", f.episodes.as_str())
        .with("min_interval", f.min_interval)
        .with("label", f.label.as_str())
        .with("smart", f.smart)
        .with("top_priority", f.top_priority)
        .with("paused", f.paused)
}

/// `GET /pa/rss`: the feeds, the filters, the History and the dialog's fixed lists.
fn model(shared: &Shared) -> Json {
    let labels: Vec<Json> = shared.lock().categories.keys().map(|l| Json::Str(l.clone())).collect();
    let r = shared.rss();
    let feeds: Vec<Json> =
        r.d.feeds
            .iter()
            .map(|f| {
                let updating = r.worker.is_busy(f.id);
                Json::obj()
                    .with("id", f.id)
                    .with("name", f.name())
                    .with("url", f.url.as_str())
                    .with("alias", f.alias.as_str())
                    .with("enabled", f.enabled)
                    .with("subscribe", f.subscribe)
                    .with("smart", f.subscribe_smart)
                    .with("items", f.items.len())
                    .with("updating", updating)
                    .with("status", view::feed_status(f, updating))
            })
            .collect();
    let filters: Vec<Json> = r.d.filters.iter().map(filter_json).collect();
    let history: Vec<Json> = view::history_rows(&r.d)
        .into_iter()
        .map(|h| Json::Arr(h.into_iter().map(Json::Str).collect()))
        .collect();
    let qualities: Vec<Json> =
        Quality::LIST.iter().map(|(q, n)| Json::Arr(vec![q.0.into(), (*n).into()])).collect();
    let intervals: Vec<Json> =
        MIN_INTERVALS.iter().map(|(s, n)| Json::Arr(vec![(*s).into(), lang::text(*n).into()])).collect();
    Json::obj()
        .with("feeds", Json::Arr(feeds))
        .with("filters", Json::Arr(filters))
        .with("history", Json::Arr(history))
        .with("qualities", Json::Arr(qualities))
        .with("intervals", Json::Arr(intervals))
        .with("labels", Json::Arr(labels))
}

/// `GET /pa/rss/items?feed=ID` (0 or none: every feed's): the item list's columns and rows.
fn items(shared: &Shared, feed: Option<FeedId>) -> Json {
    let r = shared.rss();
    let rows: Vec<Json> = view::item_rows(&r.d, feed)
        .into_iter()
        .map(|row| {
            Json::obj()
                .with("feed", row.feed)
                .with("id", row.id)
                .with("downloaded", row.downloaded)
                .with("can", row.url.is_some())
                .with("cells", Json::Arr(row.cells.into_iter().map(Json::Str).collect()))
        })
        .collect();
    let columns: Vec<Json> = view::ITEM_COLUMNS
        .iter()
        .map(|&(t, w, _)| Json::obj().with("title", lang::text(t)).with("width", w as i64))
        .collect();
    Json::obj().with("columns", Json::Arr(columns)).with("rows", Json::Arr(rows))
}

/// `GET /pa/rss/preview?filter=ID`: the items of the feeds' last fetches the filter matches,
/// as `[title, feed]`.
fn preview(shared: &Shared, filter: FilterId) -> Json {
    let r = shared.rss();
    let Some(f) = r.d.filter(filter) else { return Json::Arr(Vec::new()) };
    Json::Arr(
        r.d.matching_items(f)
            .take(500)
            .map(|(feed, i)| {
                let name = r.d.feed(feed).map(|f| f.name().to_owned()).unwrap_or_default();
                Json::Arr(vec![i.title.as_str().into(), name.into()])
            })
            .collect(),
    )
}

fn id_of(body: &Json, key: &str) -> Result<u32, CmdError> {
    body.get(key)
        .and_then(Json::as_i64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| err(400, format!("missing {key}")))
}

fn text<'a>(body: &'a Json, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Json::as_str)
}

fn flag(body: &Json, key: &str) -> Result<Option<bool>, CmdError> {
    match body.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| err(400, format!("{key} must be a boolean"))),
    }
}

/// Sets the filter fields `body` names (the others stay).
fn edit_filter(f: &mut RssFilter, body: &Json) -> Result<(), CmdError> {
    for (key, slot) in [
        ("name", &mut f.name),
        ("pattern", &mut f.pattern),
        ("not", &mut f.not),
        ("episodes", &mut f.episodes),
    ] {
        if let Some(v) = text(body, key) {
            *slot = v.to_owned();
        }
    }
    if let Some(v) = text(body, "save_in") {
        f.save_in = v.trim().to_owned();
    }
    if let Some(v) = text(body, "label") {
        f.label = v.trim().to_owned();
    }
    if let Some(v) = body.get("feed") {
        f.feed = v.as_i64().and_then(|v| u32::try_from(v).ok()).filter(|&v| v != 0);
    }
    if let Some(v) = body.get("quality") {
        f.quality =
            Quality(v.as_i64().and_then(|v| u32::try_from(v).ok()).ok_or_else(|| err(400, "bad quality"))?);
    }
    if let Some(v) = body.get("min_interval") {
        f.min_interval =
            v.as_i64().and_then(|v| u32::try_from(v).ok()).ok_or_else(|| err(400, "bad min_interval"))?;
    }
    for (key, slot) in [
        ("enabled", &mut f.enabled),
        ("smart", &mut f.smart),
        ("top_priority", &mut f.top_priority),
        ("paused", &mut f.paused),
    ] {
        if let Some(v) = flag(body, key)? {
            *slot = v;
        }
    }
    Ok(())
}

fn query_id(req: &Request, key: &str) -> Option<u32> {
    parse_urlencoded(req.head.query.as_bytes())
        .into_iter()
        .rev()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.parse().ok())
}

/// `/pa/rss` and `/pa/rss/...` (`endpoint` is the part after `rss`, with its slash).
pub(crate) fn serve(
    shared: &Shared,
    endpoint: &str,
    req: &Request,
    body: Option<Json>,
) -> Result<Response, CmdError> {
    let Some(body) = body else {
        return match endpoint {
            "" => Ok(Response::json(&model(shared))),
            "/items" => Ok(Response::json(&items(shared, query_id(req, "feed").filter(|&f| f != 0)))),
            "/preview" => {
                let id = query_id(req, "filter").ok_or_else(|| err(400, "missing filter"))?;
                Ok(Response::json(&preview(shared, id)))
            }
            _ => Err(err(405, "POST only")),
        };
    };
    let mut reply = None;
    match endpoint {
        "/feed-add" => {
            let url = text(&body, "url")
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .ok_or_else(|| err(400, "missing url"))?;
            let mut r = shared.rss();
            let id = r.d.add_feed(url, text(&body, "alias").unwrap_or(""));
            if let Some(f) = r.d.feed_mut(id) {
                f.subscribe = flag(&body, "subscribe")?.unwrap_or(false);
                f.subscribe_smart = f.subscribe && flag(&body, "smart")?.unwrap_or(false);
            }
            r.dirty = true;
            drop(r);
            refresh(shared, Some(id));
            reply = Some(Json::obj().with("id", id));
        }
        "/feed-edit" => {
            let id = id_of(&body, "id")?;
            let mut r = shared.rss();
            let Some(old) = r.d.feed(id).map(|f| (f.url.clone(), f.alias.clone())) else {
                return Err(err(404, "no such feed"));
            };
            let url = text(&body, "url").map(str::trim).unwrap_or(&old.0).to_owned();
            if url.is_empty() {
                return Err(err(400, "missing url"));
            }
            r.d.edit_feed(id, &url, text(&body, "alias").unwrap_or(&old.1));
            let f = r.d.feed_mut(id).expect("checked");
            if let Some(v) = flag(&body, "enabled")? {
                f.enabled = v;
            }
            if let Some(v) = flag(&body, "subscribe")? {
                f.subscribe = v;
            }
            if let Some(v) = flag(&body, "smart")? {
                f.subscribe_smart = v;
            }
            f.subscribe_smart &= f.subscribe;
            r.dirty = true;
            drop(r);
            if url != old.0 {
                refresh(shared, Some(id));
            }
        }
        "/feed-remove" => {
            let id = id_of(&body, "id")?;
            let mut r = shared.rss();
            if r.d.feed(id).is_none() {
                return Err(err(404, "no such feed"));
            }
            r.d.remove_feed(id);
            r.dirty = true;
        }
        "/feed-update" => {
            refresh(shared, body.get("id").and_then(Json::as_i64).and_then(|v| u32::try_from(v).ok()))
        }
        "/filter-add" => {
            let mut f = RssFilter::default();
            edit_filter(&mut f, &body)?;
            let mut r = shared.rss();
            let id = r.d.add_filter(f);
            r.dirty = true;
            reply = Some(Json::obj().with("id", id));
        }
        "/filter-edit" => {
            let id = id_of(&body, "id")?;
            let mut r = shared.rss();
            let f = r.d.filter_mut(id).ok_or_else(|| err(404, "no such filter"))?;
            edit_filter(f, &body)?;
            r.dirty = true;
        }
        "/filter-remove" | "/filter-up" | "/filter-down" => {
            let id = id_of(&body, "id")?;
            let mut r = shared.rss();
            if r.d.filter(id).is_none() {
                return Err(err(404, "no such filter"));
            }
            match endpoint {
                "/filter-remove" => r.d.remove_filter(id),
                e => r.d.move_filter(id, e == "/filter-up"),
            }
            r.dirty = true;
        }
        "/download" => {
            let (feed, item) = (id_of(&body, "feed")?, text(&body, "item").unwrap_or(""));
            let mut r = shared.rss();
            let req = r.d.request_item(feed, item, unix_now()).ok_or_else(|| err(404, "no such item"))?;
            download(shared, r, vec![req]);
            return Ok(Response::new(204, "", ""));
        }
        "/history-clear" => {
            let mut r = shared.rss();
            r.d.history.clear();
            r.dirty = true;
        }
        // the dialog closed: the filters may have changed
        "/process" => process_all(shared),
        _ => return Err(err(404, "no such endpoint")),
    }
    after_change(shared, Vec::new(), true);
    Ok(match reply {
        Some(j) => Response::json(&j),
        None => Response::new(204, "", ""),
    })
}

// ---- the qBittorrent shim's rss/* ------------------------------------------------------------
// The qBittorrent RSS tree has folders; ours is flat, so a feed's path is its name. A rule is
// a filter, found by its name; `mustContain` / `mustNotContain` take our wildcard syntax
// (qBittorrent's own, without regular expressions, is close but not the same), `ignoreDays`
// becomes the Minimum interval and `affectedFeeds` the Feed (one feed, else all).

fn feed_by_path(d: &RssDownloader, path: &str) -> Option<FeedId> {
    let path = path.trim().trim_matches('\\');
    d.feeds.iter().find(|f| f.name() == path || f.url == path).map(|f| f.id)
}

fn rule_json(d: &RssDownloader, f: &RssFilter) -> Json {
    let feeds: Vec<Json> = match f.feed {
        Some(id) => d.feed(id).map(|x| Json::Str(x.fetch_url().0.to_owned())).into_iter().collect(),
        None => d.feeds.iter().map(|x| Json::Str(x.fetch_url().0.to_owned())).collect(),
    };
    let episodes: Vec<Json> = f.seen_episodes.iter().map(|e| Json::Str(e.clone())).collect();
    Json::obj()
        .with("enabled", f.enabled)
        .with("mustContain", f.pattern.as_str())
        .with("mustNotContain", f.not.as_str())
        .with("useRegex", false)
        .with("episodeFilter", f.episodes.as_str())
        .with("smartFilter", f.smart)
        .with("previouslyMatchedEpisodes", Json::Arr(episodes))
        .with("affectedFeeds", Json::Arr(feeds))
        .with("ignoreDays", (f.min_interval / 86_400) as i64)
        .with("lastMatch", f.last_match.map(patorrent_frontend::fmt::date_time).unwrap_or_default())
        .with("addPaused", f.paused)
        .with("assignedCategory", f.label.as_str())
        .with("savePath", f.save_in.as_str())
}

fn apply_rule(d: &RssDownloader, f: &mut RssFilter, def: &Json) {
    let s = |k: &str| def.get(k).and_then(Json::as_str);
    let b = |k: &str| def.get(k).and_then(Json::as_bool);
    if let Some(v) = b("enabled") {
        f.enabled = v;
    }
    if let Some(v) = s("mustContain") {
        // qBittorrent: an empty "must contain" takes everything
        f.pattern = if v.trim().is_empty() { "*".into() } else { v.to_owned() };
    }
    if let Some(v) = s("mustNotContain") {
        f.not = v.to_owned();
    }
    if let Some(v) = s("episodeFilter") {
        f.episodes = v.trim().trim_end_matches(';').to_owned();
    }
    if let Some(v) = b("smartFilter") {
        f.smart = v;
    }
    if let Some(Json::Arr(urls)) = def.get("affectedFeeds") {
        let ids: Vec<FeedId> =
            urls.iter().filter_map(Json::as_str).filter_map(|u| feed_by_path(d, u)).collect();
        f.feed = (ids.len() == 1 && d.feeds.len() > 1).then(|| ids[0]);
    }
    if let Some(v) = def.get("ignoreDays").and_then(Json::as_i64) {
        f.min_interval = (v.clamp(0, 3650) * 86_400) as u32;
    }
    if let Some(v) = def.get("addPaused") {
        f.paused = v.as_bool().unwrap_or(false);
    }
    if let Some(v) = s("assignedCategory") {
        f.label = v.trim().to_owned();
    }
    if let Some(v) = s("savePath") {
        f.save_in = v.trim().to_owned();
    }
}

/// One `api/v2/rss/<endpoint>`; `param` reads the request's parameters.
pub(crate) fn shim(shared: &Shared, endpoint: &str, param: &dyn Fn(&str) -> Option<String>) -> Response {
    let conflict = |m: &str| Response::text(409, m.to_owned());
    let mut changed = true;
    let resp = match endpoint {
        "addFolder" => conflict("RSS folders are not supported"),
        "addFeed" => {
            let Some(url) = param("url").map(|u| u.trim().to_owned()).filter(|u| !u.is_empty()) else {
                return Response::text(400, "missing url");
            };
            let path = param("path").unwrap_or_default();
            let mut r = shared.rss();
            if r.d.feeds.iter().any(|f| f.url == url)
                || feed_by_path(&r.d, &path).is_some() && !path.is_empty()
            {
                return conflict("the feed exists already");
            }
            if path.contains('\\') {
                return conflict("RSS folders are not supported");
            }
            let id = r.d.add_feed(&url, &path);
            r.dirty = true;
            drop(r);
            refresh(shared, Some(id));
            Response::ok()
        }
        "removeItem" => {
            let mut r = shared.rss();
            let Some(id) = feed_by_path(&r.d, &param("path").unwrap_or_default()) else {
                return conflict("no such feed");
            };
            r.d.remove_feed(id);
            r.dirty = true;
            Response::ok()
        }
        "moveItem" => {
            let (item, dest) = (param("itemPath").unwrap_or_default(), param("destPath").unwrap_or_default());
            let mut r = shared.rss();
            let Some(id) = feed_by_path(&r.d, &item) else { return conflict("no such feed") };
            if dest.contains('\\') || dest.trim().is_empty() {
                return conflict("RSS folders are not supported");
            }
            let url = r.d.feed(id).map(|f| f.url.clone()).unwrap_or_default();
            r.d.edit_feed(id, &url, &dest);
            r.dirty = true;
            Response::ok()
        }
        "refreshItem" => {
            let path = param("itemPath").unwrap_or_default();
            let id = feed_by_path(&shared.rss().d, &path);
            if id.is_none() && !path.trim().is_empty() {
                return conflict("no such feed");
            }
            refresh(shared, id);
            Response::ok()
        }
        "markAsRead" => {
            let path = param("itemPath").unwrap_or_default();
            let mut r = shared.rss();
            let Some(id) = feed_by_path(&r.d, &path) else { return conflict("no such feed") };
            let article = param("articleId");
            let ids: Vec<String> =
                r.d.feed(id)
                    .map(|f| {
                        f.items
                            .iter()
                            .map(|i| i.id().to_owned())
                            .filter(|i| article.as_deref().is_none_or(|a| a == i))
                            .collect()
                    })
                    .unwrap_or_default();
            r.read.extend(ids.into_iter().map(|i| (id, i)));
            Response::ok()
        }
        "items" => {
            changed = false;
            let with_data = param("withData").is_some_and(|v| v == "true" || v == "1");
            let r = shared.rss();
            let mut out = Json::obj();
            for f in &r.d.feeds {
                let mut j = Json::obj()
                    .with("uid", format!("{{{:08x}-0000-4000-8000-000000000000}}", f.id))
                    .with("url", f.url.as_str());
                if with_data {
                    let articles: Vec<Json> = f
                        .items
                        .iter()
                        .map(|i| {
                            let read =
                                f.downloaded.contains(i.id()) || r.read.contains(&(f.id, i.id().to_owned()));
                            Json::obj()
                                .with("id", i.id())
                                .with(
                                    "date",
                                    i.published.map(patorrent_frontend::fmt::date_time).unwrap_or_default(),
                                )
                                .with("title", i.title.as_str())
                                .with("description", i.description.as_str())
                                .with("link", i.link.as_str())
                                .with("torrentURL", i.torrent_url().unwrap_or_default())
                                .with("isRead", read)
                        })
                        .collect();
                    j = j
                        .with("title", f.title.as_str())
                        .with(
                            "lastBuildDate",
                            f.last_refresh.map(patorrent_frontend::fmt::date_time).unwrap_or_default(),
                        )
                        .with("isLoading", r.worker.is_busy(f.id))
                        .with("hasError", !f.last_error.is_empty())
                        .with("articles", Json::Arr(articles));
                }
                out.set(f.name(), j);
            }
            Response::json(&out)
        }
        "setRule" => {
            let Some(name) = param("ruleName").map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()) else {
                return Response::text(400, "missing ruleName");
            };
            let Some(Ok(def @ Json::Obj(_))) = param("ruleDef").map(|t| json::parse(&t)) else {
                return Response::text(400, "ruleDef must be a JSON object");
            };
            let mut r = shared.rss();
            let rss = &mut *r;
            let pos = rss.d.filters.iter().position(|f| f.name == name);
            let mut f = pos
                .map(|i| rss.d.filters[i].clone())
                .unwrap_or_else(|| RssFilter { name: name.clone(), ..RssFilter::default() });
            apply_rule(&rss.d, &mut f, &def);
            match pos {
                Some(i) => rss.d.filters[i] = f,
                None => {
                    rss.d.add_filter(f);
                }
            }
            rss.dirty = true;
            drop(r);
            // a new or changed rule takes the matching items the feeds hold now
            process_all(shared);
            Response::ok()
        }
        "renameRule" => {
            let (old, new) =
                (param("ruleName").unwrap_or_default(), param("newRuleName").unwrap_or_default());
            let mut r = shared.rss();
            if new.trim().is_empty() {
                return Response::text(400, "missing newRuleName");
            }
            let Some(f) = r.d.filters.iter_mut().find(|f| f.name == old) else {
                return conflict("no such rule");
            };
            f.name = new.trim().to_owned();
            r.dirty = true;
            Response::ok()
        }
        "removeRule" => {
            let name = param("ruleName").unwrap_or_default();
            let mut r = shared.rss();
            let Some(id) = r.d.filters.iter().find(|f| f.name == name).map(|f| f.id) else {
                return conflict("no such rule");
            };
            r.d.remove_filter(id);
            r.dirty = true;
            Response::ok()
        }
        "rules" => {
            changed = false;
            let r = shared.rss();
            let mut out = Json::obj();
            for f in &r.d.filters {
                out.set(&f.name, rule_json(&r.d, f));
            }
            Response::json(&out)
        }
        "matchingArticles" => {
            changed = false;
            let name = param("ruleName").unwrap_or_default();
            let r = shared.rss();
            let Some(f) = r.d.filters.iter().find(|f| f.name == name) else {
                return conflict("no such rule");
            };
            let mut out = Json::obj();
            for (feed, item) in r.d.matching_items(f) {
                let Some(name) = r.d.feed(feed).map(|x| x.name().to_owned()) else { continue };
                let mut list = match out.get(&name) {
                    Some(Json::Arr(v)) => v.clone(),
                    _ => Vec::new(),
                };
                list.push(Json::Str(item.title.clone()));
                out.set(&name, Json::Arr(list));
            }
            Response::json(&out)
        }
        _ => Response::text(404, "Not Found"),
    };
    if changed && resp.status == 200 {
        after_change(shared, Vec::new(), true);
    }
    resp
}

/// The shim's `rss/*` that change something (POST only).
pub(crate) fn shim_post_only(endpoint: &str) -> bool {
    !matches!(endpoint, "items" | "rules" | "matchingArticles")
}
