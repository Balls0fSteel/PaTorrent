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
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSMenu, NSSplitViewItem};
use objc2_foundation::{NSArray, NSNotification, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer, NSURL};
use rookparcel_core::util::lockfile::Lock;
use rookparcel_core::{MockConfig, Session, SessionParams, TorrentId};
use rookparcel_frontend::app::{self as fapp, Core, Events, Start, Wakeup};
use rookparcel_frontend::columns::{Align as ColAlign, Column, RowExtra};
use rookparcel_frontend::prefs::Prefs;
use rookparcel_frontend::status::{Category, status_icon};
use rookparcel_frontend::{profile, tr};

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
    let dirs = paths::dirs();
    let mut prefs = Prefs::load(dirs.config.join("rookparcel.ini"));
    // (`true`: the Setup Guide comes before a first start, D53; until slice J builds it, the
    // start goes on with the options as they are, unsaved, so the guide is offered again)
    let (opts, _guide_first) = fapp::prepare_options(&mut prefs, &paths::downloads(), false);
    let session = match args.demo {
        Some(n) => Session::start_mock(
            opts.settings.clone(),
            MockConfig { initial_torrents: n, active_fraction: 0.02, ..MockConfig::default() },
        ),
        None => {
            match fapp::lock_library(&dirs.data) {
                Ok(lock) => LOCK.with(|k| *k.borrow_mut() = lock),
                Err((heading, text)) => return give_up(&heading, &text, mtm),
            }
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
    // the engine's alert notification and the workers wake the main thread through this
    let wake: fapp::Wake = Arc::new(|w: Wakeup| mainq::on_main(move || on_wake(w)));
    let core = Core::start(Start {
        session,
        remote: None,
        prefs,
        options: opts,
        dirs,
        wake,
        clock: fapp::system_clock(),
        // a demo (screenshots, scale runs) never asks the update server
        updates: rookparcel_frontend::update::AVAILABLE && args.demo.is_none(),
    });
    let sort = core.list().sort();
    APP.with(|a| {
        *a.borrow_mut() = Some(App { core, demo: args.demo, started: args.started, reported: false })
    });

    let ui = Rc::new(window::build(sidebar(mtm), torrent_table(&columns, sort, mtm), mtm));
    UI.with(|u| *u.borrow_mut() = Some(ui.clone()));
    ui.window.center();
    ui.window.makeKeyAndOrderFront(None);
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
    crate::snapshots::start(mtm);
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
    if ev.status {
        update_status(&ui);
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
        NSApplication::sharedApplication(mtm).terminate(None);
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

/// The status bar (`Core::status_bar`); slice C adds the update link and the network part's
/// click.
fn update_status(ui: &Ui) {
    let Some(s) = peek(|c| c.status_bar()) else { return };
    let st = &ui.status;
    st.network.setStringValue(&NSString::from_str(&s.net));
    st.network.setToolTip(Some(&NSString::from_str(&s.net_tip)));
    st.dht.setStringValue(&NSString::from_str(&s.dht));
    st.down.setStringValue(&NSString::from_str(&s.down));
    st.up.setStringValue(&NSString::from_str(&s.up));
    st.net_icon.setImage(Some(&images::state(s.net_icon, images::is_dark(&st.net_icon))));
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
        || peek(|c| c.list().view().len()).unwrap_or(0),
        move |row, i, dark| {
            let col = cells[i];
            peek(|c| {
                let Some(&id) = c.list().view().get(row as usize) else { return CellValue::Empty };
                let Some(st) = c.list().get(id) else { return CellValue::Empty };
                let x: RowExtra<'_> = c.list().extra(id);
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
    hooks.id = Box::new(|row| peek(|c| c.list().view().get(row).map(|id| id.to_raw())).flatten());
    hooks.rows_of = Box::new(|ids| {
        let want: std::collections::HashSet<u64> = ids.iter().copied().collect();
        peek(|c| {
            c.list()
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
            let list = c.list_mut();
            list.set_sort(Some((col, !ascending)));
            // the table reloads right after this hook: the new order must be there
            list.refresh();
            let sort = list.sort();
            c.prefs_mut().set_sort(sort);
            c.save_prefs();
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
        with_core(|c| c.prefs_mut().set_columns(&out));
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
    let Some(mtm) = MainThreadMarker::new() else { return };
    crate::snapshots::log_command(cmd);
    use Command::*;
    match cmd {
        Quit => NSApplication::sharedApplication(mtm).terminate(None),
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

/// Before the process ends: `Core::shutdown` saves the prefs and `rss.dat` and stops the engine;
/// this waits for it (slice G puts the closing window, D47, in front of this).
pub fn shutdown() {
    UI.with(|u| u.borrow_mut().take());
    if let Some(Some(exit)) = with_core(|c| c.shutdown()) {
        exit.wait();
    }
}
