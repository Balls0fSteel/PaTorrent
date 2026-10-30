//! Torrent search through the user's Prowlarr or Jackett (D41, `docs/INTEGRATIONS.md` part 2):
//! the native `/pa/search...` endpoints the page's search window uses, over the shared model
//! (`patorrent_frontend::search`).
//!
//! The sources are `search.source.N.*` in `State::options` (the API keys sealed in the file:
//! `webui.json` in daemon mode, the host's `patorrent.ini` when hosted, reached through the
//! preferences journal of [`Shared::save`]). One search runs at a time for the whole server,
//! like the Win32 window's: every page sees the same results. The worker's threads only
//! answer into a channel; [`collect`] takes the answers on every alert batch (and on a `GET`)
//! and bumps `State::search_version`, which the WebSocket's `tick` carries as `search`, so an
//! open search window asks `GET /pa/search` again.
//!
//! The qBittorrent shim's `search/*` ([`shim`]) runs its own searches over the same sources,
//! each source a plugin: up to five at once, as qBittorrent, kept until deleted
//! ([`ShimSearches`]) and collected with the native one.
//!
//! Locks: the search's own lock ([`Shared::search`]), the shim's ([`Shared::shim_searches`])
//! and the State lock are never held together, and none is held while anything is fetched.
//!
//! Security (docs/WEBUI.md "Security"): a source's API key never leaves the server (the page
//! gets `has_key` and the URL with the key masked; result links, which carry the indexer's
//! key, are never sent: the shim's `fileUrl` is a magnet or a [`FILE_URL_SCHEME`] URL), and
//! adding a source or changing its URL or key needs the current password, since a source
//! makes the server fetch its URL with the stored key.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use patorrent_core::crypto::Sha1;
use patorrent_core::fetch::FetchProxy;
use patorrent_frontend::rss::Download;
use patorrent_frontend::search::{
    self, Query, Search, SearchResult, Source, SourceStatus, category_text, fetch_caps, fetch_download,
    load_sources, main_categories, redact, store_sources,
};
use patorrent_frontend::{fmt, tr};

use super::commands::{self, AddOptions, CmdError};
use super::http::Response;
use super::json::Json;
use super::{Shared, unix_now};

/// Results `GET /pa/search` lists (the ones with the most seeders).
pub const MAX_RESULTS: usize = 500;
/// Longest query text taken.
const MAX_QUERY: usize = 512;

fn err(status: u16, msg: impl Into<String>) -> CmdError {
    CmdError { status, msg: msg.into() }
}

/// The server's search: the frontend's [`Search`], whose threads need waking nobody (the
/// alert batches collect).
pub(crate) fn new_search() -> Search {
    Search::new(|| {})
}

/// Takes the answers that came in; true when the results or a source's status changed (the
/// caller bumps `State::search_version`). Takes the search lock only.
pub(crate) fn collect(shared: &Shared) -> bool {
    shared.search().collect()
}

/// [`collect`] and the version bump, for a request.
fn collect_and_bump(shared: &Shared) {
    if collect(shared) {
        changed(shared);
    }
}

fn changed(shared: &Shared) {
    shared.lock().search_version += 1;
}

