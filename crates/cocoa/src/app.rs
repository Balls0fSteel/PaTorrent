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
//! `Core` here is a temporary stand-in for slice A's `frontend::app::Core`: a session (the
//! simulated one for `--demo N`), its status updates in a `TorrentList`, the prefs and a log.
//! It is kept small so that the merge can replace it.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSMenu, NSSplitViewItem};
use objc2_foundation::{NSArray, NSNotification, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer, NSURL};
use rookparcel_core::{Alert, MockConfig, Session, SessionHandle, SessionParams, SessionStats, TorrentId};
use rookparcel_frontend::columns::{Align as ColAlign, Column, RowExtra};
use rookparcel_frontend::model::TorrentList;
use rookparcel_frontend::options::Options;
use rookparcel_frontend::prefs::Prefs;
use rookparcel_frontend::status::{Category, status_icon};
use rookparcel_frontend::{fmt, profile, tr};

use crate::commands::Command;
use crate::table::{self, Align, CellKind, CellValue, ColumnSpec, Hooks, TableKind};
use crate::window::{self, Ui};
use crate::{canvas, images, mainq, menu, paths, ui};

/// What `main` hands over.
pub struct Args {
    /// `--demo N`: the simulated engine with N torrents.
    pub demo: Option<usize>,
    /// When the process started (for the time until the list shows).
    pub started: Instant,
}

/// The engine side (temporary; slice A's `Core` replaces it).
pub struct Core {
    session: Option<Session>,
    pub handle: SessionHandle,
    pub list: TorrentList,
    pub prefs: Prefs,
    pub stats: SessionStats,
    pub log: Vec<String>,
    alerts: Vec<Alert>,
    demo: Option<usize>,
    started: Instant,
    /// The torrent rows shown at the last refresh (to log when the demo's list is complete).
    shown: usize,
    reported: bool,
}

/// The application.
pub struct App {
    pub core: Core,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
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

/// A line in the Logger (stderr for now; slice D shows `Core`'s log in the Logger tab).
pub fn log_line(text: &str) {
    let line = format!("{} {text}", fmt::date_time(unix_now()));
    eprintln!("{line}");
    with_core(|c| c.log.push(line));
}

/// What an unbuilt entry point logs.
pub fn not_yet(what: &str) {
    log_line(&format!("not yet: {what}"));
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
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
                start(MainThreadMarker::from(self), args);
            }
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            shutdown();
        }

        /// A click on the Dock icon with no window shown brings the main window back.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, visible: bool) -> bool {
            if !visible {
                show_window();
            }
            true
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
);

impl RPAppDelegate {
    pub fn new(args: Args, mtm: MainThreadMarker) -> Retained<RPAppDelegate> {
        let this = RPAppDelegate::alloc(mtm).set_ivars(RefCell::new(Some(args)));
        // SAFETY: NSObject's `init` on a fresh object with its ivars set.
        unsafe { msg_send![super(this), init] }
    }
}

/// The library's lock (`rookparcel.lock`), held until the process ends.
fn lock_library(dir: &std::path::Path) -> Result<(), String> {
    use rookparcel_core::util::lockfile;
    thread_local! {
        static LOCK: RefCell<Option<lockfile::Lock>> = const { RefCell::new(None) };
    }
    let made = rookparcel_core::util::private_file::create_dir_all(dir);
    match made.and_then(|()| lockfile::Lock::take(&dir.join(lockfile::FILE_NAME))) {
        Ok(l) => {
            LOCK.with(|k| *k.borrow_mut() = Some(l));
            Ok(())
        }
        Err(e) if lockfile::is_taken(&e) => Err(tr!(FolderInUse, folder = dir.display())),
        Err(_) => Ok(()),
    }
}

