//! The search helper (D51, `docs/SEARCH_HELPER.md`): for users without a Prowlarr or Jackett
//! of their own, PaTorrent downloads Jackett's own release from GitHub at the user's click,
//! unpacks it into the profile folder (`<profile>/search-helper/`) and runs it, as a separate
//! program over HTTP on `127.0.0.1`, only while searching. Its `all` Torznab endpoint is added
//! as an ordinary search source ("Search helper (all)", [`helper_source`]) with the API key we
//! generated, so the D41 search does the rest. Never bundled, never started without the user's
//! install, never updated on its own.
//!
//! - [`release`]: the newest release, this platform's asset, the download checked against the
//!   asset's SHA-256 from the GitHub API.
//! - [`archive`]: the zip and tar.gz readers (the engine's inflater), unpacking safely.
//! - [`config`]: the folder, `helper.ini` (the version, port, generated key and password,
//!   sealed) and the `ServerConfig.json` written before every start.
//! - [`process`]: the child process: started by the first search ([`route`]), health-checked,
//!   stopped after ten idle minutes ([`process::IDLE`]) and at exit ([`shutdown`]).
//! - [`api`]: the helper's admin API: sign-in, the indexer listing, configuring the ticked
//!   public indexers ([`list_sites`], [`apply_sites`]).
//!
//! Off where downloading and running a program is not ours to do ([`available`]): builds with
//! `PATORRENT_NO_UPDATE_CHECK` (a package manager updates them), a Flatpak, and systems with
//! no Jackett build (FreeBSD).

pub mod api;
pub mod archive;
pub mod config;
pub mod json;
pub mod process;
pub mod release;
pub mod sha512;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

use patorrent_core::fetch::FetchProxy;

pub use api::Indexer;
pub use config::{HelperProxy, Layout, State};
pub use release::Release;

use super::source::Source;
use crate::tr;
use process::{Launch, Runner};

/// Whether this build can offer the helper at all: not without the update check (a package
/// manager's build), and not where Jackett has no build.
/// The Store flavour (`PATORRENT_STORE` at build time, D46, `docs/STORE.md`) leaves it out too.
pub const AVAILABLE: bool =
    crate::update::AVAILABLE && option_env!("PATORRENT_STORE").is_none() && !cfg!(target_os = "freebsd");

/// Whether this process offers the helper: [`AVAILABLE`], an asset for this platform, and not
/// in a Flatpak (a sandbox that downloads and runs programs would not pass Flathub's review).
pub fn available() -> bool {
    AVAILABLE && release::platform_asset().is_some() && std::env::var_os("FLATPAK_ID").is_none()
}

/// The helper's Torznab endpoint for all its configured indexers.
pub fn torznab_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/api/v2.0/indexers/all/results/torznab/")
}

/// The search source the install adds: the helper's `all` endpoint with its key, enabled.
pub fn helper_source(state: &State) -> Source {
    Source {
        name: tr!(SearchHelperSource).to_owned(),
        url: torznab_url(state.port.max(1)),
        api_key: state.api_key.clone(),
        enabled: true,
    }
}

/// Whether `s` is the helper's source (its key is the helper's, on this machine).
pub fn is_helper_source(s: &Source, state: &State) -> bool {
    !state.api_key.is_empty()
        && s.api_key.trim() == state.api_key
        && super::source::url_host(&s.url).is_some_and(|h| h == "127.0.0.1" || h == "localhost")
}

/// `sources` with the helper's source added (or put back to its endpoint and enabled).
pub fn with_helper_source(sources: &[Source], state: &State) -> Vec<Source> {
    let mut out = sources.to_vec();
    match out.iter_mut().find(|s| is_helper_source(s, state)) {
        Some(s) => {
            s.url = torznab_url(state.port.max(1));
            s.enabled = true;
        }
        None => out.push(helper_source(state)),
    }
    out
}

/// `sources` without the helper's.
pub fn without_helper_source(sources: &[Source], state: &State) -> Vec<Source> {
    sources.iter().filter(|s| !is_helper_source(s, state)).cloned().collect()
}

// ---- the process-wide helper -------------------------------------------------------------

/// Where the helper is and what we know of it: short locks only.
struct Shared {
    layout: Option<Layout>,
    /// The installed helper's state, as last read.
    state: Option<State>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared { layout: None, state: None });
/// The proxy the launcher writes into `ServerConfig.json` at each start.
static PROXY: Mutex<Option<HelperProxy>> = Mutex::new(None);
/// The process; held for as long as a start takes, so a search of another source never
/// waits on it (they only look at [`SHARED`]).
static RUNNER: Mutex<Option<Runner>> = Mutex::new(None);
/// Set at the program's exit: a start under way gives up at its next health check.
static EXITING: AtomicBool = AtomicBool::new(false);
static REAPER: Once = Once::new();