/// A result's id: stable while the search runs (merging reorders the list), from what makes
/// two results the same torrent.
pub fn result_id(r: &SearchResult) -> String {
    let mut h = DefaultHasher::new();
    if r.info_hash.is_empty() {
        (r.name.to_lowercase(), r.size).hash(&mut h);
    } else {
        r.info_hash.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

fn source_json(index: usize, s: &Source) -> Json {
    Json::obj()
        .with("index", index)
        .with("name", s.label())
        .with("url_redacted", redact(&s.url, &s.api_key))
        .with("enabled", s.enabled)
        .with("has_key", !s.api_key.trim().is_empty())
}

/// A source's status line: the state, the count, the error (the key removed) and the text.
fn status_json(label: &str, status: &SourceStatus) -> Json {
    let (state, count, error, text) = match status {
        SourceStatus::Searching => ("searching", 0, String::new(), tr!(SearchSearching, source = label)),
        SourceStatus::Done(n) => ("done", *n, String::new(), tr!(SearchFound, source = label, count = n)),
        SourceStatus::Failed(e) => ("failed", 0, e.clone(), tr!(SearchFailed, source = label, error = e)),
        SourceStatus::Stopped => ("stopped", 0, String::new(), tr!(SearchStopped, source = label)),
    };
    Json::obj()
        .with("source", label)
        .with("status", state)
        .with("count", count)
        .with("error", error)
        .with("text", text)
}

/// `GET /pa/search`: the sources (without their keys), the current search, its sources'
/// status lines and its first [`MAX_RESULTS`] results (never their links: an indexer's
/// download link carries its key).
fn model(shared: &Shared) -> Json {
    let sources = load_sources(&shared.lock().options);
    let now = unix_now();
    let s = shared.search();
    let names: Vec<String> = s.sources().iter().map(|x| x.source.label()).collect();
    let statuses: Vec<Json> =
        s.sources().iter().zip(&names).map(|(x, name)| status_json(name, &x.status)).collect();
    let results: Vec<Json> = s
        .results()
        .iter()
        .take(MAX_RESULTS)
        .map(|r| {
            let opt = |v: Option<u32>| v.map_or(Json::Null, Json::from);
            let from: Vec<Json> =
                r.sources.iter().filter_map(|&i| names.get(i)).map(|n| Json::Str(n.clone())).collect();
            let cats: Vec<Json> = r.categories.iter().map(|&c| Json::from(c)).collect();
            Json::obj()
                .with("id", result_id(r))
                .with("name", r.name.as_str())
                .with("size", r.size.map_or(Json::Null, Json::from))
                .with("size_text", r.size.map(fmt::size).unwrap_or_default())
                .with("seeders", opt(r.seeders))
                .with("peers", opt(r.peers))
                .with("info_hash", r.info_hash.as_str())
                .with("categories", Json::Arr(cats))
                .with("category_text", category_text(&r.categories))
                .with("published", r.published.map_or(Json::Null, Json::from))
                .with(
                    "age_text",
                    r.published
                        .map(|p| fmt::duration(now.saturating_sub(p).max(0) as u64))
                        .unwrap_or_default(),
                )
                .with("source_names", Json::Arr(from))
        })
        .collect();
    let q = s.query();
    let query_cats: Vec<Json> = q.categories.iter().map(|&c| Json::from(c)).collect();
    let main: Vec<Json> = main_categories()
        .iter()
        .map(|(id, name)| Json::Arr(vec![Json::from(*id), Json::from(*name)]))
        .collect();
    Json::obj()
        .with("sources", Json::Arr(sources.iter().enumerate().map(|(i, x)| source_json(i, x)).collect()))
        .with("running", s.is_running())
        .with("id", s.id())
        .with("query", Json::obj().with("q", q.text.as_str()).with("categories", Json::Arr(query_cats)))
        .with("statuses", Json::Arr(statuses))
        .with("total", s.results().len())
        .with("results", Json::Arr(results))
        .with("main_categories", Json::Arr(main))
}

fn text<'a>(body: &'a Json, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Json::as_str)
}

fn index_of(body: &Json, sources: &[Source]) -> Result<usize, CmdError> {
    let i = body
        .get("index")
        .and_then(Json::as_i64)
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| err(400, "missing index"))?;
    if i >= sources.len() {
        return Err(err(404, "no such source"));
    }
    Ok(i)
}

fn flag(body: &Json, key: &str) -> Result<Option<bool>, CmdError> {
    match body.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| err(400, format!("{key} must be a boolean"))),
    }
}

/// An `http://` or `https://` URL, trimmed.
fn valid_url(url: &str) -> Result<String, CmdError> {
    let url = url.trim();
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    if !matches!(scheme.as_deref(), Some("http" | "https")) || search::source::url_host(url).is_none() {
        return Err(err(400, tr!(SearchNoUrl)));
    }
    Ok(url.to_owned())
}

/// The current password, needed to add a source or change its URL or key: the source makes
/// the server fetch its URL (any address: it is the user's) with the stored key, so a stolen
/// session must not be able to point a source, and its key, somewhere else. A wrong one ends
/// the session that sent it, as Preferences' security fields do (SEC-03).
fn check_password(shared: &Shared, body: &Json, session: &str) -> Result<(), CmdError> {
    let mut st = shared.lock();
    let given = text(body, "password");
    if given.is_some_and(|p| st.password.verify(p)) {
        return Ok(());
    }
    if given.is_some_and(|p| !p.is_empty()) {
        st.sessions.remove(session);
    }
    Err(err(400, tr!(WebSearchNeedPassword)))
}

