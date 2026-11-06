//! The toolkit-free controller a GUI drives (D61): GTK's `App` without its widgets.
//!
//! [`Core`] holds the session, the torrent list and everything only the frontend knows (labels,
//! pause marks, magnets awaiting metadata, the scheduler, seeding goals, the `.torrent` folders,
//! RSS, the update check, Auto Shutdown) and does what GTK's `app.rs` does with them. It never
//! draws, asks or opens anything: what a GUI has to show or do comes back as [`Events`] or as
//! return values (dialogs to open, messages, notifications, power actions), and the GUI calls
//! back with the user's answer.
//!
//! Threading: `Core` lives on the GUI's main thread. The engine's alert notification and the
//! workers (RSS fetches, the update check, `.torrent` downloads) call the [`Start::wake`]
//! closure from their own threads; the GUI hands that to its main thread and calls
//! [`Core::on_wake`] there. A 1 s timer calls [`Core::tick`].
//!
//! The parts: `alerts` (the alert loop), `tick` (the timer: scheduler, seeding goals, auto-load,
//! Auto Shutdown, the update check), `commands` (torrent actions, menus, Preferences), `add`
//! (adding torrents, the Add New Torrent dialog's flow), `rss` (the RSS Downloader and the feed
//! view), `status` (status bar, titles), `dirs` (where a GUI keeps its files), `layout` (the
//! macOS main window's layout in the preferences).

mod add;
mod alerts;
mod commands;
mod dirs;
mod layout;
mod rss;
mod status;
mod tick;

#[cfg(test)]
mod tests;

pub use add::{AddRequest, AddStep};
pub use commands::{MenuState, TorrentAction};
pub use dirs::{Dirs, Os, dirs, dirs_for, lock_library};
pub use layout::Layout;
pub use status::{StatusBar, UpdateOffer};

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rookparcel_core::fetch::FetchProxy;
use rookparcel_core::{Alert, Exit, MoveCompleted, Session, SessionHandle, SessionStats, TorrentId};

use crate::add::AwaitingMetadata;
use crate::auto_shutdown::{Action, AutoShutdown, Step};
use crate::detail::{Detail, Tab};
use crate::model::TorrentList;
use crate::options::{GuiOptions, Options};
use crate::prefs::Prefs;
use crate::rss::view::ItemRow;
use crate::rss::{FeedId, RssDownloader, Worker};
use crate::scheduler::{Schedule, Scheduler};
use crate::seeding::SeedingGoal;
use crate::status::ListenProblem;
use crate::torrent_folders::{TorrentFiles, TorrentFolders, Watcher};
use crate::update::{Checker, Manifest, Version};
use crate::{fmt, profile, settings, tr};

/// Who a worker wakes the GUI for; the GUI passes it on to [`Core::on_wake`] on its main thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wakeup {
    /// The engine queued alerts.
    Alerts,
    /// An RSS fetch or download finished.
    Rss,
    /// The update check answered.
    Update,
    /// A `.torrent` download ([`Core::add_url`], [`Core::add_link`]) finished.
    Fetched,
}

/// Called from any thread to wake the GUI's main thread.
pub type Wake = Arc<dyn Fn(Wakeup) + Send + Sync>;

/// The local clock: seconds since 1970 and the zone's offset east of UTC (`None` when it cannot
/// be read). The scheduler's hour, the Logger's time stamps and RSS go by it.
pub type Clock = Arc<dyn Fn() -> Option<(i64, i32)> + Send + Sync>;

/// The system's clock with the offset [`fmt::set_utc_offset`] last set (a GUI keeps that
/// current, or passes a clock of its own that asks the OS each time).
pub fn system_clock() -> Clock {
    Arc::new(|| {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
        Some((now, fmt::utc_offset()))
    })
}

/// A remote window's server (remote mode).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    /// `host:port`, as the title bar shows it.
    pub label: String,
}

/// What [`Core::start`] needs; the GUI makes the session (or the remote connection) first.
pub struct Start {
    /// The engine session, or a remote server's ([`rookparcel_core::remote::connect`]).
    pub session: Session,
    /// In remote mode, the server.
    pub remote: Option<Remote>,
    /// The window's `rookparcel.ini` (in remote mode with the server's settings in it,
    /// [`adopt_server`]).
    pub prefs: Prefs,
    /// The options the session was started with ([`prepare_options`]).
    pub options: Options,
    pub dirs: Dirs,
    pub wake: Wake,
    pub clock: Clock,
    /// This GUI checks for updates at all ([`crate::update::AVAILABLE`] and not a packaged
    /// build that a store updates).
    pub updates: bool,
}

