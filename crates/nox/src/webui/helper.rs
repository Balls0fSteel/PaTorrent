//! The search helper (D51, `docs/SEARCH_HELPER.md`) in the Web UI: the `/pa/search/helper...`
//! endpoints behind Manage Sources' "Get the Search Helper (Jackett)...", "Pick Sites...",
//! "Open the Helper's Page", "Update" and "Remove", over the GUIs' own code
//! (`rookparcel_frontend::search::helper`). The helper is the process's: nox's in its state
//! directory ([`Config::helper_dir`](super::Config::helper_dir)), and hosted (D40) the window's
//! own, in its profile folder, so a browser and the window act on one helper.
//!
//! Every step that waits (the release asked of GitHub, the download, a start of the helper,
//! the sites configured) runs as a task on a thread of its own: the request answers its id at
//! once, and the page asks `GET /pa/search/helper` until the task is done. One task runs at a
//! time for the whole server. A cancel ends a download at once; a start under way is left to
//! finish (its answer is dropped), and no new task starts until it has.
//!
//! The download and install need the current Web UI password, as adding a source does
//! (`search.rs`): they make the server download and run a program. The install takes only the
//! release the last check found (the page names its version, never an address).
//!
//! "Open the Helper's Page": the helper listens on the server's `127.0.0.1` alone, so its page
//! opens only in a browser on that computer. The task and the password it hands back are for a
//! request from this machine (loopback, not through a reverse proxy); elsewhere the page says
//! where it works instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rookparcel_core::fetch::FetchProxy;
use rookparcel_frontend::search::helper::{self, HelperProxy, Indexer, Release};
use rookparcel_frontend::search::load_sources;
use rookparcel_frontend::{fmt, tr};

use super::Shared;
use super::commands::CmdError;
use super::http::Response;
use super::json::Json;

fn err(status: u16, msg: impl Into<String>) -> CmdError {
    CmdError { status, msg: msg.into() }
}

/// What a task does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The newest release, and the question to ask before its download.
    Check,
    /// The download, the unpack and the source.
    Install,
    /// The public sites (the helper started first).
    Sites,
    /// The ticked sites configured, the unticked removed.
    Apply,
    /// The helper started for its own page.
    Page,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Check => "check",
            Kind::Install => "install",
            Kind::Sites => "sites",
            Kind::Apply => "apply",
            Kind::Page => "page",
        }
    }
}

/// One step of the helper's, as the page sees it.
struct Task {
    id: u64,
    kind: Kind,
    /// The page waits on it: false once it ended or was cancelled.
    running: bool,
    /// Its thread has not ended (a cancelled start still may run): no new task until it has.
    worker: bool,
    cancel: Arc<AtomicBool>,
    cancelled: bool,
    got: u64,
    total: Option<u64>,
    /// The translated error when it failed.
    error: String,
    /// The translated line when it is done (installed, up to date, sites ticked).
    text: String,
    /// Check: the box to show before the download, and the release's version.
    question: String,
    up_to_date: bool,
    version: String,
    /// Check and install: no helper was installed (the page opens the site picker after it).
    first: bool,
    /// Page: its address and the password it asks for.
    page: Option<(String, String)>,
}

/// The helper's tasks ([`Shared::helper`]).
#[derive(Default)]
pub(crate) struct Tasks {
    last_id: u64,
    task: Option<Task>,
    /// The last check's release: what an install downloads.
    release: Option<Release>,
    /// The last listing: what the ticks are applied to.
    sites: Vec<Indexer>,
}

/// What a finished task's thread hands back.
enum Done {
    Checked { release: Release, question: String, up_to_date: Option<String> },
    Installed(helper::State),
    Listed(Vec<Indexer>),
    Applied(usize),
    Page(String, String),
}

/// Tells the frontend where the helper lives and which proxy it gets (the session's, for its
/// own fetches); whether this server offers the helper at all ([`helper::available`]: not in
/// the Flatpak, the Store or package-manager builds, FreeBSD). Cheap; called before each step
/// and each search, so a changed proxy reaches the next start.
pub(crate) fn configure(shared: &Shared) -> bool {
    let Some(dir) = &shared.helper_dir else { return false };
    if !helper::available() {
        return false;
    }
    let proxy = HelperProxy::from_settings(&shared.lock().settings);
    helper::configure(dir, proxy);
    true
}

fn proxy(shared: &Shared) -> Option<FetchProxy> {
    FetchProxy::from_settings(&shared.lock().settings)
}