/// Starts the engine (the simulated one for `--demo`), builds the menus and the window, and
/// starts the timers.
fn start(mtm: MainThreadMarker, args: Args) {
    canvas::init();
    // one main window, no document tabs (and no Show Tab Bar items in View)
    objc2_app_kit::NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
    menu::build_main(mtm);
    let dir = paths::data_dir();
    let prefs = Prefs::load(dir.join("rookparcel.ini"));
    let session = match args.demo {
        Some(n) => Session::start_mock(
            rookparcel_core::Settings::default(),
            MockConfig { initial_torrents: n, active_fraction: 0.02, ..MockConfig::default() },
        ),
        None => {
            if let Err(e) = lock_library(&dir) {
                ui::alert(ui::Style::Error, &e, "", &[], mtm);
                NSApplication::sharedApplication(mtm).terminate(None);
                return;
            }
            let opts = Options::load(&prefs, &paths::downloads());
            let p = profile::current();
            match Session::start(SessionParams {
                settings: opts.settings,
                state_dir: Some(dir.join("state")),
                label: p.engine_label(),
                portable_root: p.portable_root.clone(),
            }) {
                Ok(s) => s,
                Err(e) => {
                    ui::alert(ui::Style::Error, tr!(CannotStartEngine), &e.to_string(), &[], mtm);
                    NSApplication::sharedApplication(mtm).terminate(None);
                    return;
                }
            }
        }
    };
    let handle = session.handle();
    let mut list = TorrentList::new();
    list.set_sort(prefs.sort());
    let columns = prefs.columns();
    let core = Core {
        session: Some(session),
        handle: handle.clone(),
        list,
        prefs,
        stats: SessionStats::default(),
        log: Vec::new(),
        alerts: Vec::new(),
        demo: args.demo,
        started: args.started,
        shown: 0,
        reported: false,
    };
    let sort = core.list.sort();
    APP.with(|a| *a.borrow_mut() = Some(App { core }));

    let ui = Rc::new(window::build(sidebar(mtm), torrent_table(&columns, sort, mtm), mtm));
    UI.with(|u| *u.borrow_mut() = Some(ui.clone()));
    ui.window.center();
    ui.window.makeKeyAndOrderFront(None);
    NSApplication::sharedApplication(mtm).activate();

    // the engine thread only queues a wakeup; alerts queued during the start do not notify
    handle.set_alert_notify(|| mainq::on_main(on_alerts));
    handle.post_torrent_updates();
    handle.post_session_stats();
    on_alerts();

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
    crate::snapshots::start(mtm);
    log_line(&format!(
        "{} started{}",
        profile::name_build(),
        args.demo.map_or(String::new(), |n| format!(" (demo, {n} torrents)"))
    ));
}

/// The 1 s timer: asks the engine for the changed torrents and the session's numbers.
pub fn tick() {
    with_core(|c| {
        c.handle.post_torrent_updates();
        c.handle.post_session_stats();
    });
}

/// The engine's alerts: status updates into the list, then the window follows.
pub fn on_alerts() {
    let Some((reload, status)) = with_core(|c| {
        let mut alerts = std::mem::take(&mut c.alerts);
        c.handle.pop_alerts(&mut alerts);
        let mut status = false;
        for a in alerts.drain(..) {
            match a {
                Alert::StateUpdate(v) => c.list.update(v),
                Alert::SessionStats(s) => {
                    c.stats = s;
                    status = true;
                }
                Alert::TorrentRemoved { id, .. } => c.list.remove(id),
                _ => {}
            }
        }
        c.alerts = alerts;
        let generation = c.list.view_generation();
        c.list.refresh();
        (generation != c.list.view_generation(), status)
    }) else {
        return;
    };
    let Some(ui) = ui() else { return };
    if reload {
        ui.list.reload_keep_selection();
    } else {
        ui.list.reload_visible();
    }
    if status {
        update_status(&ui);
    }
    report_list_time(&ui);
}

/// Logs once how long the demo took until all its rows were in the table.
fn report_list_time(ui: &Ui) {
    let rows = ui.list_view.numberOfRows() as usize;
    let done = with_core(|c| {
        c.shown = rows;
        match c.demo {
            Some(n) if !c.reported && n > 0 && rows >= n => {
                c.reported = true;
                Some(c.started.elapsed())
            }
            _ => None,
        }
    })
    .flatten();
    if let Some(t) = done {
        log_line(&format!("demo: {rows} rows in the list {} ms after the start", t.as_millis()));
    }
}

/// The status bar's texts (slice C gives them `Core`'s texts, tooltips and the network icon).
fn update_status(ui: &Ui) {
    let Some((net, dht, down, up, ok)) = peek(|c| {
        let s = &c.stats;
        let net = if s.has_incoming_connections { tr!(NetOk) } else { tr!(NetNoIncoming) };
        (
            net.to_owned(),
            tr!(BarDht, count = s.dht_nodes),
            tr!(BarDown, rate = fmt::speed_always(s.download_rate), total = fmt::size(s.recv_payload_bytes)),
            tr!(BarUp, rate = fmt::speed_always(s.upload_rate), total = fmt::size(s.sent_payload_bytes)),
            s.has_incoming_connections,
        )
    }) else {
        return;
    };
    let st = &ui.status;
    st.network.setStringValue(&NSString::from_str(&net));
    st.dht.setStringValue(&NSString::from_str(&dht));
    st.down.setStringValue(&NSString::from_str(&down));
    st.up.setStringValue(&NSString::from_str(&up));
    let icon = rookparcel_frontend::icons::network_icon(ok);
    st.net_icon.setImage(Some(&images::state(icon, images::is_dark(&st.net_icon))));
}