/// The OS action Auto Shutdown asks for (Quit is the app's own exit).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Power {
    Standby,
    Hibernate,
    ShutDown,
}

impl Power {
    /// The power action of an Auto Shutdown action; `None` for Quit.
    pub fn of(action: Action) -> Option<Power> {
        match action {
            Action::Quit => None,
            Action::Standby => Some(Power::Standby),
            Action::Hibernate => Some(Power::Hibernate),
            Action::ShutDown => Some(Power::ShutDown),
        }
    }
}

/// What changed and what the GUI has to do, from one call into [`Core`]. The flags say what to
/// redraw; the rest is to show or do, in order.
#[derive(Debug, Default)]
pub struct Events {
    /// The torrents' cells (values, labels, pause marks): redraw the visible rows.
    pub rows: bool,
    /// The list's view (rows added, removed, moved, filtered): reload it, keeping the selection
    /// by [`TorrentId`].
    pub view: bool,
    /// The sidebar's counts, labels or feeds.
    pub sidebar: bool,
    /// What the detail tabs show (their data or the Logger).
    pub detail: bool,
    /// The status bar ([`Core::status_bar`]) and the tray tip ([`Core::tray_tip`]).
    pub status: bool,
    /// The window title ([`Core::title`]).
    pub title: bool,
    /// The feed items the list shows instead of the torrents ([`Core::feed_rows`]), and an open
    /// RSS Downloader window.
    pub feeds: bool,
    /// Error messages to show: (message, detail).
    pub errors: Vec<(String, String)>,
    /// Information messages to show.
    pub infos: Vec<String>,
    /// Downloads that completed, by name, for a notification (only with Preferences' "Show
    /// balloon notifications", `ui.balloons`).
    pub completed: Vec<String>,
    /// A magnet's metadata arrived: open [`Core::next_magnet_dialog`].
    pub magnet_ready: bool,
    /// Add New Torrent dialogs to open, for `.torrent` files that finished downloading.
    pub adds: Vec<AddRequest>,
    /// A remote window lost its server (the error is in `errors`, the title says so).
    pub remote_lost: bool,
    /// Auto Shutdown: open the countdown (`Start`), update it (`Tick`), close it (`Abort`,
    /// `Act`). `Act`'s consequences come in `quit` and `power_after_exit`, or later in `power`.
    pub auto_shutdown: Option<Step>,
    /// Put the computer on standby or into hibernation now (the resume data is saved).
    pub power: Option<Power>,
    /// Quit the application, as File > Exit does.
    pub quit: bool,
    /// After quitting, once the session has shut down: shut the computer down.
    pub power_after_exit: Option<Power>,
    /// Offer this version in a dialog (OK opens its page).
    pub update: Option<UpdateOffer>,
    /// The status bar's update notice ([`Core::update_notice`]) appeared or went away.
    pub update_notice: bool,
    /// The list's alternating row colours were switched to this (Preferences).
    pub alternate_rows: Option<bool>,
}

impl Events {
    /// Adds `other`'s flags and messages to these.
    pub fn merge(&mut self, other: Events) {
        self.rows |= other.rows;
        self.view |= other.view;
        self.sidebar |= other.sidebar;
        self.detail |= other.detail;
        self.status |= other.status;
        self.title |= other.title;
        self.feeds |= other.feeds;
        self.errors.extend(other.errors);
        self.infos.extend(other.infos);
        self.completed.extend(other.completed);
        self.magnet_ready |= other.magnet_ready;
        self.adds.extend(other.adds);
        self.remote_lost |= other.remote_lost;
        self.auto_shutdown = other.auto_shutdown.or(self.auto_shutdown);
        self.power = other.power.or(self.power);
        self.quit |= other.quit;
        self.power_after_exit = other.power_after_exit.or(self.power_after_exit);
        self.update = other.update.or(self.update.take());
        self.update_notice |= other.update_notice;
        self.alternate_rows = other.alternate_rows.or(self.alternate_rows);
    }
}

/// A finished `.torrent` download: its URL, and the file or why not.
type Fetched = (String, Result<Vec<u8>, String>);