/// Stores `sources` in the options (sealed keys; hosted, the host gets the journal).
fn store(shared: &Shared, sources: &[Source]) {
    let mut st = shared.lock();
    store_sources(&mut st.options, sources);
    st.search_version += 1;
    shared.save(&mut st);
}

fn proxy(shared: &Shared) -> Option<FetchProxy> {
    FetchProxy::from_settings(&shared.lock().settings)
}

/// `POST /pa/search/start`.
fn start(shared: &Shared, body: &Json) -> Result<(), CmdError> {
    let q = text(body, "q").map(str::trim).unwrap_or("");
    if q.is_empty() {
        return Err(err(400, "missing q"));
    }
    let q: String = q.chars().take(MAX_QUERY).collect();
    let mut categories = Vec::new();
    if let Some(Json::Arr(v)) = body.get("categories") {
        for c in v {
            let c = c.as_i64().and_then(|c| u32::try_from(c).ok()).ok_or_else(|| err(400, "bad category"))?;
            if !categories.contains(&c) {
                categories.push(c);
            }
        }
    }
    let all = load_sources(&shared.lock().options);
    // named sources are searched even when they are left out of "All sources"
    let picked: Vec<Source> = match body.get("sources") {
        Some(Json::Arr(v)) if !v.is_empty() => {
            let mut out = Vec::new();
            for i in v {
                let i = i.as_i64().and_then(|i| usize::try_from(i).ok()).filter(|&i| i < all.len());
                let i = i.ok_or_else(|| err(404, "no such source"))?;
                out.push(Source { enabled: true, ..all[i].clone() });
            }
            out
        }
        _ => all,
    };
    if !picked.iter().any(|s| s.enabled) {
        return Err(err(409, tr!(SearchNoSources)));
    }
    let proxy = proxy(shared);
    shared.search().start(&picked, Query { text: q, categories }, proxy.as_ref());
    changed(shared);
    Ok(())
}

/// `POST /pa/search/add`: a result of the current search, fetched on this request's thread
/// (no lock held), then added as `/pa/add` adds (hosted, into the host's engine).
fn add(shared: &Shared, body: &Json) -> Result<Response, CmdError> {
    let id = text(body, "id").ok_or_else(|| err(400, "missing id"))?;
    let o = AddOptions::parse(body).map_err(|e| err(400, e))?;
    let (result, source) = {
        let s = shared.search();
        let r = s.results().iter().find(|r| result_id(r) == id).ok_or_else(|| err(404, "no such result"))?;
        let src = s.source_of(r).cloned().ok_or_else(|| err(404, "no such result"))?;
        (r.clone(), src)
    };
    if !result.info_hash.is_empty() && shared.lock().by_hash.contains_key(&result.info_hash) {
        return Err(err(409, "the torrent is already in the list"));
    }
    let proxy = proxy(shared);
    let d = fetch_download(&result, &source, proxy.as_ref()).map_err(|e| {
        let e = source.redact(&e);
        eprintln!("! search: {}: {e}", result.name);
        err(400, tr!(SearchFailed, source = source.label(), error = e))
    })?;
    let add_source = match d {
        Download::Magnet(m) => commands::Source::from_url(shared, &m)?,
        Download::Torrent(b) => commands::Source::from_torrent_bytes(b)?,
    };
    if shared.lock().by_hash.contains_key(&add_source.hash()) {
        return Err(err(409, "the torrent is already in the list"));
    }
    let hash = commands::add(shared, add_source, &o)?;
    Ok(Response::json(&Json::obj().with("hash", hash).with("text", tr!(SearchAdded, name = result.name))))
}

/// `POST /pa/search/source-test`: the source's caps (`t=caps`), asked on this request's
/// thread (30 s at most); the answer says how it went.
fn test(shared: &Shared, body: &Json) -> Result<Json, CmdError> {
    let sources = load_sources(&shared.lock().options);
    let s = &sources[index_of(body, &sources)?];
    let proxy = proxy(shared);
    let label = s.label();
    Ok(match fetch_caps(s, proxy.as_ref()) {
        Ok(cats) => {
            let n = cats.len() + cats.iter().map(|c| c.subcats.len()).sum::<usize>();
            Json::obj()
                .with("ok", true)
                .with("categories", n)
                .with("text", tr!(SearchTestOk, source = label, count = n))
        }
        Err(e) => {
            let e = s.redact(&e.to_string());
            Json::obj()
                .with("ok", false)
                .with("error", e.as_str())
                .with("text", tr!(SearchFailed, source = label, error = e))
        }
    })
}