fn lock<T>(m: &'static Mutex<T>) -> MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Tells the helper where the profile folder is and which proxy it should use (the GUI's, at
/// start and before each search; cheap). A changed folder stops a helper of the old one.
pub fn configure(profile_dir: &Path, proxy: Option<HelperProxy>) {
    let layout = Layout::new(profile_dir);
    *lock(&PROXY) = proxy;
    let changed = {
        let mut s = lock(&SHARED);
        let changed = s.layout.as_ref() != Some(&layout);
        if changed {
            s.state = State::load(&layout);
            s.layout = Some(layout);
        }
        changed
    };
    if changed {
        if let Some(mut r) = lock(&RUNNER).take() {
            r.kill();
        }
    }
}

/// Reads the helper's state again (after an install, an update or a removal).
fn reload() {
    let mut s = lock(&SHARED);
    s.state = s.layout.as_ref().and_then(State::load);
}

/// The installed helper's state, if there is one.
pub fn state() -> Option<State> {
    lock(&SHARED).state.clone()
}

/// Whether the helper is installed in the configured profile folder.
pub fn installed() -> bool {
    let s = lock(&SHARED);
    s.layout.as_ref().is_some_and(Layout::installed) && s.state.is_some()
}

/// The configured folder's layout.
pub fn layout() -> Option<Layout> {
    lock(&SHARED).layout.clone()
}

/// The arguments Jackett is started with (`src/Jackett.Common/Models/Config/ConsoleOptions.cs`):
/// no self-update, no restart after one, our data folder, local access only. The port is in
/// `ServerConfig.json`: `--Port` on Windows asks for administrator rights when it changes the
/// port (`src/Jackett.Server/Initialisation.cs`).
pub fn launch_args(data: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> =
        ["--NoUpdates", "--NoRestart", "--ListenPrivate"].iter().map(OsString::from).collect();
    a.push("--DataFolder".into());
    a.push(data.as_os_str().to_owned());
    a
}

/// Writes the helper's settings for `port` and says how to start it.
pub fn launch(layout: &Layout, port: u16, proxy: Option<&HelperProxy>) -> Result<Launch, String> {
    let mut state = State::load(layout).ok_or_else(|| tr!(SearchHelperNotInstalled).to_owned())?;
    let exe = layout.exe();
    if !exe.is_file() {
        return Err(tr!(SearchHelperNotInstalled).to_owned());
    }
    if state.port != port {
        state.port = port;
        state.save(layout)?;
    }
    config::write_server_config(&layout.data(), port, &state, proxy)?;
    Ok(Launch {
        cwd: exe.parent().map(Path::to_path_buf),
        program: exe,
        args: launch_args(&layout.data()),
        env: Vec::new(),
        api_key: state.api_key,
    })
}

/// The running helper's port, started first when needed (blocking, up to a minute and a half
/// on its first start).
pub fn ensure_running() -> Result<u16, String> {
    if EXITING.load(Ordering::Relaxed) {
        return Err("exiting".to_owned());
    }
    let (layout, preferred) = {
        let s = lock(&SHARED);
        let layout = s.layout.clone().ok_or_else(|| tr!(SearchHelperNotInstalled).to_owned())?;
        (layout, s.state.as_ref().map_or(0, |st| st.port))
    };
    let mut guard = lock(&RUNNER);
    let r = guard.get_or_insert_with(|| {
        let mut r = Runner::new(Box::new(move |port| {
            let p = lock(&PROXY).clone();
            launch(&layout, port, p.as_ref())
        }));
        r.preferred_port = preferred;
        r.cancel = Some(&EXITING);
        r
    });
    let port = r.ensure()?;
    drop(guard);
    REAPER.call_once(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                if let Some(r) = lock(&RUNNER).as_mut() {
                    r.stop_if_idle(Instant::now());
                }
            }
        });
    });
    if state().is_some_and(|s| s.port != port) {
        reload();
    }
    Ok(port)
}

/// Whether the helper runs now (false while it starts).
pub fn running() -> bool {
    RUNNER.try_lock().ok().is_some_and(|mut r| r.as_mut().is_some_and(Runner::is_running))
}

/// Stops the helper now (before an update or a removal).
pub fn stop() {
    if let Some(r) = lock(&RUNNER).as_mut() {
        r.stop();
    }
}

/// At the program's exit: the helper is killed, nothing waits longer than a start under way
/// takes to notice.
pub fn shutdown() {
    EXITING.store(true, Ordering::Relaxed);
    if let Some(mut r) = lock(&RUNNER).take() {
        r.kill();
    }
}

/// `source` as the search should ask it: the helper's source with the running helper's
/// current endpoint (started first when needed); `None` for every other source.
pub fn route(source: &Source) -> Result<Option<Source>, String> {
    let is_helper = lock(&SHARED).state.as_ref().is_some_and(|st| is_helper_source(source, st));
    if !is_helper {
        return Ok(None);
    }
    let port = ensure_running().map_err(|e| tr!(SearchHelperFailed, error = e))?;
    Ok(Some(Source { url: torznab_url(port), ..source.clone() }))
}

// ---- install, update, remove ------------------------------------------------------------

/// The newest release for this platform (blocking).
pub fn check(proxy: Option<FetchProxy>) -> Result<Release, String> {
    release::latest(proxy)
}

