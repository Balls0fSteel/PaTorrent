//! The application's state and its one command dispatch (docs/MACOS_GUI.md "Threading and
//! state"):
//!
//! - `APP` (`thread_local! RefCell<Option<App>>`) holds the engine side, reached through
//!   [`with_core`] (a mutable borrow) and [`peek`] (a shared one, for data sources); the windows'
//!   handles are apart, in [`ui()`], so AppKit can be called without borrowing `APP`
//!   (re-entrancy rule: compute under the borrow, drop it, then call AppKit).
//! - [`run`] runs every [`Command`], [`state`] and [`title`] answer the menus' and toolbar's
//!   validation.
//! - The engine wakes the main thread through `set_alert_notify` → `mainq::on_main` →
//!   [`on_alerts`]; the 1 s `NSTimer` in the common run-loop modes calls [`tick`].
//!
//! The engine side is `frontend::app::Core` (D61): its [`Events`] say what to redraw and what to
//! show; [`apply`] does that on the main thread after the borrow is dropped.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSApplicationTerminateReply, NSMenu, NSSplitViewItem,
};
use objc2_foundation::{NSArray, NSNotification, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer, NSURL};
use objc2_user_notifications::{
    UNNotification, UNNotificationPresentationOptions, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use rookparcel_core::util::lockfile::Lock;
use rookparcel_core::{MockConfig, Session, SessionParams, TorrentId};
use rookparcel_frontend::app::{self as fapp, Core, Events, Layout, Start, Wakeup};
use rookparcel_frontend::prefs::Prefs;
use rookparcel_frontend::{profile, tr};

use crate::commands::Command;
use crate::window::{self, Ui};
use crate::{canvas, mainq, menu, paths, ui};

/// What `main` hands over.
pub struct Args {
    /// `--demo N`: the simulated engine with N torrents.
    pub demo: Option<usize>,
    /// When the process started (for the time until the list shows).
    pub started: Instant,
    /// Slice J: `--remote`, a remote window (`remote`).
    pub remote: bool,
}

/// The application: the controller, and what `--demo` measures.
pub struct App {
    pub core: Core,
    demo: Option<usize>,
    started: Instant,
    /// The demo's time to its full list was logged.
    reported: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
    /// The library's lock (`rookparcel.lock`), held until the process ends.
    static LOCK: RefCell<Option<Lock>> = const { RefCell::new(None) };
}

/// Runs `f` on the application (a mutable borrow); `None` before the start or while borrowed.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Runs `f` on the core (a mutable borrow); `None` before the start or while it is borrowed
/// (a re-entrant call from AppKit), which then does nothing.
pub fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(|a| f(&mut a.core)))
}

/// Reads the core (a shared borrow, as data sources do).
pub fn peek<R>(f: impl FnOnce(&Core) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow().ok()?.as_ref().map(|a| f(&a.core)))
}

/// The main window's parts.
pub fn ui() -> Option<Rc<Ui>> {
    UI.with(|u| u.borrow().clone())
}

/// A line in the Logger (`Core::log` stamps it), and on stderr for the VM runs' logs.
pub fn log_line(text: &str) {
    eprintln!("{text}");
    with_core(|c| c.log(text));
}

/// What an unbuilt entry point logs.
pub fn not_yet(what: &str) {
    log_line(&format!("not yet: {what}"));
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and RPAppDelegate does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RPAppDelegate"]
    #[ivars = RefCell<Option<Args>>]
    pub struct RPAppDelegate;

    unsafe impl NSObjectProtocol for RPAppDelegate {}

    unsafe impl NSApplicationDelegate for RPAppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            if let Some(args) = self.ivars().borrow_mut().take() {
                // slice G: completed-download notifications show while Rookparcel is in front
                crate::mac::notify::set_delegate(ProtocolObject::from_ref(self));
                start(MainThreadMarker::from(self), args);
            }
        }

        /// Slice G: Quit and logout wait for the engine behind the closing window (D47).
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            crate::mac::lifecycle::should_terminate(MainThreadMarker::from(self))
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn after_last_window(&self, _app: &NSApplication) -> bool {
            crate::mac::lifecycle::after_last_window_closed()
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            // (after `applicationShouldTerminate:`'s wait there is nothing left to stop)
            if let Some(exit) = shutdown() {
                exit.wait();
            }
        }

        /// A click on the Dock icon with no window shown brings the main window back.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, visible: bool) -> bool {
            crate::mac::dock::reopen(visible)
        }

        #[unsafe(method_id(applicationDockMenu:))]
        fn dock_menu(&self, _app: &NSApplication) -> Option<Retained<NSMenu>> {
            let visible = ui().is_some_and(|u| u.window.isVisible());
            Some(menu::build("", &menu::dock(visible), MainThreadMarker::from(self)))
        }

        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            let urls: Vec<String> = urls.iter().filter_map(|u| u.absoluteString().map(|s| s.to_string())).collect();
            crate::mac::open::open(urls);
        }
    }

    // slice G: notifications also while Rookparcel is the active application
    unsafe impl UNUserNotificationCenterDelegate for RPAppDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _n: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((crate::mac::notify::present_options(),));
        }
    }
);