/// The controller: one per main window.
pub struct Core {
    session: Option<Session>,
    handle: SessionHandle,
    remote: Option<Remote>,
    /// A remote window's server went away.
    remote_lost: bool,
    dirs: Dirs,
    wake: Wake,
    clock: Clock,
    /// [`Start::updates`].
    updates_here: bool,
    list: TorrentList,
    stats: SessionStats,
    prefs: Prefs,
    /// The default save path (Preferences > Directories).
    save_path: String,
    /// Torrents seen in a status update since start.
    known: HashSet<TorrentId>,
    /// Torrents seen downloading: their completion is announced.
    downloading: HashSet<TorrentId>,
    /// Alerts a dialog's wait popped (`properties::fetch_trackers`), for the next drain.
    alerts: Vec<Alert>,
    /// What the detail tabs show, and the Logger and Speed history.
    detail: Detail,
    /// The detail tab on screen.
    tab: Tab,
    /// The torrent whose file list was asked for (it is asked once per selection).
    files_requested: Option<TorrentId>,
    /// Magnets waiting for their metadata before the Add New Torrent dialog opens.
    awaiting: AwaitingMetadata,
    /// A magnet's Add New Torrent dialog is open (they open one at a time).
    awaiting_dialog: bool,
    /// Info-hashes to move to the top of the queue once added ("Add to top of queue").
    queue_top: HashSet<String>,
    move_completed: Option<MoveCompleted>,
    seeding_goal: SeedingGoal,
    torrent_folders: TorrentFolders,
    torrent_files: TorrentFiles,
    autoload: Watcher,
    schedule: Schedule,
    scheduler: Scheduler,
    auto_shutdown: AutoShutdown,
    /// The standby or hibernation waiting for the resume data.
    suspend_pending: Option<Power>,
    rss: RssDownloader,
    rss_worker: Worker,
    rss_dirty: bool,
    /// The feed whose items the list shows (`Some(None)`: every feed's), and their rows.
    feed_view: Option<Option<FeedId>>,
    feed_rows: Vec<ItemRow>,
    updates: Checker,
    update_beta: bool,
    /// The update check's answer, from its thread.
    update_result: Arc<Mutex<Option<Result<Manifest, String>>>>,
    /// Finished `.torrent` downloads: (URL, the file or why not).
    fetched: Arc<Mutex<Vec<Fetched>>>,
    /// A listen port another program holds (another profile, D42).
    listen: ListenProblem,
    /// "Show current speed in the title bar".
    title_speed: bool,
    /// What happened since the last [`Events`] were handed out.
    ev: Events,
    last_title: String,
    last_view: u64,
    last_generation: u64,
}

/// `[16:47:12]`, the Logger's time stamp for `unix` at `offset` seconds east of UTC.
pub fn hms(unix: i64, offset: i32) -> String {
    let t = (unix + i64::from(offset)).rem_euclid(86_400);
    format!("[{:02}:{:02}:{:02}]", t / 3600, t % 3600 / 60, t % 60)
}

/// The options a main window starts with, from its `rookparcel.ini` (GTK `build`'s first part):
/// a new profile's or portable folder's first start takes a port of its own (D42, saved at
/// once unless the Setup Guide is to come first); "Randomize port each start" picks a new one.
/// `downloads` is the system's Downloads folder; `remote`: a remote window (the server keeps
/// its own port). Returns the options and whether the Setup Guide is to be shown first (D53),
/// in which case the GUI shows it and calls this again afterwards.
pub fn prepare_options(prefs: &mut Prefs, downloads: &str, remote: bool) -> (Options, bool) {
    let mut opts = Options::load(prefs, downloads);
    // asked before `first_run_settings` writes the first keys
    let first_start = !remote && crate::setup_guide::should_show(prefs);
    if !remote && profile::first_run_settings(prefs, &mut opts.settings) {
        // with the guide, saved once it closes: quitting it leaves the file empty, to ask again
        if !first_start {
            if let Err(e) = prefs.save() {
                eprintln!("cannot save the settings: {e}");
            }
        }
        // a portable folder's Downloads too
        opts = Options::load(prefs, downloads);
    }
    if first_start {
        return (opts, true);
    }
    if opts.gui.random_port && !remote {
        settings::set_listen_port(&mut opts.settings, crate::options::random_port());
        settings::store(prefs, &opts.settings);
    }
    (opts, false)
}