/// `/pa/search` and `/pa/search/...` (`endpoint` is the part after `search`, with its slash);
/// `session` is the requester's.
pub(crate) fn serve(
    shared: &Shared,
    endpoint: &str,
    body: Option<Json>,
    session: &str,
) -> Result<Response, CmdError> {
    let Some(body) = body else {
        return match endpoint {
            "" => {
                collect_and_bump(shared);
                Ok(Response::json(&model(shared)))
            }
            _ => Err(err(405, "POST only")),
        };
    };
    let none = || Response::new(204, "", "");
    match endpoint {
        "/start" => {
            start(shared, &body)?;
            Ok(none())
        }
        "/stop" => {
            shared.search().stop();
            changed(shared);
            Ok(none())
        }
        "/add" => add(shared, &body),
        "/source-add" => {
            let url = valid_url(text(&body, "url").unwrap_or(""))?;
            check_password(shared, &body, session)?;
            let mut sources = load_sources(&shared.lock().options);
            if sources.len() >= search::source::MAX_SOURCES {
                return Err(err(409, "too many sources"));
            }
            sources.push(Source {
                name: text(&body, "name").unwrap_or("").trim().to_owned(),
                url,
                api_key: text(&body, "key").unwrap_or("").trim().to_owned(),
                enabled: flag(&body, "enabled")?.unwrap_or(true),
            });
            store(shared, &sources);
            Ok(Response::json(&Json::obj().with("index", sources.len() - 1)))
        }
        "/source-edit" => {
            let mut sources = load_sources(&shared.lock().options);
            let i = index_of(&body, &sources)?;
            let url = match text(&body, "url") {
                Some(u) => valid_url(u)?,
                None => sources[i].url.clone(),
            };
            let key = text(&body, "key").map(str::trim).filter(|k| !k.is_empty());
            if url != sources[i].url || key.is_some_and(|k| k != sources[i].api_key) {
                check_password(shared, &body, session)?;
            }
            let s = &mut sources[i];
            s.url = url;
            if let Some(k) = key {
                s.api_key = k.to_owned();
            } else if flag(&body, "clear_key")? == Some(true) {
                s.api_key.clear();
            }
            if let Some(n) = text(&body, "name") {
                s.name = n.trim().to_owned();
            }
            if let Some(e) = flag(&body, "enabled")? {
                s.enabled = e;
            }
            store(shared, &sources);
            Ok(none())
        }
        "/source-remove" => {
            let mut sources = load_sources(&shared.lock().options);
            let i = index_of(&body, &sources)?;
            sources.remove(i);
            store(shared, &sources);
            Ok(none())
        }
        "/source-test" => Ok(Response::json(&test(shared, &body)?)),
        _ => Err(err(404, "no such endpoint")),
    }
}

// ---- the qBittorrent shim's `search/*` --------------------------------------------------------

/// Searches the shim runs at once (qBittorrent's limit).
const MAX_RUNNING_JOBS: usize = 5;
/// Searches the shim keeps; a new one past it drops the oldest finished one.
const MAX_JOBS: usize = 32;
/// A shim result's `fileUrl` when it has no magnet of its own: `patorrent-search:<job>/<result
/// id>`. The indexer's link carries its API key, so it never leaves the server; `torrents/add`
/// and `search/downloadTorrent` take this URL back and fetch the result as `/pa/search/add`
/// does ([`fetch_download`], the proxy rule included).
pub const FILE_URL_SCHEME: &str = "patorrent-search:";

/// qBittorrent's search categories the shim offers (id, qBittorrent's name) and the Newznab
/// categories each one searches; any other id searches every category.
const CATEGORIES: [(&str, &str, &[u32]); 7] = [
    ("anime", "Anime", &[5070]),
    ("books", "Books", &[7000]),
    ("games", "Games", &[1000, 4050]),
    ("movies", "Movies", &[2000]),
    ("music", "Music", &[3000]),
    ("software", "Software", &[4000]),
    ("tv", "TV shows", &[5000]),
];