impl RPAppDelegate {
    pub fn new(args: Args, mtm: MainThreadMarker) -> Retained<RPAppDelegate> {
        let this = RPAppDelegate::alloc(mtm).set_ivars(RefCell::new(Some(args)));
        // SAFETY: NSObject's `init` on a fresh object with its ivars set.
        unsafe { msg_send![super(this), init] }
    }
}

/// Shows `message` and `detail` and ends the application (before the window exists).
fn give_up(message: &str, detail: &str, mtm: MainThreadMarker) {
    ui::alert(ui::Style::Error, message, detail, &[], mtm);
    NSApplication::sharedApplication(mtm).terminate(None);
}

/// Starts the engine (the simulated one for `--demo`) and `Core` on it, builds the menus and
/// the window, and starts the timers (GTK `build`).
fn start(mtm: MainThreadMarker, args: Args) {
    canvas::init();
    // one main window, no document tabs (and no Show Tab Bar items in View)
    objc2_app_kit::NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
    menu::build_main(mtm);
    // (from the start: the Setup Guide and the Connect to Remote dialog come first)
    crate::snapshots::start(mtm);
    // slice J: a named profile without folders asks first; a remote window connects first
    if !crate::profiles::ensure_exists() {
        return NSApplication::sharedApplication(mtm).terminate(None);
    }
    if args.remote {
        return crate::remote::ask(move |c| open(mtm, args, Some(c)));
    }
    open(mtm, args, None)
}