fn task_json(t: &Task, sites: &[Indexer], local: bool) -> Json {
    let progress = match t.total {
        Some(total) => tr!(ExitMoveProgress, done = fmt::size(t.got), total = fmt::size(total)),
        None if t.got > 0 => fmt::size(t.got),
        None => String::new(),
    };
    let mut j = Json::obj()
        .with("id", t.id)
        .with("kind", t.kind.name())
        .with("running", t.running)
        .with("cancelled", t.cancelled)
        .with("got", t.got)
        .with("total", t.total.map_or(Json::Null, Json::from))
        .with("progress_text", progress)
        .with("error", t.error.as_str())
        .with("text", t.text.as_str())
        .with("version", t.version.as_str())
        .with("first", t.first);
    if t.kind == Kind::Check {
        j = j.with("question", t.question.as_str()).with("up_to_date", t.up_to_date);
    }
    if t.kind == Kind::Sites && !t.running && t.error.is_empty() {
        let list: Vec<Json> = sites
            .iter()
            .map(|i| {
                Json::obj()
                    .with("id", i.id.as_str())
                    .with("name", i.name.as_str())
                    .with("language", i.language.as_str())
                    .with("type", i.kind.as_str())
                    .with("configured", i.configured)
                    .with("needs_flaresolverr", i.needs_flaresolverr)
            })
            .collect();
        j = j.with("sites", Json::Arr(list));
    }
    // the password only to a browser on this computer, the one place the page opens
    if let (Some((url, password)), true) = (&t.page, local) {
        j = j.with("url", url.as_str()).with("password", password.as_str());
    }
    j
}

/// `GET /pa/search/helper`.
fn model(shared: &Shared, local: bool) -> Json {
    let available = configure(shared);
    let mut j = Json::obj().with("available", available).with("local", local);
    if !available {
        return j;
    }
    let state = helper::state();
    let folder = helper::layout().map(|l| l.root.display().to_string()).unwrap_or_default();
    j = j
        .with("installed", helper::installed())
        .with("version", state.map(|s| s.version).unwrap_or_default())
        .with("folder", folder)
        .with("running", helper::running());
    let t = shared.helper();
    j.with("task", t.task.as_ref().map_or(Json::Null, |x| task_json(x, &t.sites, local)))
}

/// Hands a task's progress to the page, and its cancel to the download.
struct Progress {
    shared: Arc<Shared>,
    id: u64,
    cancel: Arc<AtomicBool>,
}