/// A remote window's preferences: the server's engine settings and default folder are the
/// window's; the language is the local client's (`base`, its `rookparcel.ini`).
pub fn adopt_server(prefs: &mut Prefs, hello: &rookparcel_core::remote::Hello, base: &Prefs) {
    settings::store(prefs, &hello.settings);
    prefs.set("dir.download", &hello.save_path);
    match base.get("ui.language") {
        Some(code) => prefs.set("ui.language", code),
        None => prefs.remove("ui.language"),
    }
}

impl Core {
    /// Takes over the session (GTK `build`'s model part): the list's sort from the
    /// preferences, the IP filter, the network adapters, the alert notification (through
    /// [`Start::wake`]) and a first drain of the alerts the engine queued while it started
    /// (their [`Events`] come with the first call that returns some).
    pub fn start(s: Start) -> Core {
        let Start { session, remote, prefs, options: opts, dirs, wake, clock, updates } = s;
        let handle = session.handle();
        let awaiting = AwaitingMetadata::load(&prefs);
        let update_seen = prefs.get("update.seen").and_then(Version::parse);
        let tab = prefs.get("ui.tab").and_then(Tab::from_key).unwrap_or(Tab::General);
        let rss = RssDownloader::load(&dirs.settings.join(crate::rss::store::FILE_NAME));
        let w = wake.clone();
        let mut core = Core {
            session: Some(session),
            handle: handle.clone(),
            remote,
            remote_lost: false,
            dirs,
            wake: wake.clone(),
            clock,
            updates_here: updates,
            list: TorrentList::new(),
            stats: SessionStats::default(),
            save_path: opts.gui.download_dir.clone(),
            prefs,
            known: HashSet::new(),
            downloading: HashSet::new(),
            alerts: Vec::new(),
            detail: Detail::default(),
            tab,
            files_requested: None,
            awaiting,
            awaiting_dialog: false,
            queue_top: HashSet::new(),
            move_completed: opts.gui.move_completed(),
            seeding_goal: opts.gui.seeding_goal(),
            torrent_folders: opts.gui.torrent_folders.clone(),
            torrent_files: TorrentFiles::new(),
            autoload: Watcher::new(),
            schedule: opts.gui.schedule.clone(),
            scheduler: Scheduler::new(),
            auto_shutdown: AutoShutdown::default(),
            suspend_pending: None,
            rss,
            rss_worker: Worker::new(move || w(Wakeup::Rss)),
            rss_dirty: false,
            feed_view: None,
            feed_rows: Vec::new(),
            updates: Checker::new(opts.gui.check_updates && updates, Instant::now(), update_seen),
            update_beta: opts.gui.update_beta,
            update_result: Arc::new(Mutex::new(None)),
            fetched: Arc::new(Mutex::new(Vec::new())),
            listen: ListenProblem::new(settings::listen_port(&opts.settings)),
            title_speed: opts.gui.title_speed,
            ev: Events::default(),
            last_title: String::new(),
            last_view: u64::MAX,
            last_generation: u64::MAX,
        };
        let sort = core.prefs.sort();
        core.list.set_sort(sort);
        // a server applies its own block list
        if opts.gui.ip_filter && core.remote.is_none() {
            core.load_ip_filter(&opts.gui);
        }
        // the engine host's network adapters, for Preferences > Connection (D54)
        handle.post_network_adapters();
        handle.set_alert_notify(move || wake(Wakeup::Alerts));
        // the notification fires when the queue goes from empty to not: drain what the engine
        // queued while it started, or it never would
        core.drain();
        handle.post_torrent_updates();
        handle.post_session_stats();
        core.ev.status = true;
        core.ev.title = true;
        core
    }

    /// Ends the window's session (GTK `shutdown`'s model part): the preferences (with the
    /// detail tab; the GUI puts its layout in [`Core::prefs_mut`] first) and `rss.dat` are
    /// saved, the alert notification is cleared, and the engine starts shutting down. The
    /// [`Exit`] says when it is done (D47's closing window); `None` if it was called before.
    pub fn shutdown(&mut self) -> Option<Exit> {
        let session = self.session.take()?;
        self.prefs.set("ui.tab", self.tab.key());
        if let Err(e) = self.prefs.save() {
            eprintln!("cannot save the settings: {e}");
        }
        let file = self.rss_file();
        if let Err(e) = self.rss.save(&file) {
            eprintln!("cannot save {}: {e}", file.display());
        }
        self.handle.clear_alert_notify();
        Some(session.begin_shutdown())
    }