/// The window on a new engine session (after the Setup Guide at a first start), or on a remote
/// server's (slice J).
fn open(mtm: MainThreadMarker, args: Args, remote: Option<crate::remote::Connected>) {
    let remote_window = remote.is_some();
    let (dirs, prefs, opts, remote, session) = match remote {
        Some(c) => {
            let (dirs, prefs, opts) = crate::remote::prepare(&c);
            (dirs, prefs, opts, Some(fapp::Remote { label: c.label }), Some(c.session))
        }
        None => {
            let dirs = paths::dirs();
            // before the Setup Guide: a second start meanwhile hands over to this one
            if args.demo.is_none() && LOCK.with(|k| k.borrow().is_none()) {
                match fapp::lock_library(&dirs.data) {
                    Ok(lock) => LOCK.with(|k| *k.borrow_mut() = lock),
                    // slice G: a Rookparcel of this profile runs: it gets what this start opens
                    Err(_) if crate::mac::open::forward_to_running(&dirs.data, mtm) => return,
                    Err((heading, text)) => return give_up(&heading, &text, mtm),
                }
            }
            let mut prefs = Prefs::load(dirs.config.join("rookparcel.ini"));
            let (opts, guide_first) = fapp::prepare_options(&mut prefs, &paths::downloads(), false);
            // slice J: the Setup Guide before a first start (D53; not for a demo), then this
            // again, which finds the file no longer empty
            if guide_first && args.demo.is_none() {
                return crate::setup_guide::first_start(opts, prefs, move || open(mtm, args, None));
            }
            (dirs, prefs, opts, None, None)
        }
    };
    let session = match (session, args.demo) {
        (Some(s), _) => s,
        (None, Some(n)) => Session::start_mock(
            opts.settings.clone(),
            MockConfig {
                initial_torrents: n,
                active_fraction: 0.02,
                // slice G: to look at the closing window (D47)
                slow_exit: crate::mac::lifecycle::demo_slow_exit(),
                ..MockConfig::default()
            },
        ),
        (None, None) => {
            let p = profile::current();
            match Session::start(SessionParams {
                settings: opts.settings.clone(),
                state_dir: Some(dirs.data.clone()),
                label: p.engine_label(),
                portable_root: p.portable_root.clone(),
            }) {
                Ok(s) => s,
                Err(e) => return give_up(tr!(CannotStartEngine), &e.to_string(), mtm),
            }
        }
    };
    let columns = prefs.columns();
    let layout = Layout::load(&prefs);
    // the engine's alert notification and the workers wake the main thread through this
    let wake: fapp::Wake = Arc::new(|w: Wakeup| mainq::on_main(move || on_wake(w)));
    let core = Core::start(Start {
        session,
        remote,
        prefs,
        options: opts,
        dirs,
        wake,
        clock: fapp::system_clock(),
        // a demo (screenshots, scale runs) never asks the update server
        updates: rookparcel_frontend::update::AVAILABLE && args.demo.is_none(),
    });
    let sort = core.list().sort();
    let alternate_rows = core.gui_options().alternate_rows;
    APP.with(|a| {
        *a.borrow_mut() = Some(App { core, demo: args.demo, started: args.started, reported: false })
    });

    let list = crate::list::build(&columns, sort, alternate_rows, mtm);
    let ui = Rc::new(window::build(crate::sidebar::build(mtm), list, mtm));
    UI.with(|u| *u.borrow_mut() = Some(ui.clone()));
    crate::detail::install(&ui);
    ui.apply_layout(&layout);
    crate::sidebar::select_all_torrents();
    ui.window.makeKeyAndOrderFront(None);
    ui.apply_sizes(&layout);
    NSApplication::sharedApplication(mtm).activate();
    // what the first drain inside `Core::start` found
    apply(with_core(|c| c.events()));

    let target = ui::target(mtm);
    // SAFETY: `tick:` is RPTarget's timer action; the run loop keeps the timer, the timer its
    // target. Common modes: it fires during menu tracking and live resizing too.
    unsafe {
        let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            1.0,
            &target,
            sel!(tick:),
            None,
            true,
        );
        NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
    }
    // slice G: the status item, the translocation notice, --minimized, the association check,
    // what was opened before (slice J: a remote window opens nothing local)
    if remote_window {
        crate::remote::started(mtm)
    } else {
        crate::mac::started(mtm)
    }
    log_line(&format!(
        "{} started{}",
        profile::name_build(),
        args.demo.map_or(String::new(), |n| format!(" (demo, {n} torrents)"))
    ));
}

/// The 1 s timer (`Core::tick`: status requests, scheduler, auto-load, RSS, Auto Shutdown,
/// the update check).
pub fn tick() {
    apply(with_core(|c| c.tick(Instant::now())));
}

/// The engine's alerts, drained into the list, the detail tabs and the Logger.
pub fn on_alerts() {
    apply(with_core(|c| c.on_alerts()));
}

/// A worker's or the engine's wakeup, on the main thread.
fn on_wake(w: Wakeup) {
    apply(with_core(|c| c.on_wake(w)));
}

/// Does what `Core`'s events ask, with no borrow held: the window redraws what changed and
/// shows the messages. The parts later slices build log "not yet" until then.
pub fn apply(ev: Option<Events>) {
    let Some(ev) = ev else { return };
    let Some(ui) = ui() else { return };
    if ev.view {
        ui.list.reload_keep_selection();
    } else if ev.rows {
        ui.list.reload_visible();
    }
    if ev.sidebar {
        crate::sidebar::update();
    }
    if let Some(on) = ev.alternate_rows {
        ui.list_view.setUsesAlternatingRowBackgroundColors(on);
    }
    if ev.status || ev.update_notice {
        crate::statusbar::update(&ui);
    }
    if ev.title {
        if let Some(t) = peek(|c| c.title()) {
            ui.window.setTitle(&NSString::from_str(&t));
        }
    }
    report_list_time(&ui);
    let Some(mtm) = MainThreadMarker::new() else { return };
    for (message, detail) in &ev.errors {
        ui::sheet(Some(&ui.window), ui::Style::Error, message, detail, &[], |_| {}, mtm);
    }
    for message in &ev.infos {
        ui::sheet(Some(&ui.window), ui::Style::Info, message, "", &[], |_| {}, mtm);
    }
    if !ev.completed.is_empty() {
        crate::mac::notify::completed(&ev.completed);
    }
    // (one line per slice: each fills its own module's function)
    if ev.detail {
        crate::detail::refresh();
    }
    if ev.status {
        crate::mac::status_item::update();
    }
    if ev.magnet_ready || !ev.adds.is_empty() {
        crate::dialogs::add::from_events(ev.magnet_ready, ev.adds);
    }
    crate::mac::power::on_events(ev.auto_shutdown, ev.power, ev.power_after_exit);
    crate::mac::update::on_events(ev.update, ev.update_notice);
    if ev.quit {
        // slice G: Auto Shutdown's Quit, as the menu's
        crate::mac::lifecycle::quit();
    }
}