/// Downloads `release` (checked against its SHA-256) and unpacks it into the configured
/// folder, replacing an older one (stopped first); the key and password of an earlier install
/// stay, so the source keeps working. Blocking: a worker thread's. `progress` as
/// [`release::download`]'s.
pub fn install(
    release: &Release,
    proxy: Option<FetchProxy>,
    progress: &mut dyn FnMut(u64, Option<u64>) -> bool,
) -> Result<State, String> {
    let layout = layout().ok_or_else(|| tr!(SearchHelperNotInstalled).to_owned())?;
    let data = release::download(release, proxy, progress)?;
    stop();
    install_archive(&layout, &release.asset.name, &data, &release.version)?;
    reload();
    state().ok_or_else(|| "helper.ini".to_owned())
}

/// Unpacks `data` (an archive named `name`) as the helper's program and records `version`.
pub fn install_archive(layout: &Layout, name: &str, data: &[u8], version: &str) -> Result<State, String> {
    patorrent_core::util::private_file::create_dir_all(&layout.root)
        .map_err(|e| format!("{}: {e}", layout.root.display()))?;
    let fresh = layout.root.join("app.new");
    let _ = std::fs::remove_dir_all(&fresh);
    let unpacked = if name.ends_with(".zip") {
        archive::unpack_zip(data, &fresh)
    } else {
        archive::unpack_tar_gz(data, &fresh)
    };
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(format!("{name}: {e}"));
    }
    let app = layout.app();
    if app.exists() {
        std::fs::remove_dir_all(&app).map_err(|e| format!("{}: {e}", app.display()))?;
    }
    std::fs::rename(&fresh, &app).map_err(|e| format!("{}: {e}", app.display()))?;
    let mut state = State::load(layout).unwrap_or_else(|| State::generate(version));
    state.version = version.to_owned();
    if state.port == 0 {
        state.port = process::free_port(0)?;
    }
    state.save(layout)?;
    if !layout.exe().is_file() {
        return Err(format!("{name}: {} is missing", layout.exe().display()));
    }
    Ok(state)
}

/// Stops the helper and deletes its folder; the caller removes its source
/// ([`without_helper_source`]) with the state this returns.
pub fn remove() -> Result<Option<State>, String> {
    if let Some(mut r) = lock(&RUNNER).take() {
        r.stop();
    }
    let (state, layout) = {
        let mut s = lock(&SHARED);
        (s.state.take(), s.layout.clone())
    };
    if let Some(l) = layout {
        if l.root.exists() {
            std::fs::remove_dir_all(&l.root).map_err(|e| format!("{}: {e}", l.root.display()))?;
        }
    }
    Ok(state)
}

// ---- sites ---------------------------------------------------------------------------------

/// Signs in to the running helper (started first when needed).
pub fn admin() -> Result<api::Admin, String> {
    let port = ensure_running()?;
    let password = state().map(|s| s.password).unwrap_or_default();
    api::Admin::login(port, &password)
}

/// The helper's public indexers, by name, each marked when it needs FlareSolverr (blocking).
pub fn list_sites() -> Result<Vec<Indexer>, String> {
    let defs = layout().map(|l| l.definitions());
    let mut list = admin()?.indexers()?;
    list.retain(Indexer::is_public);
    mark_flaresolverr(&mut list, defs.as_deref());
    list.sort_by_key(|i| i.name.to_lowercase());
    Ok(list)
}

/// Marks the indexers whose definition in `defs` asks for FlareSolverr.
pub fn mark_flaresolverr(list: &mut [Indexer], defs: Option<&Path>) {
    let Some(defs) = defs else { return };
    for i in list {
        let file: PathBuf = defs.join(format!("{}.yml", i.id));
        if let Ok(text) = std::fs::read_to_string(file) {
            i.needs_flaresolverr = api::needs_flaresolverr(&text);
        }
    }
}

/// Makes the configured indexers the `ticked` ones of `list` (FlareSolverr ones are never
/// configured): new ones configured with their defaults, unticked ones removed. The errors,
/// one per indexer (`name: error`), when some failed. Blocking.
pub fn apply_sites(list: &[Indexer], ticked: &[String]) -> Result<(), Vec<String>> {
    let admin = admin().map_err(|e| vec![e])?;
    apply_sites_with(&admin, list, ticked)
}

/// [`apply_sites`] on a signed-in session.
pub fn apply_sites_with(admin: &api::Admin, list: &[Indexer], ticked: &[String]) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for i in list {
        let on = ticked.iter().any(|t| t == &i.id) && !i.needs_flaresolverr;
        let result = match (on, i.configured) {
            (true, false) => admin.configure(&i.id),
            (false, true) => admin.remove(&i.id),
            _ => Ok(()),
        };
        if let Err(e) = result {
            errors.push(format!("{}: {e}", i.name));
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// The helper's own page (its web UI, for private sites and their credentials) and the
/// password it asks for; started first when needed (blocking).
pub fn page() -> Result<(String, String), String> {
    let port = ensure_running()?;
    let password = state().map(|s| s.password).unwrap_or_default();
    Ok((format!("http://127.0.0.1:{port}/UI/Dashboard"), password))
}

#[cfg(test)]
mod tests;