/// The torrent table: the 24 columns in the prefs' order and widths, over `list.view()`.
fn torrent_table(
    columns: &[(Column, i32)],
    sort: Option<(Column, bool)>,
    mtm: MainThreadMarker,
) -> (Retained<objc2_app_kit::NSScrollView>, Retained<table::RPTableView>, Retained<table::RPTableSource>) {
    let cols: Vec<Column> = columns.iter().map(|(c, _)| *c).collect();
    let specs: Vec<ColumnSpec> = columns
        .iter()
        .map(|&(c, w)| ColumnSpec {
            id: c.key().to_owned(),
            title: c.title().to_owned(),
            width: f64::from(w.unsigned_abs()),
            min_width: 24.0,
            align: if c.align() == ColAlign::Right { Align::Right } else { Align::Left },
            kind: match c {
                Column::Name => CellKind::IconText,
                Column::Done => CellKind::Bar,
                _ => CellKind::Text,
            },
            visible: w > 0,
            hideable: c != Column::Name,
            sortable: true,
            descending_first: c.default_descending(),
        })
        .collect();
    let cells = cols.clone();
    let mut hooks = Hooks::flat(
        || peek(|c| c.list.view().len()).unwrap_or(0),
        move |row, i, dark| {
            let col = cells[i];
            peek(|c| {
                let Some(&id) = c.list.view().get(row as usize) else { return CellValue::Empty };
                let Some(st) = c.list.get(id) else { return CellValue::Empty };
                let x: RowExtra<'_> = c.list.extra(id);
                match col {
                    Column::Name => CellValue::IconText(
                        Some(images::state(status_icon(st, x.user_paused), dark)),
                        st.name.clone(),
                    ),
                    Column::Done => CellValue::Bar(canvas::Bar {
                        fraction: f64::from(st.progress_ppm) / 1e6,
                        complete: st.is_finished,
                        text: col.text(st, x),
                    }),
                    _ => CellValue::Text(col.text(st, x)),
                }
            })
            .unwrap_or(CellValue::Empty)
        },
    );
    hooks.id = Box::new(|row| peek(|c| c.list.view().get(row).map(|id| id.to_raw())).flatten());
    hooks.rows_of = Box::new(|ids| {
        let want: std::collections::HashSet<u64> = ids.iter().copied().collect();
        peek(|c| {
            c.list
                .view()
                .iter()
                .enumerate()
                .filter(|(_, id)| want.contains(&id.to_raw()))
                .map(|(r, _)| r)
                .collect()
        })
        .unwrap_or_default()
    });
    let sorted = cols.clone();
    hooks.sort = Some(Box::new(move |(i, ascending)| {
        let col = sorted[i];
        with_core(|c| {
            c.list.set_sort(Some((col, !ascending)));
            c.list.refresh();
            c.prefs.set_sort(c.list.sort());
            let _ = c.prefs.save();
        });
    }));
    let laid = cols.clone();
    hooks.layout = Some(Box::new(move |order| {
        let mut out: Vec<(Column, i32)> =
            order.iter().map(|&(i, w, v)| (laid[i], if v { w as i32 } else { -(w as i32) })).collect();
        for c in &laid {
            if !out.iter().any(|(d, _)| d == c) {
                out.push((*c, -c.default_width()));
            }
        }
        with_core(|c| c.prefs.set_columns(&out));
    }));
    hooks.activate = Some(Box::new(crate::list::activate));
    hooks.key = Some(Box::new(|k, ids| match k {
        table::Key::Return | table::Key::Delete => crate::list::key(k, ids),
    }));
    hooks.menu = Some(Box::new(move |_ids| {
        Some(menu::build("", &menu::torrent(&labels(), false), MainThreadMarker::new()?))
    }));
    let sort = sort.and_then(|(col, desc)| cols.iter().position(|&c| c == col).map(|i| (i, !desc)));
    table::table(TableKind("torrents"), specs, hooks, sort, mtm)
}