/// One `search/start`.
pub(crate) struct ShimJob {
    id: u32,
    pattern: String,
    category: String,
    /// The plugin names of `search`'s sources, in its order ([`SearchResult::source`]).
    plugins: Vec<String>,
    search: Search,
    /// The results' ids in the order they first came: merging re-sorts the search's list, and
    /// clients page through `search/results` by offset.
    order: Vec<String>,
    seen: HashSet<String>,
}

/// The shim's searches ([`Shared::shim_searches`]); their lock is never held with the State
/// lock or the native search's.
#[derive(Default)]
pub(crate) struct ShimSearches {
    last_id: u32,
    jobs: Vec<ShimJob>,
}

impl ShimSearches {
    /// Takes every job's answers, appending the new results' ids.
    fn collect(&mut self) {
        for j in &mut self.jobs {
            if j.search.collect() {
                for r in j.search.results() {
                    let id = result_id(r);
                    if j.seen.insert(id.clone()) {
                        j.order.push(id);
                    }
                }
            }
        }
    }

    fn job(&self, id: i64) -> Option<&ShimJob> {
        self.jobs.iter().find(|j| i64::from(j.id) == id)
    }
}

/// Takes the shim's searches' answers (the alert batches call it beside [`collect`]).
pub(crate) fn collect_shim(shared: &Shared) {
    shared.shim_searches().collect();
}

/// The sources' plugin names: `torznab-` and 8 hex digits of a SHA-1 of the URL with its key
/// masked, so a name stays while sources are removed or renumbered (a twin URL gets `-2`, ...).
fn plugin_names(sources: &[Source]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(sources.len());
    for s in sources {
        let hex = Sha1::digest(redact(s.url.trim(), &s.api_key).as_bytes()).to_hex();
        let base = format!("torznab-{}", &hex[..8]);
        let (mut name, mut n) = (base.clone(), 1);
        while out.contains(&name) {
            n += 1;
            name = format!("{base}-{n}");
        }
        out.push(name);
    }
    out
}

/// A source's site: its scheme, host and port, nothing that could hold the key.
fn site_url(url: &str) -> String {
    let Some((scheme, host, port)) = search::source::url_origin(url) else { return String::new() };
    let host = if host.contains(':') { format!("[{host}]") } else { host };
    match (scheme.as_str(), port) {
        ("http", 80) | ("https", 443) => format!("{scheme}://{host}"),
        _ => format!("{scheme}://{host}:{port}"),
    }
}

/// A result's `fileUrl`: its magnet, or one made from its info-hash when it has no link (what
/// [`fetch_download`] would add), else the [`FILE_URL_SCHEME`] URL; never anything holding the
/// source's key.
fn file_url(job: u32, r: &SearchResult, key: &str) -> String {
    let clean = |m: String| (redact(&m, key) == m).then_some(m);
    let direct = if !r.magnet.is_empty() {
        clean(r.magnet.clone())
    } else if r.link.trim().is_empty() {
        r.magnet_link().and_then(clean)
    } else {
        None
    };
    direct.unwrap_or_else(|| format!("{FILE_URL_SCHEME}{job}/{}", result_id(r)))
}

/// What a [`FILE_URL_SCHEME`] URL adds, fetched on the request's thread (no lock held); `None`
/// for any other URL.
pub(crate) fn shim_download(shared: &Shared, url: &str) -> Option<Result<commands::Source, CmdError>> {
    let rest = url.trim().strip_prefix(FILE_URL_SCHEME)?;
    Some(resolve(shared, rest))
}

fn resolve(shared: &Shared, rest: &str) -> Result<commands::Source, CmdError> {
    let gone = || err(404, "no such search result");
    let (job, id) = rest.split_once('/').ok_or_else(gone)?;
    let job: i64 = job.parse().map_err(|_| gone())?;
    let (result, source) = {
        let mut jobs = shared.shim_searches();
        jobs.collect();
        let j = jobs.job(job).ok_or_else(gone)?;
        let r = j.search.results().iter().find(|r| result_id(r) == id).ok_or_else(gone)?;
        (r.clone(), j.search.source_of(r).cloned().ok_or_else(gone)?)
    };
    let proxy = proxy(shared);
    let d = fetch_download(&result, &source, proxy.as_ref()).map_err(|e| {
        let e = source.redact(&e);
        eprintln!("! search: {}: {e}", result.name);
        err(400, tr!(SearchFailed, source = source.label(), error = e))
    })?;
    match d {
        Download::Magnet(m) => commands::Source::from_url(shared, &m),
        Download::Torrent(b) => commands::Source::from_torrent_bytes(b),
    }
}