impl Progress {
    fn update(&self, got: u64, total: Option<u64>) -> bool {
        if let Some(t) = self.shared.helper().task.as_mut().filter(|t| t.id == self.id) {
            t.got = got;
            t.total = total;
        }
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// Starts `work` as a task of `kind` on its own thread; its id. 409 while another one runs.
fn start(
    shared: &Shared,
    kind: Kind,
    first: bool,
    work: impl FnOnce(&Progress) -> Result<Done, String> + Send + 'static,
) -> Result<u64, CmdError> {
    let me = shared.this.upgrade().ok_or_else(|| err(503, "the server is stopping"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let id = {
        let mut t = shared.helper();
        if t.task.as_ref().is_some_and(|x| x.worker) {
            return Err(err(409, tr!(WebHelperBusy)));
        }
        t.last_id += 1;
        let id = t.last_id;
        t.task = Some(Task {
            id,
            kind,
            running: true,
            worker: true,
            cancel: cancel.clone(),
            cancelled: false,
            got: 0,
            total: None,
            error: String::new(),
            text: String::new(),
            question: String::new(),
            up_to_date: false,
            version: String::new(),
            first,
            page: None,
        });
        id
    };
    let p = Progress { shared: me, id, cancel };
    let spawned = std::thread::Builder::new().name("search-helper".into()).spawn(move || {
        let r = work(&p);
        finish(&p.shared, p.id, r);
    });
    if let Err(e) = spawned {
        finish(shared, id, Err(e.to_string()));
    }
    Ok(id)
}

/// A task's thread ended with `r`.
fn finish(shared: &Shared, id: u64, r: Result<Done, String>) {
    // installed is installed, even when the cancel came too late to stop it: its source goes in
    if let Ok(Done::Installed(state)) = &r {
        let sources = helper::with_helper_source(&load_sources(&shared.lock().options), state);
        super::search::store(shared, &sources);
    }
    let mut guard = shared.helper();
    let tasks = &mut *guard;
    let Some(t) = tasks.task.as_mut().filter(|t| t.id == id) else { return };
    t.worker = false;
    if !t.running {
        // cancelled: the page has moved on
        return;
    }
    t.running = false;
    match r {
        Err(e) => {
            t.error = match t.kind {
                Kind::Check | Kind::Install => tr!(SearchHelperDownloadFailed, error = e),
                Kind::Sites | Kind::Page => tr!(SearchHelperFailed, error = e),
                Kind::Apply => e,
            }
        }
        Ok(Done::Checked { release, question, up_to_date }) => {
            t.version = release.version.clone();
            t.question = question;
            t.up_to_date = up_to_date.is_some();
            t.text = up_to_date.unwrap_or_default();
            tasks.release = Some(release);
        }
        Ok(Done::Installed(state)) => {
            t.text = tr!(SearchHelperInstalled, version = state.version);
            t.version = state.version;
        }
        Ok(Done::Listed(list)) => tasks.sites = list,
        Ok(Done::Applied(count)) => t.text = tr!(SearchHelperSitesDone, count = count),
        Ok(Done::Page(url, password)) => t.page = Some((url, password)),
    }
}

fn reply_id(id: u64) -> Response {
    Response::json(&Json::obj().with("id", id))
}

fn flag(body: &Json, key: &str) -> bool {
    body.get(key).and_then(Json::as_bool).unwrap_or(false)
}

/// `/pa/search/helper` and `/pa/search/helper/...` (`endpoint` is the part after `helper`);
/// `local`: the request comes from this computer (not through a reverse proxy).
pub(crate) fn serve(
    shared: &Shared,
    endpoint: &str,
    body: Option<Json>,
    session: &str,
    local: bool,
) -> Result<Response, CmdError> {
    let Some(body) = body else {
        return match endpoint {
            "" => Ok(Response::json(&model(shared, local))),
            _ => Err(err(405, "POST only")),
        };
    };
    if !configure(shared) {
        return Err(err(404, "the search helper is not offered here"));
    }
    let installed = helper::installed();
    match endpoint {
        "/check" => {
            let update = flag(&body, "update");
            if update && !installed {
                return Err(err(409, tr!(SearchHelperNotInstalled)));
            }
            let proxy = proxy(shared);
            let id = start(shared, Kind::Check, !installed, move |_| {
                let release = helper::check(proxy)?;
                let size = fmt::size(release.asset.size);
                let current = helper::state().map(|s| s.version).unwrap_or_default();
                let (question, up_to_date) = if update && current == release.version {
                    (String::new(), Some(tr!(SearchHelperUpToDate, version = current)))
                } else if update {
                    let q = tr!(
                        SearchHelperUpdateConfirm,
                        current = current,
                        version = release.version,
                        size = size
                    );
                    (q, None)
                } else {
                    let folder = helper::layout().map(|l| l.root.display().to_string()).unwrap_or_default();
                    (tr!(SearchHelperConfirm, version = release.version, size = size, folder = folder), None)
                };
                Ok(Done::Checked { release, question, up_to_date })
            })?;
            Ok(reply_id(id))
        }
        "/install" => {
            let version = body.get("version").and_then(Json::as_str).unwrap_or("");
            let release = shared.helper().release.clone().filter(|r| r.version == version);
            let release = release.ok_or_else(|| err(409, "check for the newest release first"))?;
            super::search::check_password(shared, &body, session)
                .map_err(|_| err(400, tr!(WebHelperNeedPassword)))?;
            let proxy = proxy(shared);
            let id = start(shared, Kind::Install, !installed, move |p| {
                helper::install(&release, proxy, &mut |got, total| p.update(got, total)).map(Done::Installed)
            })?;
            Ok(reply_id(id))
        }
        "/sites" => {
            if !installed {
                return Err(err(409, tr!(SearchHelperNotInstalled)));
            }
            Ok(reply_id(start(shared, Kind::Sites, false, |_| helper::list_sites().map(Done::Listed))?))
        }
        "/apply" => {
            let list = shared.helper().sites.clone();
            if list.is_empty() {
                return Err(err(409, "list the sites first"));
            }
            let ticked: Vec<String> = match body.get("ticked") {
                Some(Json::Arr(v)) => v.iter().filter_map(Json::as_str).map(str::to_owned).collect(),
                _ => return Err(err(400, "missing ticked")),
            };
            let count =
                list.iter().filter(|i| !i.needs_flaresolverr && ticked.iter().any(|t| t == &i.id)).count();
            let id = start(shared, Kind::Apply, false, move |_| {
                helper::apply_sites(&list, &ticked)
                    .map(|()| Done::Applied(count))
                    .map_err(|e| tr!(SearchHelperSitesFailed, errors = e.join("\n")))
            })?;
            Ok(reply_id(id))
        }
        "/page" => {
            if !local {
                return Err(err(403, tr!(WebHelperPageHostOnly)));
            }
            if !installed {
                return Err(err(409, tr!(SearchHelperNotInstalled)));
            }
            Ok(reply_id(start(shared, Kind::Page, false, |_| {
                helper::page().map(|(url, password)| Done::Page(url, password))
            })?))
        }
        "/cancel" => {
            let want = body.get("id").and_then(Json::as_i64);
            let mut t = shared.helper();
            if let Some(task) = t.task.as_mut().filter(|t| t.running && want.is_none_or(|w| w == t.id as i64))
            {
                task.cancel.store(true, Ordering::Relaxed);
                task.running = false;
                task.cancelled = true;
                task.error = tr!(WebHelperCancelled).to_owned();
            }
            Ok(Response::new(204, "", ""))
        }
        "/remove" => {
            {
                let mut t = shared.helper();
                if t.task.as_ref().is_some_and(|x| x.worker) {
                    return Err(err(409, tr!(WebHelperBusy)));
                }
                *t = Tasks { last_id: t.last_id, ..Tasks::default() };
            }
            let state = helper::remove().map_err(|e| err(500, e))?;
            if let Some(state) = state {
                let sources = helper::without_helper_source(&load_sources(&shared.lock().options), &state);
                super::search::store(shared, &sources);
            }
            Ok(Response::json(&Json::obj().with("text", tr!(SearchHelperRemoved))))
        }
        _ => Err(err(404, "no such endpoint")),
    }
}