    /// What happened since the last call that returned [`Events`] (for a GUI that ran commands
    /// returning none).
    pub fn events(&mut self) -> Events {
        self.take_events()
    }

    /// The torrents.
    pub fn list(&self) -> &TorrentList {
        &self.list
    }

    /// The torrents, to change the filter, search or sort (then [`Core::events`]).
    pub fn list_mut(&mut self) -> &mut TorrentList {
        &mut self.list
    }

    /// The detail tabs' data, the Logger and the Speed history.
    pub fn detail(&self) -> &Detail {
        &self.detail
    }

    /// The detail tab on screen.
    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn prefs(&self) -> &Prefs {
        &self.prefs
    }

    /// The preferences, for the GUI's own keys (layout, window size); [`Core::save_prefs`]
    /// writes them.
    pub fn prefs_mut(&mut self) -> &mut Prefs {
        &mut self.prefs
    }

    /// Writes `rookparcel.ini`; a failure goes to the Logger.
    pub fn save_prefs(&mut self) {
        if let Err(e) = self.prefs.save() {
            self.log(tr!(LogPrefsNotSaved, error = e));
        }
    }

    /// The session's handle, for what `Core` does not cover.
    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }

    /// The last session statistics.
    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }

    /// The window's folders.
    pub fn dirs(&self) -> &Dirs {
        &self.dirs
    }

    /// A remote window's server.
    pub fn remote(&self) -> Option<&Remote> {
        self.remote.as_ref()
    }

    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// The default save path.
    pub fn save_path(&self) -> &str {
        &self.save_path
    }

    /// Preferences' frontend options as they are now.
    pub fn gui_options(&self) -> GuiOptions {
        GuiOptions::load(&self.prefs, &self.save_path)
    }

    /// Every option as it is now (what Preferences opens with; the GUI fills in
    /// [`Options::integration`]).
    pub fn options(&self) -> Options {
        Options::load(&self.prefs, &self.save_path)
    }

    /// Every torrent's id.
    pub fn ids(&self) -> Vec<TorrentId> {
        self.list.ids().collect()
    }

    /// The labels in use, for the label menus and the Add dialog.
    pub fn labels(&self) -> Vec<String> {
        self.list.labels().into_iter().map(|(l, _)| l).collect()
    }

    /// The name of the torrent in the list with info-hash `key` (`InfoHashes::key_hex`).
    pub fn in_list(&self, key: &str) -> Option<String> {
        self.list
            .ids()
            .find_map(|id| self.list.get(id).filter(|s| s.info_hashes.key_hex() == key))
            .map(|s| s.name.clone())
    }

    /// The proxy feeds, `.torrent` downloads, searches and the update check go through: the
    /// engine's, when it carries HTTP tracker connections.
    pub fn proxy(&self) -> Option<FetchProxy> {
        FetchProxy::from_settings(&self.options().settings)
    }

    /// A line in the Logger, with the time.
    pub fn log(&mut self, text: impl AsRef<str>) {
        let stamp = (self.clock)().map(|(t, o)| hms(t, o)).unwrap_or_default();
        self.detail.push_log(format!("{stamp} {}", text.as_ref()));
        self.ev.detail = true;
    }

    /// `rss.dat`, beside `rookparcel.ini`.
    fn rss_file(&self) -> PathBuf {
        self.dirs.settings.join(crate::rss::store::FILE_NAME)
    }

    /// Seconds since 1970 by [`Start::clock`].
    fn unix_now(&self) -> i64 {
        (self.clock)().map_or(0, |(t, _)| t)
    }

    /// The events gathered, after bringing the list's view up to date.
    fn take_events(&mut self) -> Events {
        self.list.refresh();
        let view = self.list.view_generation();
        if view != self.last_view {
            self.last_view = view;
            self.ev.view = true;
        }
        let generation = self.list.generation();
        if generation != self.last_generation {
            self.last_generation = generation;
            self.ev.rows = true;
            self.ev.sidebar = true;
        }
        let title = self.title();
        if title != self.last_title {
            self.last_title = title;
            self.ev.title = true;
        }
        std::mem::take(&mut self.ev)
    }
}
