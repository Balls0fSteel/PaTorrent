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
//! Locks: the search's own lock ([`Shared::search`]) and the State lock are never held
//! together, and neither is held while anything is fetched.
//!
//! Security (docs/WEBUI.md "Security"): a source's API key never leaves the server (the page
//! gets `has_key` and the URL with the key masked; result links, which carry the indexer's
//! key, are never sent), and adding a source or changing its URL or key needs the current
//! password, since a source makes the server fetch its URL with the stored key.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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