fn plugin_json(s: &Source, name: &str) -> Json {
    let mut cats = vec![Json::obj().with("id", "all").with("name", "All categories")];
    cats.extend(CATEGORIES.iter().map(|(id, n, _)| Json::obj().with("id", *id).with("name", *n)));
    Json::obj()
        .with("name", name)
        .with("version", "1.0")
        .with("fullName", s.label())
        .with("url", site_url(&s.url))
        .with("supportedCategories", Json::Arr(cats))
        .with("enabled", s.enabled)
}

fn running(j: &ShimJob) -> &'static str {
    if j.search.is_running() { "Running" } else { "Stopped" }
}

fn job_json(j: &ShimJob) -> Json {
    Json::obj()
        .with("id", j.id)
        .with("status", running(j))
        .with("pattern", j.pattern.as_str())
        .with("category", j.category.as_str())
        .with("plugins", Json::Arr(j.plugins.iter().map(|p| Json::from(p.as_str())).collect()))
        .with("total", j.order.len())
}

/// A `search/results` entry with qBittorrent's fields (-1 for a number the indexer did not
/// give).
fn result_json(j: &ShimJob, r: &SearchResult) -> Json {
    let source = j.search.sources().get(r.source).map(|s| &s.source);
    let num = |v: Option<u32>| v.map_or(-1, i64::from);
    let leechers = match (r.seeders, r.peers) {
        (Some(s), Some(p)) => i64::from(p.saturating_sub(s)),
        _ => -1,
    };
    Json::obj()
        .with("fileName", r.name.as_str())
        .with("fileUrl", file_url(j.id, r, source.map_or("", |s| s.api_key.as_str())))
        .with("fileSize", r.size.map_or(-1, |s| i64::try_from(s).unwrap_or(i64::MAX)))
        .with("nbSeeders", num(r.seeders))
        .with("nbLeechers", leechers)
        .with("engineName", j.plugins.get(r.source).map_or("", String::as_str))
        .with("engineFullName", source.map(Source::label).unwrap_or_default())
        .with("siteUrl", source.map(|s| site_url(&s.url)).unwrap_or_default())
        .with("descrLink", "")
        .with("pubDate", r.published.unwrap_or(-1))
}

/// `search/start`.
fn shim_start(shared: &Shared, param: &dyn Fn(&str) -> Option<String>) -> Response {
    let (Some(pattern), Some(category), Some(plugins)) =
        (param("pattern"), param("category"), param("plugins"))
    else {
        return Response::text(400, "missing pattern, category or plugins");
    };
    let pattern: String = pattern.trim().chars().take(MAX_QUERY).collect();
    if pattern.is_empty() {
        return Response::text(400, "missing pattern");
    }
    let category = category.trim().to_owned();
    let categories =
        CATEGORIES.iter().find(|c| c.0.eq_ignore_ascii_case(&category)).map_or(Vec::new(), |c| c.2.to_vec());
    let all = load_sources(&shared.lock().options);
    let names = plugin_names(&all);
    let wanted: Vec<&str> = plugins.split('|').map(str::trim).filter(|p| !p.is_empty()).collect();
    let forced = |s: &Source| Source { enabled: true, ..s.clone() };
    // "all" and plugins named one by one are searched even when switched off, as a source
    // named in `/pa/search/start` is
    let picked: Vec<(Source, String)> = match wanted.as_slice() {
        [one] if one.eq_ignore_ascii_case("all") => all.iter().map(forced).zip(names).collect(),
        [one] if one.eq_ignore_ascii_case("enabled") || one.eq_ignore_ascii_case("multi") => {
            all.into_iter().zip(names).filter(|(s, _)| s.enabled).collect()
        }
        list => all
            .iter()
            .zip(names)
            .filter(|(_, n)| list.contains(&n.as_str()))
            .map(|(s, n)| (forced(s), n))
            .collect(),
    };
    if picked.is_empty() {
        return Response::text(409, tr!(SearchNoSources));
    }
    let (sources, plugins): (Vec<Source>, Vec<String>) = picked.into_iter().unzip();
    let proxy = proxy(shared);
    let mut jobs = shared.shim_searches();
    jobs.collect();
    if jobs.jobs.iter().filter(|j| j.search.is_running()).count() >= MAX_RUNNING_JOBS {
        return Response::text(
            409,
            format!("Unable to create more than {MAX_RUNNING_JOBS} concurrent searches."),
        );
    }
    while jobs.jobs.len() >= MAX_JOBS {
        let Some(i) = jobs.jobs.iter().position(|j| !j.search.is_running()) else { break };
        jobs.jobs.remove(i);
    }
    jobs.last_id += 1;
    let id = jobs.last_id;
    let mut search = new_search();
    search.start(&sources, Query { text: pattern.clone(), categories }, proxy.as_ref());
    jobs.jobs.push(ShimJob {
        id,
        pattern,
        category,
        plugins,
        search,
        order: Vec::new(),
        seen: HashSet::new(),
    });
    Response::json(&Json::obj().with("id", id))
}