/// The sidebar's rows (slice C fills them from `Core`, with counts and labels).
const SIDEBAR_TORRENTS: u64 = 1;
const SIDEBAR_LABELS: u64 = 2;

fn sidebar(
    mtm: MainThreadMarker,
) -> (
    Retained<objc2_app_kit::NSScrollView>,
    Retained<objc2_app_kit::NSOutlineView>,
    Retained<table::RPTableSource>,
) {
    let mut spec = ColumnSpec::text("name", "", 160.0);
    spec.kind = CellKind::IconText;
    spec.hideable = false;
    let mut hooks = Hooks::flat(
        || 0,
        |id, _, dark| match id {
            SIDEBAR_TORRENTS => CellValue::Text(ui::text(Category::All.label())),
            SIDEBAR_LABELS => CellValue::Text(ui::text(tr!(CatLabels))),
            10..=14 => {
                let c = Category::ALL[(id - 10) as usize];
                CellValue::IconText(
                    Some(images::ui(rookparcel_frontend::icons::category_icon(c), dark)),
                    ui::text(c.label()),
                )
            }
            20 => CellValue::IconText(
                Some(images::ui(rookparcel_frontend::icons::UiIcon::NoLabel, dark)),
                ui::text(tr!(CatNoLabel)),
            ),
            _ => CellValue::Empty,
        },
    );
    hooks.children = Some(Box::new(|parent| match parent {
        None => vec![SIDEBAR_TORRENTS, SIDEBAR_LABELS],
        Some(SIDEBAR_TORRENTS) => (11..=14).collect(),
        Some(SIDEBAR_LABELS) => vec![20],
        Some(_) => Vec::new(),
    }));
    hooks.is_group = Some(Box::new(|id| id == SIDEBAR_TORRENTS || id == SIDEBAR_LABELS));
    hooks.selection = Some(Box::new(crate::sidebar::select));
    table::outline(TableKind("sidebar"), vec![spec], hooks, true, mtm)
}

/// The labels the Label submenu lists.
pub fn labels() -> Vec<String> {
    peek(|c| c.list.labels().into_iter().map(|(l, _)| l).collect()).unwrap_or_default()
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
    let Some(mtm) = MainThreadMarker::new() else { return };
    crate::snapshots::log_command(cmd);
    use Command::*;
    match cmd {
        Quit => NSApplication::sharedApplication(mtm).terminate(None),
        Find => {
            if let Some(u) = ui() {
                u.window.makeFirstResponder(Some(&u.search));
            }
        }
        Search => {
            let Some(u) = ui() else { return };
            let text = u.search.stringValue().to_string();
            with_core(|c| {
                c.list.set_search(&text);
                c.list.refresh();
            });
            u.list.reload_keep_selection();
        }
        ToggleToolbar => {
            if let Some(u) = ui() {
                u.toolbar.setVisible(!u.toolbar.isVisible());
            }
        }
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
        ToggleWindow => match ui() {
            Some(u) if u.window.isVisible() => crate::mac::lifecycle::toggle_window(),
            _ => show_window(),
        },
        About => crate::mac::about::about(),
        CheckUpdates => crate::mac::update::check(),
        Website => crate::mac::about::website(),
        Notices => crate::mac::about::notices(),
        Preferences => crate::prefs::open(),
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
    }
}

/// Collapses or expands a split view item.
fn collapse(item: &NSSplitViewItem, collapsed: bool) {
    item.setCollapsed(collapsed);
}

/// Whether a command is enabled now, and whether it shows a check mark.
pub fn state(cmd: Command) -> (bool, Option<bool>) {
    let checked = match cmd {
        Command::Profile(i) => {
            let me = profile::menu(&paths::profiles_root()).get(i as usize).is_some_and(|(_, me)| *me);
            Some(me)
        }
        Command::SearchNames => Some(true),
        Command::AutoShutdown(i) => Some(i == 0),
        _ => None,
    };
    let enabled = !cmd.needs_selection() || ui().is_some_and(|u| !u.list.selected().is_empty());
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
        _ => return None,
    };
    Some(ui::text(t))
}

/// Before the process ends: the prefs saved, the engine stopped (slice G puts the closing
/// window in front of this).
pub fn shutdown() {
    UI.with(|u| u.borrow_mut().take());
    let session = with_core(|c| {
        c.handle.clear_alert_notify();
        let _ = c.prefs.save();
        c.session.take()
    })
    .flatten();
    if let Some(s) = session {
        s.shutdown();
    }
}