/// Logs once how long the demo took until all its rows were in the table.
fn report_list_time(ui: &Ui) {
    let rows = ui.list_view.numberOfRows() as usize;
    let done = with_app(|a| match a.demo {
        Some(n) if !a.reported && n > 0 && rows >= n => {
            a.reported = true;
            Some(a.started.elapsed())
        }
        _ => None,
    })
    .flatten();
    if let Some(t) = done {
        log_line(&format!("demo: {rows} rows in the list {} ms after the start", t.as_millis()));
    }
}

/// The labels the Label submenu lists.
pub fn labels() -> Vec<String> {
    peek(|c| c.labels()).unwrap_or_default()
}

/// File > Profiles' names (`None`: the default profile).
pub fn profile_names() -> Vec<Option<String>> {
    profile::menu(&paths::profiles_root()).into_iter().map(|(n, _)| n).collect()
}

/// The selected torrents.
pub fn selected() -> Vec<TorrentId> {
    ui().map(|u| u.list.selected().into_iter().map(TorrentId::from_raw).collect()).unwrap_or_default()
}

fn show_window() {
    if let Some(u) = ui() {
        u.window.makeKeyAndOrderFront(None);
    }
}

/// Runs a command: the one place every menu item, toolbar button, Dock and status item entry
/// comes to (convention 2).
pub fn run(cmd: Command) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    crate::snapshots::log_command(cmd);
    use Command::*;
    match cmd {
        // slice G: sheets that would hold the quit up go first
        Quit => crate::mac::lifecycle::quit(),
        Find => {
            if crate::detail::find() {
                return;
            }
            if let Some(u) = ui() {
                u.window.makeFirstResponder(Some(&u.search));
            }
        }
        Search => {
            let Some(u) = ui() else { return };
            let text = u.search.stringValue().to_string();
            if !crate::search::filters_names() {
                return crate::search::run(&text);
            }
            apply(with_core(|c| {
                c.set_search(&text);
                c.events()
            }));
        }
        ToggleToolbar => {
            if let Some(u) = ui() {
                u.toolbar.setVisible(!u.toolbar.isVisible());
            }
        }
        UpdateNotice | NetworkStatus => crate::statusbar::run(cmd),
        ToggleSidebar => {
            if let Some(u) = ui() {
                collapse(&u.sidebar_item, !u.sidebar_item.isCollapsed());
            }
        }
        ToggleDetails => {
            if let Some(u) = ui() {
                collapse(&u.details_item, !u.details_item.isCollapsed());
            }
        }
        ToggleStatusBar => {
            if let Some(u) = ui() {
                u.set_status_visible(!u.status_visible());
            }
        }
        ToggleWindow => crate::mac::lifecycle::toggle_window(),
        // slice G: the status item's button, Quit Now, the countdown's Cancel
        StatusItem | QuitNow | CancelShutdown => crate::mac::run(cmd),
        About => crate::mac::about::about(),
        CheckUpdates => crate::mac::update::check(),
        Website => crate::mac::about::website(),
        Notices => crate::mac::about::notices(),
        Preferences => crate::prefs::open(),
        Pref(i) => crate::prefs::run(i),
        SetupGuide => crate::setup_guide::open(),
        AddTorrent => crate::dialogs::add::open_files(),
        AddUrl => crate::dialogs::add_url::open(),
        CreateTorrent => crate::dialogs::create::open(),
        ImportQbt => crate::import::import(true),
        ImportUt => crate::import::import(false),
        Profile(i) => crate::profiles::start(i as usize),
        NewProfile => crate::profiles::new_profile(),
        ProfileFolder => crate::profiles::open_folder(),
        ConnectRemote => crate::remote::connect(),
        Rss => crate::rss::open(),
        AutoShutdown(i) => crate::mac::power::auto_shutdown(i as usize),
        SearchNames | SearchSource(_) | SearchAllSources => crate::search::set_mode(cmd),
        ManageSources => crate::search_helper::manage_sources(),
        Properties => crate::dialogs::properties::open(&ui().map(|u| u.list.selected()).unwrap_or_default()),
        StartAll | StopAll | OpenFolder | ForceStart | Start | Pause | Stop | QueueUp | QueueDown
        | QueueTop | QueueBottom | Remove | RemoveAndDelete | Recheck | UpdateTracker | SetLocation
        | InitialSeeding | DownLimit(_) | UpLimit(_) | NewLabel | RemoveLabel | Label(_) => {
            crate::torrent_menu::run(cmd)
        }
        FileFilter | FileOpen | FileOpenFolder | FileByOrder | FilePriority(_) | FileRelocate => {
            crate::detail::run(cmd)
        }
        Dialog(v) => crate::dialogs::run(v),
    }
}