/// `search/results`, qBittorrent's paging: `offset` may count from the end, a `limit` of 0 or
/// less is everything.
fn shim_results(j: &ShimJob, param: &dyn Fn(&str) -> Option<String>) -> Response {
    let int = |k: &str| param(k).and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(0);
    let total = j.order.len() as i64;
    let mut offset = int("offset");
    if offset < 0 {
        offset += total;
    }
    if !(0..=total).contains(&offset) {
        return Response::text(409, "Offset is out of range");
    }
    let limit = int("limit");
    let limit = if limit <= 0 { usize::MAX } else { limit as usize };
    let by_id: HashMap<String, &SearchResult> =
        j.search.results().iter().map(|r| (result_id(r), r)).collect();
    let results: Vec<Json> = j
        .order
        .iter()
        .skip(offset as usize)
        .take(limit)
        .filter_map(|id| by_id.get(id))
        .map(|r| result_json(j, r))
        .collect();
    Response::json(
        &Json::obj().with("status", running(j)).with("results", Json::Arr(results)).with("total", total),
    )
}

/// One `api/v2/search/<endpoint>` (docs/WEBUI.md "The torrent search"): each search source is
/// a plugin; `param` reads the request's parameters.
pub(crate) fn shim(shared: &Shared, endpoint: &str, param: &dyn Fn(&str) -> Option<String>) -> Response {
    let id = || param("id").and_then(|v| v.trim().parse::<i64>().ok());
    let missing = |what: &str| Response::text(400, format!("missing {what}"));
    let not_found = || Response::text(404, "Not Found");
    match endpoint {
        "start" => shim_start(shared, param),
        "stop" | "delete" => {
            let Some(id) = id() else { return missing("id") };
            let mut jobs = shared.shim_searches();
            let Some(i) = jobs.jobs.iter().position(|j| i64::from(j.id) == id) else { return not_found() };
            if endpoint == "stop" {
                jobs.jobs[i].search.stop();
            } else {
                jobs.jobs.remove(i);
            }
            Response::ok()
        }
        "status" => {
            let mut jobs = shared.shim_searches();
            jobs.collect();
            match id().filter(|&i| i != 0) {
                Some(i) => {
                    jobs.job(i).map_or_else(not_found, |j| Response::json(&Json::Arr(vec![job_json(j)])))
                }
                None => Response::json(&Json::Arr(jobs.jobs.iter().map(job_json).collect())),
            }
        }
        "results" => {
            let Some(id) = id() else { return missing("id") };
            let mut jobs = shared.shim_searches();
            jobs.collect();
            jobs.job(id).map_or_else(not_found, |j| shim_results(j, param))
        }
        "downloadTorrent" => {
            let Some(url) = param("torrentUrl") else { return missing("torrentUrl") };
            let source =
                shim_download(shared, &url).unwrap_or_else(|| commands::Source::from_url(shared, &url));
            match source.and_then(|s| commands::add(shared, s, &AddOptions::default())) {
                Ok(_) => Response::ok(),
                Err(e) => Response::text(e.status, e.msg),
            }
        }
        "plugins" => {
            let sources = load_sources(&shared.lock().options);
            let names = plugin_names(&sources);
            Response::json(&Json::Arr(sources.iter().zip(&names).map(|(s, n)| plugin_json(s, n)).collect()))
        }
        "enablePlugin" | "uninstallPlugin" => {
            let Some(names) = param("names") else { return missing("names") };
            let names: Vec<&str> = names.split('|').map(str::trim).collect();
            let enable = match (endpoint, param("enable")) {
                ("uninstallPlugin", _) => None,
                (_, Some(v)) => Some(v.trim().eq_ignore_ascii_case("true") || v.trim() == "1"),
                (_, None) => return missing("enable"),
            };
            // as the native switch and source-remove: no password (neither points a source,
            // or its key, anywhere new)
            let all = load_sources(&shared.lock().options);
            let plugins = plugin_names(&all);
            let mut sources = Vec::with_capacity(all.len());
            for (mut s, name) in all.iter().cloned().zip(&plugins) {
                let named = names.contains(&name.as_str());
                match enable {
                    Some(e) if named => s.enabled = e,
                    None if named => continue,
                    _ => {}
                }
                sources.push(s);
            }
            if sources != all {
                store(shared, &sources);
            }
            Response::ok()
        }
        "installPlugin" => Response::text(
            409,
            "Search plugins are not supported: PaTorrent searches Torznab sources (Prowlarr, Jackett), \
             added in its Web UI's search window",
        ),
        // nothing to update: a source is the user's own server
        "updatePlugins" => Response::ok(),
        _ => not_found(),
    }
}

/// The shim's `search/*` that change something (POST only).
pub(crate) fn shim_post_only(endpoint: &str) -> bool {
    !matches!(endpoint, "status" | "results" | "plugins")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(url: &str, key: &str) -> Source {
        Source { url: url.into(), api_key: key.into(), ..Source::default() }
    }

    /// A plugin's name does not move with the key or the source's place; twins get a suffix.
    #[test]
    fn plugin_names_are_stable() {
        let a = source("http://prowlarr:9696/1/api", "k1");
        let b = source("http://prowlarr:9696/2/api", "k1");
        let names = plugin_names(&[a.clone(), b.clone()]);
        assert_ne!(names[0], names[1]);
        assert_eq!(plugin_names(&[b, source("http://prowlarr:9696/1/api", "other")])[1], names[0]);
        let in_url = source("http://j/api?apikey=k1", "k1");
        let twins = plugin_names(&[in_url.clone(), source("http://j/api?apikey=k2", "k2"), in_url]);
        assert_eq!(twins[1], format!("{}-2", twins[0]));
        assert_eq!(twins[2], format!("{}-3", twins[0]));
    }

    #[test]
    fn site_urls_hold_only_the_origin() {
        assert_eq!(site_url("https://user:pw@idx.example/api?apikey=k"), "https://idx.example");
        assert_eq!(site_url("http://[::1]:9696/1/api"), "http://[::1]:9696");
        assert_eq!(site_url("not a url"), "");
    }

    /// The magnet, else one from the info-hash when there is no link, else the opaque URL; a
    /// magnet holding the key is never sent.
    #[test]
    fn file_urls_never_hold_the_key() {
        let hash = "58cebaa968651dc2a241c495719f2b6cbec33e32";
        let r = SearchResult { name: "x".into(), info_hash: hash.into(), ..SearchResult::default() };
        assert!(file_url(1, &r, "k").starts_with(&format!("magnet:?xt=urn:btih:{hash}")));
        let linked = SearchResult { link: "http://idx/dl?apikey=k".into(), ..r.clone() };
        assert_eq!(file_url(3, &linked, "k"), format!("{FILE_URL_SCHEME}3/{}", result_id(&linked)));
        let m =
            SearchResult { magnet: "magnet:?xt=urn:btih:aa&tr=http://t/announce".into(), ..linked.clone() };
        assert_eq!(file_url(3, &m, "k"), m.magnet);
        let keyed = SearchResult { magnet: "magnet:?xt=urn:btih:aa&tr=http://idx/K3Y".into(), ..linked };
        assert!(file_url(3, &keyed, "K3Y").starts_with(FILE_URL_SCHEME));
    }
}