/// Collapses or expands a split view item.
fn collapse(item: &NSSplitViewItem, collapsed: bool) {
    item.setCollapsed(collapsed);
}

/// Whether a command is enabled now, and whether it shows a check mark.
pub fn state(cmd: Command) -> (bool, Option<bool>) {
    if let Some(s) = crate::detail::state(cmd) {
        return s;
    }
    // slice G: the current mode checked, Hibernate greyed
    if let Command::AutoShutdown(i) = cmd {
        return crate::mac::power::state(i);
    }
    if cmd.needs_selection() {
        return crate::torrent_menu::state(cmd);
    }
    let checked = match cmd {
        Command::Profile(i) => {
            let me = profile::menu(&paths::profiles_root()).get(i as usize).is_some_and(|(_, me)| *me);
            Some(me)
        }
        Command::SearchNames => Some(crate::search::filters_names()),
        _ => None,
    };
    let enabled = match cmd {
        Command::StartAll | Command::StopAll => peek(|c| !c.list().is_empty()).unwrap_or(false),
        // slice J: the client's files are on this Mac, not a remote server
        Command::ImportQbt | Command::ImportUt => crate::import::enabled(),
        _ => true,
    };
    (enabled, checked)
}

/// The text of an item that follows the state (Show or Hide), `None` for the others.
pub fn title(cmd: Command) -> Option<String> {
    let u = ui()?;
    let t = match cmd {
        Command::ToggleToolbar if u.toolbar.isVisible() => tr!(MacHideToolbar),
        Command::ToggleToolbar => tr!(MenuShowToolbar),
        Command::ToggleSidebar if !u.sidebar_item.isCollapsed() => tr!(MacHideSidebar),
        Command::ToggleSidebar => tr!(MacShowSidebar),
        Command::ToggleDetails if !u.details_item.isCollapsed() => tr!(MacHideDetails),
        Command::ToggleDetails => tr!(MenuShowDetails),
        Command::ToggleStatusBar if u.status_visible() => tr!(MacHideStatusBar),
        Command::ToggleStatusBar => tr!(MenuShowStatus),
        Command::ToggleWindow if u.window.isVisible() => tr!(MenuHide),
        Command::ToggleWindow => tr!(MenuShow),
        Command::OpenFolder => crate::torrent_menu::open_folder_title(),
        _ => return None,
    };
    Some(ui::text(t))
}

/// Quit (`applicationShouldTerminate:`, `mac::lifecycle`): `Core::shutdown` saves the prefs and
/// `rss.dat` and starts stopping the engine; the closing window (D47) follows the [`Exit`]
/// until it is done. `None` before the start or once called. The Preferences field being typed
/// in and the window's layout go into the prefs first.
///
/// [`Exit`]: rookparcel_core::Exit
pub fn shutdown() -> Option<rookparcel_core::Exit> {
    // a Preferences field being typed in is applied first
    crate::prefs::before_quit();
    // the window's layout goes into the prefs `Core::shutdown` saves
    if let Some(layout) = ui().map(|u| u.layout()) {
        with_core(|c| layout.store(c.prefs_mut()));
    }
    UI.with(|u| u.borrow_mut().take());
    with_core(|c| c.shutdown()).flatten()
}
