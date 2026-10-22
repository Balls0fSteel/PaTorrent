//! The application: the engine session, the main window (menu bar, toolbar, category list,
//! torrent list, detail tabs, status bar), the alert loop and the torrent actions.
//!
//! The engine's alert notification runs on the engine thread; it only asks the main loop to
//! drain the queue (`MainContext::invoke`). Everything else happens on the GTK thread, with
//! the state in a thread-local [`App`] (the widgets that draw from it in [`Ui`], so drawing
//! never runs while the state is borrowed).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_core::fetch::{FetchOptions, FetchProxy, http_get};
use patorrent_core::metainfo::{TorrentInfo, parse_magnet_uri};
use patorrent_core::{
    AddTorrentParams, Alert, IpFilter, MoveCompleted, MoveFlags, QueueMove, RemoveFlags, Session,
    SessionHandle, SessionParams, SessionStats, TorrentFlags, TorrentId, TorrentSource, same_dir,
};
use patorrent_frontend::auto_shutdown::{self, AutoShutdown, Mode as AutoMode, Step};
use patorrent_frontend::columns::Column;
use patorrent_frontend::detail::{Detail, PRIORITIES, Tab};
use patorrent_frontend::icons::network_icon;
use patorrent_frontend::integration::{Integration, MINIMIZED};
use patorrent_frontend::lang::{self, gtk_mnemonic};
use patorrent_frontend::model::TorrentList;
use patorrent_frontend::options::{self, GuiOptions, Options};
use patorrent_frontend::prefs::Prefs;
use patorrent_frontend::properties::{self, Properties, TorrentProps};
use patorrent_frontend::rss::view::{self as rss_view, FeedForm, ItemRow};
use patorrent_frontend::rss::{AddRequest, Download, FeedId, RssDownloader, Worker};
use patorrent_frontend::scheduler::{Mode, Schedule, Scheduler};
use patorrent_frontend::seeding::{GoalAction, SeedGoal, SeedingGoal};
use patorrent_frontend::status::status_icon;
use patorrent_frontend::torrent_folders::{self, Poller, TorrentFiles, TorrentFolders};
use patorrent_frontend::{actions, add, create, fmt, settings, tr};

use crate::detail::{Details, Visit};
use crate::list::{Content, List};
use crate::paths;
use crate::shutdown::{self, Power};
use crate::sidebar::{Pick, Sidebar};
use crate::table::Table;
use crate::tray::{Command as TrayCommand, Tray};

/// The application id (D-Bus name, desktop file, single instance).
pub const APP_ID: &str = patorrent_frontend::integration::APP_ID;

/// The engine-side state.
struct App {
    window: gtk::ApplicationWindow,
    session: Option<Session>,
    handle: SessionHandle,
    list: TorrentList,
    stats: SessionStats,
    prefs: Prefs,
    /// The default save path (Preferences > Directories, else the Downloads folder).
    save_path: String,
    /// Torrents seen in a status update since start.
    known: HashSet<TorrentId>,
    /// Torrents seen downloading: their completion is announced.
    downloading: HashSet<TorrentId>,
    alerts: Vec<Alert>,
    /// What the detail tabs show, and the Logger and Speed history.
    detail: Detail,
    /// The torrent whose file list was asked for (it is asked once per selection).
    files_requested: Option<TorrentId>,
    /// Magnets waiting for their metadata before the Add New Torrent dialog opens.
    awaiting: add::AwaitingMetadata,
    /// A magnet's Add New Torrent dialog is open (they open one at a time).
    awaiting_dialog: bool,
    /// Info-hashes to move to the top of the queue once added ("Add to top of queue").
    queue_top: HashSet<String>,
    /// Preferences > Directories "Move completed downloads to".
    move_completed: Option<MoveCompleted>,
    /// Preferences > Queueing's seeding goal.
    seeding_goal: SeedingGoal,
    /// Preferences > Directories "Location of .torrents", the copies being stored and the
    /// auto-load folder's watch.
    torrent_folders: TorrentFolders,
    torrent_files: TorrentFiles,
    autoload: Poller,
    /// Preferences > Scheduler, and the mode put into effect.
    schedule: Schedule,
    scheduler: Scheduler,
    /// Options > Auto Shutdown, and the standby or hibernation waiting for the resume data.
    auto_shutdown: AutoShutdown,
    suspend_pending: Option<Power>,
    /// The RSS Downloader (`rss.dat` beside `patorrent.ini`), its fetches, and whether it
    /// changed since it was saved.
    rss: RssDownloader,
    rss_worker: Worker,
    rss_dirty: bool,
    /// The feed whose items the list shows (`Some(None)`: every feed's), and their rows.
    feed_view: Option<Option<FeedId>>,
    feed_rows: Vec<ItemRow>,
    /// The feed a Feeds row's menu is for (`None`: the Feeds row).
    menu_feed: Option<FeedId>,
}

/// The widgets that draw from [`App`].
struct Ui {
    list: List,
    sidebar: Rc<Sidebar>,
    status: [gtk::Label; 4],
    /// The network indicator before the first status text.
    net_icon: gtk::Image,
    paned: gtk::Paned,
    details: Rc<Details>,
    /// The list over the detail tabs.
    split: gtk::Paned,
    /// Holds the torrent list, or a feed's items in its place.
    list_scroll: gtk::ScrolledWindow,
    feed_table: Table,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
    /// Started with `--minimized` (the autostart entry's "Start minimized"), until the tray
    /// came or did not.
    static START_MINIMIZED: Cell<bool> = const { Cell::new(false) };
    /// Quitting: the window closes instead of going into the tray.
    static QUITTING: Cell<bool> = const { Cell::new(false) };
    static TRAY: RefCell<Option<Rc<Tray>>> = const { RefCell::new(None) };
    /// Torrents from the command line that came before the window (a remote window's, while
    /// it connects): opened once it is built.
    static PENDING: RefCell<Vec<Source>> = const { RefCell::new(Vec::new()) };
}

/// A torrent named on the command line.
enum Source {
    /// A `magnet:` or `http(s)` link.
    Link(String),
    File(gio::File),
}

/// Opens a command-line torrent, or keeps it until the window is there.
fn open(source: Source) {
    // (borrowed: it is there)
    let built = APP.with(|a| a.try_borrow().map(|a| a.is_some()).unwrap_or(true));
    if !built {
        PENDING.with(|p| p.borrow_mut().push(source));
        return;
    }
    match source {
        Source::Link(link) => open_link(link),
        Source::File(f) => open_source(&f),
    }
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

fn ui() -> Option<Rc<Ui>> {
    UI.with(|u| u.borrow().clone())
}

/// The detail tabs' view of the state ([`crate::detail::Peek`]): read-only, so it works while
/// the widgets draw.
fn peek(f: &mut Visit<'_>) {
    APP.with(|a| {
        let Ok(a) = a.try_borrow() else { return };
        let Some(a) = a.as_ref() else { return };
        let st = a.detail.id.and_then(|id| a.list.get(id).map(|st| (st, a.list.extra(id).user_paused)));
        f(&a.detail, st);
    });
}

/// `[16:47:12]`, the Logger's time stamp.
fn now_hms() -> String {
    glib::DateTime::now_local()
        .and_then(|t| t.format("[%H:%M:%S]"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

pub fn run() -> ExitCode {
    // before the menu bar is built (`startup`); no Logger yet, so problems go to stderr
    let prefs = Prefs::load(paths::config_dir().join("patorrent.ini"));
    if let Ok(now) = glib::DateTime::now_local() {
        fmt::set_utc_offset(now.utc_offset().as_seconds() as i32);
    }
    for p in lang::init(prefs.get("ui.language").unwrap_or_default()) {
        eprintln!("{p}");
    }
    // the raw command line, so links reach us as typed (GIO's file handling would turn a
    // `magnet:` argument into a file); a second start hands it to the first. A remote window
    // is a process of its own, never that first one (`crate::remote`)
    let remote = crate::remote::requested();
    let mut flags = gio::ApplicationFlags::HANDLES_COMMAND_LINE;
    if remote {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    // Finder's double click on a `.torrent` reaches a running or starting app as an `odoc`
    // Apple event, which GTK's macOS delegate (`application:openFiles:`) turns into
    // `g_application_open`; that needs HANDLES_OPEN, and refuses the files without it. The
    // command line still goes to `command-line` (GApplication gives HANDLES_COMMAND_LINE the
    // arguments when both are set). Links come as `GURL` events: `crate::macos`
    if cfg!(target_os = "macos") {
        flags |= gio::ApplicationFlags::HANDLES_OPEN;
    }
    let app = gtk::Application::builder().application_id(APP_ID).flags(flags).build();
    app.connect_startup(startup);
    app.connect_activate(move |app| {
        if let Some(w) = app.active_window() {
            w.present();
        } else if remote {
            let a = app.clone();
            crate::remote::connect(app, move |session| build(&a, Some(session)));
        } else {
            build(app, None);
        }
    });
    app.connect_command_line(|app, cmdline| {
        // the autostart entry's "Start minimized"; a second start's switch is not for us
        if app.active_window().is_none() && cmdline.arguments().iter().any(|a| a.to_str() == Some(MINIMIZED))
        {
            START_MINIMIZED.with(|m| m.set(true));
        }
        app.activate();
        for arg in cmdline.arguments().iter().skip(1) {
            let arg = arg.to_string_lossy();
            if arg.starts_with("--") {
                continue;
            }
            if arg.starts_with("magnet:") || arg.starts_with("http://") || arg.starts_with("https://") {
                open(Source::Link(arg.into_owned()));
            } else {
                // relative to the directory of the process that was started
                open(Source::File(cmdline.create_file_for_arg(&*arg)));
            }
        }
        glib::ExitCode::SUCCESS
    });
    // only emitted with HANDLES_OPEN (macOS, above): files by path, any link by its URI
    app.connect_open(|app, files, _hint| {
        app.activate();
        for f in files {
            match f.uri_scheme().as_deref() {
                Some("magnet" | "http" | "https") => open(Source::Link(f.uri().into())),
                _ => open(Source::File(f.clone())),
            }
        }
    });
    let code = app.run();
    // Auto Shutdown's Shut Down, once the session has saved its state and stopped
    if let Some(Err(e)) = shutdown::take_after_exit().map(shutdown::perform) {
        eprintln!("{e}");
    }
    ExitCode::from(u8::from(code))
}

/// A URL from a `GURL` Apple event (a `magnet:` link clicked in a browser), through the `open`
/// signal like Finder's files.
#[cfg(target_os = "macos")]
fn open_url(url: String) {
    if let Some(app) = gio::Application::default() {
        app.open(&[gio::File::for_uri(&url)], "");
    }
}

/// The few style rules the window needs: the Done column's taller bar with its text on it.
const CSS: &str = "
progressbar.pa-done, progressbar.pa-done > trough { min-width: 16px; }
progressbar.pa-done > trough, progressbar.pa-done > trough > progress { min-height: 16px; }
.pa-done-text { font-size: smaller; }
";

/// The menu bar and the application-wide actions and shortcuts.
fn startup(app: &gtk::Application) {
    // GTK takes its direction from gettext's locale, which we do not use: an Arabic, Hebrew
    // or Persian table mirrors every window
    if lang::right_to_left() {
        gtk::Widget::set_default_direction(gtk::TextDirection::Rtl);
    }
    let css = gtk::CssProvider::new();
    css.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        crate::icons::setup(&display);
    }
    // after GTK's own startup (the signal runs its class handler first), which set its
    // NSApplication delegate and called `finishLaunching`
    #[cfg(target_os = "macos")]
    if let Err(e) = crate::macos::install_url_handler(open_url) {
        eprintln!("magnet: links from other programs will not arrive: {e}");
    }
    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(|_, _| quit_app());
    app.add_action(&quit);
    let remote = gio::SimpleAction::new("connect-remote", None);
    let a = app.downgrade();
    remote.connect_activate(move |_, _| {
        if let Some(w) = a.upgrade().and_then(|a| a.active_window()) {
            crate::remote::start_remote_window(&w);
        }
    });
    app.add_action(&remote);
    for (action, keys) in [
        ("win.add", &["<Control>o"][..]),
        ("win.add-url", &["<Control>u"]),
        ("win.create", &["<Control>n"]),
        ("win.remove", &["Delete"]),
        ("win.remove-data", &["<Shift>Delete"]),
        ("win.select-all", &["<Control>a"]),
        ("win.preferences", &["<Control>p"]),
        ("app.quit", &["<Control>q"]),
    ] {
        app.set_accels_for_action(action, keys);
    }

    let menubar = gio::Menu::new();
    let file = gio::Menu::new();
    item(&file, tr!(MenuAddTorrent), "win.add");
    item(&file, tr!(MenuAddUrl), "win.add-url");
    item(&file, tr!(MenuCreate), "win.create");
    let remote_section = gio::Menu::new();
    item(&remote_section, tr!(MenuConnectRemote), "app.connect-remote");
    file.append_section(None, &remote_section);
    let quit_section = gio::Menu::new();
    item(&quit_section, tr!(MenuExit), "app.quit");
    file.append_section(None, &quit_section);
    menubar.append_submenu(Some(&gtk_mnemonic(tr!(MenuFile))), &file);
    let options = gio::Menu::new();
    item(&options, tr!(MenuPreferences), "win.preferences");
    item(&options, tr!(MenuRss), "win.rss");
    // Disabled | the client | the computer
    let auto = gio::Menu::new();
    let mut section = gio::Menu::new();
    for (i, &m) in AutoMode::MENU.iter().enumerate() {
        item(&section, AutoMode::label(m), &format!("win.auto-shutdown-{i}"));
        if i == 0 || i == 2 || i == AutoMode::MENU.len() - 1 {
            auto.append_section(None, &section);
            section = gio::Menu::new();
        }
    }
    let auto_section = gio::Menu::new();
    auto_section.append_submenu(Some(&gtk_mnemonic(tr!(MenuAutoShutdown))), &auto);
    options.append_section(None, &auto_section);
    menubar.append_submenu(Some(&gtk_mnemonic(tr!(MenuOptions))), &options);
    let help = gio::Menu::new();
    item(&help, tr!(MenuAbout), "win.about");
    menubar.append_submenu(Some(&gtk_mnemonic(tr!(MenuHelp))), &help);
    app.set_menubar(Some(&menubar));
}

/// A menu item from a string table text (its `&` mnemonic as GTK's `_`).
fn item(menu: &gio::Menu, text: &str, action: &str) {
    menu.append(Some(&gtk_mnemonic(text)), Some(action));
}

/// What the torrent context menu shows besides its actions' states.
struct MenuState {
    labels: Vec<String>,
    /// One finished torrent: "Open Containing Folder" names its own folder.
    single_done: bool,
}

/// The torrent context menu (right click on the list), as the Win32 client's.
fn torrent_menu(m: &MenuState) -> gio::Menu {
    let menu = gio::Menu::new();
    let open = gio::Menu::new();
    item(&open, if m.single_done { tr!(MenuOpenFolder) } else { tr!(MenuOpenFolderMany) }, "win.open-folder");
    menu.append_section(None, &open);
    let run = gio::Menu::new();
    item(&run, tr!(MenuForceStart), "win.force-start");
    item(&run, tr!(MenuStart), "win.start");
    item(&run, tr!(MenuPause), "win.pause");
    item(&run, tr!(MenuStop), "win.stop");
    menu.append_section(None, &run);
    let queue = gio::Menu::new();
    item(&queue, tr!(MenuQueueUp), "win.queue-up");
    item(&queue, tr!(MenuQueueDown), "win.queue-down");
    item(&queue, tr!(MenuQueueTop), "win.queue-top");
    item(&queue, tr!(MenuQueueBottom), "win.queue-bottom");
    menu.append_section(None, &queue);
    let remove = gio::Menu::new();
    item(&remove, tr!(MenuRemove), "win.remove");
    let remove_and = gio::Menu::new();
    item(&remove_and, tr!(MenuDeleteData), "win.remove-data");
    remove.append_submenu(Some(&gtk_mnemonic(tr!(MenuRemoveAnd))), &remove_and);
    menu.append_section(None, &remove);
    let check = gio::Menu::new();
    item(&check, tr!(MenuRecheck), "win.recheck");
    item(&check, tr!(MenuUpdateTracker), "win.reannounce");
    menu.append_section(None, &check);
    let more = gio::Menu::new();
    let advanced = gio::Menu::new();
    item(&advanced, tr!(MenuSetLocation), "win.set-location");
    item(&advanced, tr!(MenuInitialSeeding), "win.super-seed");
    more.append_submenu(Some(&gtk_mnemonic(tr!(MenuAdvanced))), &advanced);
    let bandwidth = gio::Menu::new();
    for (title, action) in [(tr!(MenuSetDownLimit), "win.dl-limit"), (tr!(MenuSetUpLimit), "win.ul-limit")] {
        let rates = gio::Menu::new();
        for kib in actions::LIMITS {
            let i = gio::MenuItem::new(Some(&actions::limit_text(kib)), None);
            i.set_action_and_target_value(Some(action), Some(&kib.to_variant()));
            rates.append_item(&i);
        }
        bandwidth.append_submenu(Some(&gtk_mnemonic(title)), &rates);
    }
    more.append_submenu(Some(&gtk_mnemonic(tr!(MenuBandwidth))), &bandwidth);
    let label = gio::Menu::new();
    let label_actions = gio::Menu::new();
    item(&label_actions, tr!(MenuNewLabel), "win.label-new");
    item(&label_actions, tr!(MenuRemoveLabel), "win.label-none");
    label.append_section(None, &label_actions);
    let names = gio::Menu::new();
    for l in &m.labels {
        // a label is the user's text: an underscore in it is not a mnemonic
        let i = gio::MenuItem::new(Some(&l.replace('_', "__")), None);
        i.set_action_and_target_value(Some("win.label"), Some(&l.to_variant()));
        names.append_item(&i);
    }
    label.append_section(None, &names);
    more.append_submenu(Some(&gtk_mnemonic(tr!(MenuLabel))), &label);
    menu.append_section(None, &more);
    let props = gio::Menu::new();
    item(&props, tr!(MenuProperties), "win.properties");
    menu.append_section(None, &props);
    menu
}

/// The context menu for the selection: its texts, with the actions' check marks and greying set
/// to match (the label in common, the first torrent's limits, Initial Seeding for seeds only).
fn menu_state() -> Option<MenuState> {
    let ids = selected();
    let first = *ids.first()?;
    with_app(|a| {
        let label = a.list.extra(first).label;
        let label = ids.iter().all(|&id| a.list.extra(id).label == label).then(|| label.to_owned());
        let st = a.list.get(first);
        let kib = |b: u32| if b % 1024 == 0 { b / 1024 } else { u32::MAX };
        let (seeds, super_seeding) = super_seed_state(a, &ids);
        let moving = ids.iter().any(|&id| a.list.get(id).is_some_and(|s| s.moving_storage));
        let set = |name: &str, state: Option<glib::Variant>, enabled: bool| {
            if let Some(action) = a.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                if let Some(state) = state {
                    action.set_state(&state);
                }
                action.set_enabled(enabled);
            }
        };
        // while the torrents differ, a label no item has
        set("label", Some(label.clone().unwrap_or_else(|| "\u{1}".into()).to_variant()), true);
        set("label-none", None, label.as_deref() != Some(""));
        set("dl-limit", Some(st.map_or(0, |s| kib(s.download_limit)).to_variant()), true);
        set("ul-limit", Some(st.map_or(0, |s| kib(s.upload_limit)).to_variant()), true);
        set("set-location", None, !moving);
        set("super-seed", Some(super_seeding.to_variant()), seeds);
        // the server's folders are not on this computer
        set("open-folder", None, !crate::remote::is_remote());
        MenuState { labels: labels(a), single_done: ids.len() == 1 && st.is_some_and(|s| s.is_finished) }
    })
}

/// (every torrent is a seed, every torrent is super seeding), for "Initial Seeding".
fn super_seed_state(a: &App, ids: &[TorrentId]) -> (bool, bool) {
    let all = |f: &dyn Fn(&patorrent_core::TorrentStatus) -> bool| {
        ids.iter().all(|&id| a.list.get(id).is_some_and(f))
    };
    (all(&|s| s.is_seeding), all(&|s| s.super_seeding))
}

/// The main window, on a new engine session, or on `remote_session` in remote mode
/// (`crate::remote`).
fn build(app: &gtk::Application, remote_session: Option<Session>) {
    let config = paths::settings_dir();
    let data = paths::data_dir();
    let mut prefs = Prefs::load(config.join("patorrent.ini"));
    let remote = crate::remote::get();
    if let Some(r) = remote {
        // the server's engine settings and default folder are this window's; the language
        // (picked in `run`) is every window's
        if let Err(e) = std::fs::create_dir_all(&config) {
            eprintln!("cannot create {}: {e}", config.display());
        }
        settings::store(&mut prefs, &r.hello.settings);
        prefs.set("dir.download", &r.hello.save_path);
        match Prefs::load(paths::config_dir().join("patorrent.ini")).get("ui.language") {
            Some(code) => prefs.set("ui.language", code),
            None => prefs.remove("ui.language"),
        }
    }
    // (the language was picked in `run`)
    let downloads = glib::user_special_dir(glib::UserDirectory::Downloads)
        .unwrap_or_else(|| glib::home_dir().join("Downloads"))
        .to_string_lossy()
        .into_owned();
    let mut opts = Options::load(&prefs, &downloads);
    // the server keeps its own port
    if opts.gui.random_port && remote.is_none() {
        // µTorrent's "Randomize port each start" keeps the new port as the configured one
        settings::set_listen_port(&mut opts.settings, options::random_port());
        settings::store(&mut prefs, &opts.settings);
    }
    let save_path = opts.gui.download_dir.clone();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("PaTorrent")
        .default_width(prefs.get_i32("gtk.width").unwrap_or(1000).max(400))
        .default_height(prefs.get_i32("gtk.height").unwrap_or(620).max(300))
        .show_menubar(true)
        .build();
    if prefs.get_bool("gtk.maximized", false) {
        window.maximize();
    }
    if let Some(r) = remote {
        window.set_title(Some(&tr!(TitleRemote, label = r.label)));
    }

    let started = match remote_session {
        Some(s) => Ok(s),
        None => Session::start(SessionParams { settings: opts.settings.clone(), state_dir: Some(data) }),
    };
    let session = match started {
        Ok(s) => s,
        Err(e) => {
            window.present();
            let d = gtk::AlertDialog::builder()
                .message(tr!(CannotStartEngine))
                .detail(e.to_string())
                .modal(true)
                .build();
            let w = window.clone();
            d.choose(Some(&window), None::<&gio::Cancellable>, move |_| w.close());
            return;
        }
    };
    let handle = session.handle();

    // the widgets
    let render: crate::list::Render = Rc::new(|id, column| {
        with_app(|a| {
            let st = a.list.get(id)?;
            let text = column.text(st, a.list.extra(id));
            Some(match column {
                Column::Done => Content::Progress(f64::from(st.progress_ppm) / 1e6, text),
                Column::Name => Content::Named(status_icon(st, a.list.extra(id).user_paused), text),
                _ => Content::Text(text),
            })
        })
        .flatten()
    });
    let popup: crate::list::ContextMenu = Rc::new(move |widget, x, y| {
        let Some(ui) = ui() else { return };
        // built for each click: the labels and the check marks follow the selection
        if let Some(menu) = menu_state().map(|m| torrent_menu(&m)) {
            popup_menu(&menu, ui.list.view.upcast_ref(), widget, x, y);
        }
    });
    let list = List::new(&prefs.columns(), prefs.sort(), render, popup);
    let files_menu = crate::detail::files_menu();
    let details = Details::new(
        peek,
        prefs.get("ui.tab").and_then(Tab::from_key).unwrap_or(Tab::General),
        |_| {
            request_detail();
            refresh_details();
        },
        Rc::new(move |widget, x, y| {
            // the menu hangs off the notebook, which outlives the table's cells
            if let Some(ui) = ui() {
                popup_menu(&files_menu, ui.details.notebook.upcast_ref(), widget, x, y);
            }
        }),
    );
    let sidebar = Sidebar::new(
        |pick| match pick {
            Pick::Torrents(filter) => {
                with_app(|a| a.list.set_filter(filter));
                show_feed_items(None);
                refresh();
            }
            Pick::Feed(feed) => show_feed_items(Some(feed)),
        },
        Rc::new(|feed, widget, x, y| {
            with_app(|a| a.menu_feed = feed);
            if let Some(ui) = ui() {
                popup_menu(&feed_menu(feed), ui.sidebar.widget.upcast_ref(), widget, x, y);
            }
        }),
    );
    // a feed's items, in the torrent list's place while a feed is picked
    let feed_text: crate::table::CellText = Rc::new(|row, column| {
        APP.with(|a| {
            let a = a.try_borrow().ok()?;
            a.as_ref()?.feed_rows.get(row)?.cells.get(column).cloned()
        })
        .unwrap_or_default()
    });
    let item_menu: crate::table::ContextMenu = Rc::new(|widget, x, y| {
        let menu = gio::Menu::new();
        item(&menu, tr!(MenuDownload), "win.feed-item-download");
        let more = gio::Menu::new();
        item(&more, tr!(MenuRss), "win.rss");
        menu.append_section(None, &more);
        if let Some(ui) = ui() {
            popup_menu(&menu, ui.feed_table.view.upcast_ref(), widget, x, y);
        }
    });
    let feed_table = Table::new(&rss_view::ITEM_COLUMNS, feed_text, None, Some(item_menu));
    feed_table.view.connect_activate(|_, _| download_feed_items());
    let status: [gtk::Label; 4] = std::array::from_fn(|_| {
        let l = gtk::Label::new(None);
        l.set_xalign(0.0);
        l.set_margin_start(6);
        l.set_margin_end(6);
        l
    });

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    toolbar.add_css_class("toolbar");
    for item in [
        Some(("document-open", "win.add", tr!(ToolAddTorrent))),
        Some(("insert-link", "win.add-url", tr!(ToolAddUrl))),
        Some(("document-new", "win.create", tr!(ToolCreate))),
        None,
        Some(("edit-delete", "win.remove", tr!(ToolRemove))),
        None,
        Some(("media-playback-start", "win.start", tr!(ToolStart))),
        Some(("media-playback-pause", "win.pause", tr!(ToolPause))),
        Some(("media-playback-stop", "win.stop", tr!(ToolStop))),
        None,
        Some(("go-up", "win.queue-up", tr!(ToolQueueUp))),
        Some(("go-down", "win.queue-down", tr!(ToolQueueDown))),
        None,
        Some(("application-rss+xml", "win.rss", tr!(ToolRss))),
    ] {
        match item {
            Some((icon, action, tip)) => {
                let b = gtk::Button::from_icon_name(icon);
                b.set_action_name(Some(action));
                b.set_tooltip_text(Some(tip));
                b.add_css_class("flat");
                toolbar.append(&b);
            }
            None => toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical)),
        }
    }

    let side_scroll = gtk::ScrolledWindow::builder()
        .child(&sidebar.widget)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let list_scroll = gtk::ScrolledWindow::builder().child(&list.view).hexpand(true).vexpand(true).build();
    // The list keeps a minimum (its column header) and is not shrunk below it. A shrinkable
    // start child counts 0 in GtkPaned's minimum but at least 1 px in `gtk_paned_compute_position`
    // (`max = MAX (1, allocation - end_child_req)`), so a split given exactly its minimum
    // measured the tabs 1 px short: "Trying to measure GtkNotebook ... for height of 109, but it
    // needs at least 110" on macOS (GTK 4.20). With a minimum of its own the two agree.
    list_scroll.set_size_request(-1, 24);
    let split = gtk::Paned::new(gtk::Orientation::Vertical);
    split.set_start_child(Some(&list_scroll));
    split.set_shrink_start_child(false);
    split.set_end_child(Some(&details.notebook));
    split.set_resize_end_child(false);
    split.set_shrink_end_child(false);
    split.set_position(prefs.get_i32("gtk.split").unwrap_or(330));
    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&side_scroll));
    paned.set_end_child(Some(&split));
    paned.set_shrink_start_child(false);
    paned.set_resize_start_child(false);
    paned.set_position(prefs.get_i32("gtk.sidebar").unwrap_or(180));
    let statusbar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let net_icon = crate::icons::state_image();
    net_icon.set_margin_start(6);
    statusbar.append(&net_icon);
    for (i, l) in status.iter().enumerate() {
        if i > 0 {
            statusbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        }
        l.set_hexpand(i == 0);
        statusbar.append(l);
    }
    statusbar.set_margin_top(2);
    statusbar.set_margin_bottom(2);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&toolbar);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&paned);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&statusbar);
    window.set_child(Some(&root));

    // sorting by the header: the frontend sorts, the header shows it
    if let Some(sorter) = list.view.sorter() {
        sorter.connect_changed(|_, _| {
            let Some(ui) = ui() else { return };
            let sort = ui.list.sort();
            with_app(|a| a.list.set_sort(sort));
            refresh();
        });
    }
    // a double click opens a finished torrent's folder and the others' Properties, as in µTorrent
    list.view.connect_activate(|_, position| {
        let Some(id) = ui().and_then(|ui| ui.list.model.id_at(position)) else { return };
        if with_app(|a| a.list.get(id).is_some_and(|st| st.is_finished)) == Some(true) {
            open_folder(&[id]);
        } else {
            show_properties();
        }
    });
    list.selection.connect_selection_changed(|_, _, _| selection_changed());

    add_actions(&window, &list);
    // magnets whose dialog was still to come at the last exit
    let awaiting = add::AwaitingMetadata::load(&prefs);
    let ui = Rc::new(Ui { list, sidebar, status, net_icon, paned, details, split, list_scroll, feed_table });
    UI.with(|u| *u.borrow_mut() = Some(ui));
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            window: window.clone(),
            session: Some(session),
            handle: handle.clone(),
            list: TorrentList::new(),
            stats: SessionStats::default(),
            prefs,
            save_path,
            known: HashSet::new(),
            downloading: HashSet::new(),
            alerts: Vec::new(),
            detail: Detail::default(),
            files_requested: None,
            awaiting,
            awaiting_dialog: false,
            queue_top: HashSet::new(),
            move_completed: opts.gui.move_completed(),
            seeding_goal: opts.gui.seeding_goal(),
            torrent_folders: opts.gui.torrent_folders.clone(),
            torrent_files: TorrentFiles::new(),
            autoload: Poller::new(),
            schedule: opts.gui.schedule.clone(),
            scheduler: Scheduler::new(),
            auto_shutdown: AutoShutdown::default(),
            suspend_pending: None,
            rss: RssDownloader::load(&rss_file()),
            // a fetch finished on its thread: its result is taken on the main loop
            rss_worker: Worker::new(|| glib::MainContext::default().invoke(rss_collect)),
            rss_dirty: false,
            feed_view: None,
            feed_rows: Vec::new(),
            menu_feed: None,
        })
    });
    with_app(|a| {
        let sort = a.prefs.sort();
        a.list.set_sort(sort);
        // a server applies its own block list
        if opts.gui.ip_filter && remote.is_none() {
            load_ip_filter(a, &opts.gui);
        }
    });

    // the engine's alerts: drained on the main loop
    handle.set_alert_notify(|| {
        glib::MainContext::default().invoke(on_alerts);
    });
    // the notification fires when the queue goes from empty to not: drain what the engine
    // queued while it started, or it never would
    on_alerts();
    let h = handle.clone();
    glib::timeout_add_seconds_local(1, move || {
        h.post_torrent_updates();
        h.post_session_stats();
        request_detail();
        let step = with_app(|a| {
            run_scheduler(a, None);
            autoload(a);
            rss_timer(a);
            let list = &a.list;
            a.auto_shutdown.check(list.ids().filter_map(|id| list.get(id)), std::time::Instant::now())
        });
        if let Some(step) = step {
            auto_shutdown_step(step);
        }
        glib::ControlFlow::Continue
    });
    handle.post_torrent_updates();
    handle.post_session_stats();

    window.connect_close_request(|w| {
        // µTorrent's "Close to tray"; Exit and the other ways to quit go through `quit_app`
        let to_tray = !QUITTING.with(Cell::get)
            && tray_shown()
            && with_app(|a| a.prefs.get_bool("ui.close_to_tray", true)) == Some(true);
        if to_tray {
            hide_window(w.upcast_ref());
            return glib::Propagation::Stop;
        }
        shutdown();
        glib::Propagation::Proceed
    });
    update_status();
    window.present();
    start_tray(&window);
    if START_MINIMIZED.with(|m| m.get()) {
        // into the tray once it is there (`tray_shown`); minimized until then, or for good
        // without one
        window.minimize();
    } else if opts.gui.check_association
        && remote.is_none()
        && crate::integration::AVAILABLE
        && crate::integration::associations_settable()
    {
        glib::idle_add_local_once(check_association);
    }
    for source in PENDING.with(|p| p.take()) {
        open(source);
    }
}

// ---- the tray ---------------------------------------------------------------------------------

fn tray() -> Option<Rc<Tray>> {
    TRAY.with(|t| t.borrow().clone())
}

/// A tray is on screen (a StatusNotifierWatcher took the item).
fn tray_shown() -> bool {
    tray().is_some_and(|t| t.shown())
}

/// The tray icon on the session bus, and "Minimize to tray" on the window's state.
fn start_tray(window: &gtk::ApplicationWindow) {
    let tray = Tray::start(crate::tray::Hooks {
        activate: Box::new(toggle_window),
        command: Box::new(tray_command),
        window_visible: Box::new(|| with_app(|a| a.window.is_visible()).unwrap_or(true)),
        shown: Box::new(|on| {
            // started minimized: into the tray as soon as there is one
            let start = START_MINIMIZED.with(|m| m.replace(false));
            if on && start {
                if let Some(w) = with_app(|a| a.window.clone()).filter(|w| !w.is_active()) {
                    hide_window(w.upcast_ref());
                }
            }
        }),
    });
    TRAY.with(|t| *t.borrow_mut() = tray);
    if let Some(toplevel) = window.surface().and_downcast::<gtk::gdk::Toplevel>() {
        let w = window.downgrade();
        toplevel.connect_state_notify(move |t| {
            let minimized = t.state().contains(gtk::gdk::ToplevelState::MINIMIZED);
            let to_tray = with_app(|a| a.prefs.get_bool("ui.minimize_to_tray", false)) == Some(true);
            if let (true, true, true, Some(w)) = (minimized, to_tray, tray_shown(), w.upgrade()) {
                hide_window(w.upcast_ref());
            }
        });
    }
}

/// Hides the window into the tray.
fn hide_window(w: &gtk::Window) {
    w.set_visible(false);
    if let Some(t) = tray() {
        t.menu_changed();
    }
}

/// A click on the tray icon: the window comes to the front, or goes into the tray when it is
/// in front already.
fn toggle_window() {
    let Some(w) = with_app(|a| a.window.clone()) else { return };
    if w.is_visible() && w.is_active() {
        hide_window(w.upcast_ref());
    } else {
        show_window();
    }
}

fn show_window() {
    let Some(w) = with_app(|a| a.window.clone()) else { return };
    w.set_visible(true);
    w.unminimize();
    w.present();
    if let Some(t) = tray() {
        t.menu_changed();
    }
}

fn tray_command(c: TrayCommand) {
    match c {
        TrayCommand::Toggle => toggle_window(),
        TrayCommand::AddTorrent => {
            show_window();
            add_files();
        }
        TrayCommand::AddUrl => {
            show_window();
            add_url();
        }
        TrayCommand::StartAll | TrayCommand::StopAll => {
            with_app(|a| {
                let ids: Vec<TorrentId> = a.list.ids().collect();
                for id in ids {
                    let t = a.handle.torrent(id);
                    if c == TrayCommand::StartAll {
                        actions::start(&t);
                    } else {
                        actions::stop(&t);
                    }
                    set_paused(a, id, false);
                }
                a.handle.post_torrent_updates();
            });
        }
        TrayCommand::Exit => quit_app(),
    }
}

/// File > Exit, the tray's Exit, Ctrl+Q, Auto Shutdown: every window closes, the main one
/// saves and stops the session on the way.
fn quit_app() {
    QUITTING.with(|q| q.set(true));
    let Some(app) = with_app(|a| a.window.application()).flatten() else { return };
    for w in app.windows() {
        w.close();
    }
}

/// Desktop notifications for downloads that completed (Preferences > UI Settings "Show
/// balloon notifications", µTorrent's tray balloons).
fn notify_completed(names: Vec<String>) {
    let Some((app, on)) = with_app(|a| (a.window.application(), a.prefs.get_bool("ui.balloons", true)))
    else {
        return;
    };
    let Some(app) = app.filter(|_| on) else { return };
    for name in names {
        if crate::remote::is_remote() {
            notify_directly(tr!(BalloonComplete), &name);
            continue;
        }
        let n = gio::Notification::new(tr!(BalloonComplete));
        n.set_body(Some(&name));
        app.send_notification(Some(&format!("complete-{name}")), &n);
    }
}

/// A desktop notification from `org.freedesktop.Notifications` itself: a remote window
/// (`NON_UNIQUE`) has no bus name of its own, which `GNotification` needs.
fn notify_directly(summary: &str, body: &str) {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>) else { return };
    let hints = std::collections::HashMap::<String, glib::Variant>::new();
    let args = ("PaTorrent", 0u32, "", summary, body, Vec::<String>::new(), hints, -1i32).to_variant();
    bus.call(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        None::<&gio::Cancellable>,
        |r| {
            if let Err(e) = r {
                eprintln!("cannot show a notification: {e}");
            }
        },
    );
}

/// Preferences > General "Check association on startup": asks to make PaTorrent the program
/// for `.torrent` files and `magnet:` links when it is not.
fn check_association() {
    let Some(window) = with_app(|a| a.window.clone()) else { return };
    let set = crate::integration::read();
    if set.torrent && set.magnet {
        return;
    }
    let d = gtk::AlertDialog::builder()
        .message(tr!(AskAssociate))
        .buttons([tr!(BtnCancel), tr!(BtnOk)])
        .cancel_button(0)
        .default_button(1)
        .modal(true)
        .build();
    d.choose(Some(&window), None::<&gio::Cancellable>, move |r| {
        if r != Ok(1) {
            return;
        }
        let want = Integration { torrent: true, magnet: true, ..set };
        if let Err(e) = crate::integration::write(&set, &want) {
            error(&tr!(ErrAssociate, error = e), "");
        }
    });
}

/// Preferences > General's Desktop Integration, when the dialog's boxes differ from what is set.
fn apply_integration(o: &Options) {
    let set = crate::integration::read();
    if o.integration.normalized() == set {
        return;
    }
    if let Err(e) = crate::integration::write(&set, &o.integration) {
        error(&tr!(ErrDesktopIntegration, error = e), "");
    }
}

/// Saves the layout, then stops the engine (it saves its state).
fn shutdown() {
    let Some(ui) = ui() else { return };
    let (layout, sort, sidebar) = (ui.list.layout(), ui.list.sort(), ui.paned.position());
    let (tab, split) = (ui.details.tab(), ui.split.position());
    let app = APP.with(|a| a.borrow_mut().take());
    let Some(mut app) = app else { return };
    app.prefs.set_columns(&layout);
    app.prefs.set_sort(sort);
    app.prefs.set("gtk.sidebar", sidebar);
    app.prefs.set("ui.tab", tab.key());
    app.prefs.set("gtk.split", split);
    let (w, h) = app.window.default_size();
    app.prefs.set("gtk.width", w);
    app.prefs.set("gtk.height", h);
    app.prefs.set_bool("gtk.maximized", app.window.is_maximized());
    if let Err(e) = app.prefs.save() {
        eprintln!("cannot save the settings: {e}");
    }
    if let Err(e) = app.rss.save(&rss_file()) {
        eprintln!("cannot save {}: {e}", rss_file().display());
    }
    app.handle.clear_alert_notify();
    if let Some(s) = app.session.take() {
        s.shutdown();
    }
    UI.with(|u| u.borrow_mut().take());
}

fn on_alerts() {
    let mut errors = Vec::new();
    let mut magnet_ready = false;
    let mut suspend = None;
    let mut completed = Vec::new();
    let done = with_app(|a| {
        let mut alerts = std::mem::take(&mut a.alerts);
        a.handle.pop_alerts(&mut alerts);
        for alert in alerts.drain(..) {
            if let Some((id, dir)) = a
                .move_completed
                .as_mut()
                .and_then(|m| m.on_alert(&alert, &a.handle).map(|id| (id, m.dir.clone())))
            {
                let name = a.list.get(id).map(|st| st.name.clone()).unwrap_or_default();
                a.detail.push_log(format!("{} {}", now_hms(), tr!(LogMovingCompleted, name, dir)));
                a.handle.post_torrent_updates();
            }
            for line in a.torrent_files.on_alert(&alert, &a.torrent_folders, &a.handle) {
                a.detail.push_log(format!("{} {line}", now_hms()));
            }
            magnet_ready |= a.awaiting.on_alert(&alert, &a.handle, &mut a.prefs);
            match alert {
                Alert::StateUpdate(v) => {
                    for st in &v {
                        if a.known.insert(st.id) {
                            // restored or added during Seeding only hours
                            if a.scheduler.mode() == Mode::SeedOnly
                                && !st.flags.contains(TorrentFlags::UPLOAD_MODE)
                            {
                                a.handle.torrent(st.id).set_flags(TorrentFlags::UPLOAD_MODE);
                            }
                            // first sight: what only the frontend knows
                            let key = st.info_hashes.key_hex();
                            if let Some(l) = a.prefs.get(&format!("torrent.{key}.label")) {
                                let l = l.to_owned();
                                a.list.set_label(st.id, &l);
                            }
                            if a.prefs.get_bool(&format!("torrent.{key}.paused"), false) {
                                a.list.set_user_paused(st.id, true);
                            }
                        }
                        if !st.is_finished {
                            a.downloading.insert(st.id);
                        }
                        seeding_goal(a, st);
                    }
                    a.list.update(v);
                }
                Alert::SessionStats(s) => {
                    a.stats = s;
                    a.detail.push_speed(s.download_rate, s.upload_rate);
                }
                Alert::TorrentAdded { id, info_hashes, ref name } => {
                    a.detail.push_log(format!("{} {}", now_hms(), tr!(LogAdded, name)));
                    if a.queue_top.remove(&info_hashes.key_hex()) {
                        a.handle.torrent(id).queue_position(QueueMove::Top);
                    }
                    a.handle.post_torrent_updates();
                }
                Alert::AddTorrentFailed { ref name, ref error } => {
                    a.detail.push_log(format!("{} {}", now_hms(), alert.message()));
                    errors.push((tr!(CannotAdd, name), error.to_string()));
                }
                Alert::TorrentRemoved { id, info_hashes } => {
                    a.list.remove(id);
                    a.known.remove(&id);
                    a.downloading.remove(&id);
                    let key = info_hashes.key_hex();
                    a.prefs.remove(&format!("torrent.{key}.label"));
                    a.prefs.remove(&format!("torrent.{key}.paused"));
                }
                Alert::RemoteDisconnected { ref message } => {
                    a.detail.push_log(format!("{} {}", now_hms(), alert.message()));
                    let label = crate::remote::get().map_or("", |r| r.label.as_str());
                    a.window.set_title(Some(&tr!(TitleDisconnected, label)));
                    errors.push((tr!(ErrRemoteLost, label, message), String::new()));
                }
                Alert::TorrentFinished { id } => {
                    let name = a.list.get(id).map(|st| st.name.clone()).unwrap_or_default();
                    a.detail.push_log(format!("{} {}", now_hms(), tr!(LogFinished, name)));
                    // a download that completed, not a restored or checked seed
                    if a.downloading.remove(&id) {
                        completed.push(name);
                    }
                }
                Alert::FileRenamed { id, .. } | Alert::FileRenameFailed { id, .. } => {
                    let name = a.list.get(id).map(|st| st.name.clone()).unwrap_or_default();
                    a.detail.push_log(format!("{} {name}: {}", now_hms(), alert.message()));
                    if a.detail.id == Some(id) {
                        a.handle.torrent(id).post_file_list();
                    }
                }
                Alert::PeerInfo { id, peers } if a.detail.id == Some(id) => a.detail.peers = peers,
                Alert::TrackerList { id, trackers } if a.detail.id == Some(id) => {
                    a.detail.trackers = trackers
                }
                Alert::FileList { id, files } if a.detail.id == Some(id) => a.detail.set_files(files),
                Alert::FileProgress { id, done } if a.detail.id == Some(id) => a.detail.file_done = done,
                Alert::PieceInfo { id, have, availability, downloading } if a.detail.id == Some(id) => {
                    a.detail.have = Some(have);
                    a.detail.availability = availability;
                    a.detail.partial = downloading;
                }
                // standby and hibernation wait for the state on disk
                Alert::ResumeDataSaved => suspend = a.suspend_pending.take(),
                Alert::PeerInfo { .. }
                | Alert::TrackerList { .. }
                | Alert::FileList { .. }
                | Alert::FileProgress { .. }
                | Alert::PieceInfo { .. }
                | Alert::TorrentFile { .. } => {}
                // one per stopped or queued torrent every half hour (`frontend::scrape`); the
                // counts show in the Seeds and Peers columns
                Alert::ScrapeReply { .. } | Alert::ScrapeFailed { .. } => {}
                // the mappers' step-by-step chatter stays out of the Logger, as in the Win32 one
                Alert::PortmapLog { .. } => {}
                other => {
                    let name = other.torrent().and_then(|id| a.list.get(id)).map(|st| st.name.clone());
                    a.detail.push_log(match name {
                        Some(n) => format!("{} {n}: {}", now_hms(), other.message()),
                        None => format!("{} {}", now_hms(), other.message()),
                    });
                }
            }
        }
        a.alerts = alerts;
    });
    if done.is_none() {
        return;
    }
    refresh();
    // a removed torrent may have been the one shown
    selection_changed();
    refresh_details();
    update_status();
    for (message, detail) in errors {
        error(&message, &detail);
    }
    if magnet_ready {
        magnet_dialogs();
    }
    notify_completed(completed);
    if let Some(Err(e)) = suspend.map(shutdown::perform) {
        log_line(&tr!(LogShutdown, what = e.replace('\n', " ")));
        error(&e, "");
    }
}

/// Rebuilds the list's rows if the view changed (keeping the selection), redraws the cells on
/// screen and the category counts.
fn refresh() {
    let Some(ui) = ui() else { return };
    let Some(ids) = with_app(|a| {
        a.list.refresh();
        a.list.view().iter().map(|id| id.to_raw()).collect::<Vec<_>>()
    }) else {
        return;
    };
    let selected = ui.list.selected();
    if ui.list.model.set_ids(ids) {
        ui.list.select(&selected);
    }
    ui.list.redraw();
    APP.with(|a| {
        if let Ok(a) = a.try_borrow() {
            if let Some(a) = a.as_ref() {
                let feeds: Vec<(FeedId, String)> =
                    a.rss.feeds.iter().map(|f| (f.id, f.name().to_owned())).collect();
                ui.sidebar.update(&a.list, &feeds);
            }
        }
    });
    // a label or feed shown that went away gives the list back to the torrents
    let gone = with_app(|a| match ui.sidebar.current() {
        Pick::Torrents(f) if *a.list.filter() != f => {
            a.list.set_filter(f);
            true
        }
        Pick::Torrents(_) => a.feed_view.is_some(),
        Pick::Feed(_) => false,
    });
    if gone == Some(true) {
        show_feed_items(None);
        refresh();
    }
}

/// The torrent the detail tabs show: the first selected one.
fn selection_changed() {
    let Some(ui) = ui() else { return };
    let id = ui.list.selected().first().copied();
    let changed = with_app(|a| {
        let changed = a.detail.id != id;
        a.detail.select(id);
        if changed {
            a.files_requested = None;
        }
        changed
    });
    if changed == Some(true) {
        ui.details.torrent_changed();
        request_detail();
        ui.details.refresh();
    }
}

/// Asks the engine for what the detail tab on screen needs.
fn request_detail() {
    let Some(tab) = ui().map(|ui| ui.details.tab()) else { return };
    with_app(|a| {
        let Some(id) = a.detail.id else { return };
        let t = a.handle.torrent(id);
        match tab {
            Tab::General | Tab::Pieces => t.post_piece_info(),
            Tab::Trackers => t.post_trackers(),
            Tab::Peers => t.post_peer_info(),
            Tab::Files => {
                if a.files_requested != Some(id) {
                    a.files_requested = Some(id);
                    t.post_file_list();
                }
                t.post_file_progress();
            }
            Tab::Speed | Tab::Logger => {}
        }
    });
}

fn refresh_details() {
    if let Some(ui) = ui() {
        ui.details.refresh();
    }
}

/// Opens `menu` at (x, y) of `widget`, hung off `parent`.
pub(crate) fn popup_menu(menu: &gio::Menu, parent: &gtk::Widget, widget: &gtk::Widget, x: f64, y: f64) {
    let at = widget
        .compute_point(parent, &gtk::graphene::Point::new(x as f32, y as f32))
        .unwrap_or_else(|| gtk::graphene::Point::new(x as f32, y as f32));
    let popover = gtk::PopoverMenu::from_model(Some(menu));
    popover.set_parent(parent);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(at.x() as i32, at.y() as i32, 1, 1)));
    popover.connect_closed(|p| {
        let p = p.clone();
        // unparent once the menu's action has run
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}

fn update_status() {
    let Some(ui) = ui() else { return };
    let Some((texts, has_incoming)) = with_app(|a| {
        let s = &a.stats;
        let net = if s.has_incoming_connections { tr!(NetOk) } else { tr!(NetNoIncoming) };
        let texts = [
            match a.scheduler.status_text() {
                Some(sched) => format!("{net} | {sched}"),
                None => net.to_owned(),
            },
            tr!(BarDht, count = s.dht_nodes),
            tr!(BarDown, rate = fmt::speed_always(s.download_rate), total = fmt::size(s.recv_payload_bytes)),
            tr!(BarUp, rate = fmt::speed_always(s.upload_rate), total = fmt::size(s.sent_payload_bytes)),
        ];
        (texts, s.has_incoming_connections)
    }) else {
        return;
    };
    crate::icons::set_state(&ui.net_icon, network_icon(has_incoming));
    if let Some(t) = tray() {
        let tip = with_app(|a| {
            let s = &a.stats;
            tr!(TrayTip, down = fmt::speed_always(s.download_rate), up = fmt::speed_always(s.upload_rate))
        });
        if let Some(tip) = tip {
            t.set_tooltip(&tip);
        }
    }
    for (label, text) in ui.status.iter().zip(texts) {
        if label.text() != text {
            label.set_text(&text);
        }
    }
}

fn error(message: &str, detail: &str) {
    let Some(window) = with_app(|a| a.window.clone()) else { return };
    gtk::AlertDialog::builder().message(message).detail(detail).modal(true).build().show(Some(&window));
}

fn info(message: &str) {
    let Some(window) = with_app(|a| a.window.clone()) else { return };
    gtk::AlertDialog::builder().message(message).modal(true).build().show(Some(&window));
}

// --- actions -------------------------------------------------------------------------------

fn add_actions(window: &gtk::ApplicationWindow, list: &List) {
    let simple = |name: &str, f: fn()| {
        let a = gio::SimpleAction::new(name, None);
        a.connect_activate(move |_, _| f());
        window.add_action(&a);
    };
    simple("add", add_files);
    simple("add-url", add_url);
    simple("create", create_torrent);
    simple("rss", || show_rss(None));
    simple("feed-item-download", download_feed_items);
    for name in ["feed-add", "feed-edit", "feed-toggle", "feed-update", "feed-remove", "feed-rss"] {
        let a = gio::SimpleAction::new(name, None);
        a.connect_activate(move |_, _| feed_command(name));
        window.add_action(&a);
    }
    simple("remove", || remove(false));
    simple("remove-data", || remove(true));
    simple("open-folder", || open_folder(&selected()));
    simple("select-all", || {
        if let Some(ui) = ui() {
            ui.list.selection.select_all();
        }
    });
    simple("about", about);
    simple("preferences", preferences);
    simple("properties", show_properties);
    simple("set-location", set_download_location);
    simple("label-new", || {
        input(tr!(NewLabelTitle), tr!(NewLabelPrompt), "", |l| {
            if !l.is_empty() {
                set_label(&l);
            }
        })
    });
    simple("label-none", || set_label(""));
    let label = gio::SimpleAction::new_stateful("label", Some(glib::VariantTy::STRING), &"".to_variant());
    label.connect_activate(|_, v| {
        if let Some(l) = v.and_then(|v| v.get::<String>()) {
            set_label(&l);
        }
    });
    window.add_action(&label);
    for (name, download) in [("dl-limit", true), ("ul-limit", false)] {
        let a = gio::SimpleAction::new_stateful(name, Some(glib::VariantTy::UINT32), &0u32.to_variant());
        a.connect_activate(move |_, v| {
            if let Some(kib) = v.and_then(|v| v.get::<u32>()) {
                set_limit(download, kib.saturating_mul(1024));
            }
        });
        window.add_action(&a);
    }
    let super_seed = gio::SimpleAction::new_stateful("super-seed", None, &false.to_variant());
    super_seed.connect_activate(|_, _| toggle_super_seeding());
    window.add_action(&super_seed);
    // Options > Auto Shutdown: one check item per mode, the chosen one checked
    for (i, &mode) in AutoMode::MENU.iter().enumerate() {
        let a = gio::SimpleAction::new_stateful(&format!("auto-shutdown-{i}"), None, &(i == 0).to_variant());
        a.connect_activate(move |_, _| {
            with_app(|a| a.auto_shutdown.set_mode(mode));
            update_auto_shutdown_menu();
        });
        window.add_action(&a);
    }
    // Standby, Hibernate and Shut Down need logind; asked once the window is up
    let w = window.downgrade();
    glib::idle_add_local_once(move || {
        let Some(window) = w.upgrade() else { return };
        for (i, m) in AutoMode::MENU.iter().enumerate() {
            let Some(m) = m.filter(|m| m.action.is_power()) else { continue };
            if let Some(a) =
                window.lookup_action(&format!("auto-shutdown-{i}")).and_downcast::<gio::SimpleAction>()
            {
                a.set_enabled(shutdown::available(m.action));
            }
        }
    });
    for (i, &(_, prio)) in PRIORITIES.iter().enumerate() {
        let a = gio::SimpleAction::new(&format!("file-prio-{i}"), None);
        a.connect_activate(move |_, _| {
            let Some(rows) = ui().map(|ui| ui.details.selected_files()) else { return };
            with_app(|a| {
                let Some(id) = a.detail.id else { return };
                let t = a.handle.torrent(id);
                for f in rows.iter().filter_map(|&r| a.detail.file_index(r)) {
                    t.set_file_priority(f, prio);
                }
                // read the list back to show the new priorities
                a.files_requested = None;
            });
            request_detail();
        });
        window.add_action(&a);
    }
    for name in [
        "start",
        "force-start",
        "pause",
        "stop",
        "queue-up",
        "queue-down",
        "queue-top",
        "queue-bottom",
        "recheck",
        "reannounce",
    ] {
        let a = gio::SimpleAction::new(name, None);
        let n = name.to_owned();
        a.connect_activate(move |_, _| torrent_action(&n));
        window.add_action(&a);
    }
    // the header's menu: one check item per column
    let columns = gio::Menu::new();
    for (column, col) in &list.columns {
        let name = format!("column-{}", column.key());
        let action = gio::SimpleAction::new_stateful(&name, None, &col.is_visible().to_variant());
        let col = col.clone();
        action.connect_activate(move |a, _| {
            let on = !a.state().and_then(|s| s.get::<bool>()).unwrap_or(true);
            a.set_state(&on.to_variant());
            col.set_visible(on);
        });
        window.add_action(&action);
        columns.append(Some(column.title()), Some(&format!("win.{name}")));
    }
    for (_, col) in &list.columns {
        col.set_header_menu(Some(&columns));
    }
}

fn selected() -> Vec<TorrentId> {
    ui().map(|ui| ui.list.selected()).unwrap_or_default()
}

fn torrent_action(name: &str) {
    let ids = selected();
    with_app(|a| {
        // moving several up or to the top keeps their order: the first one goes last
        let ids: Vec<TorrentId> =
            if matches!(name, "queue-up" | "queue-top") { ids.iter().rev().copied().collect() } else { ids };
        for id in ids {
            let t = a.handle.torrent(id);
            match name {
                "start" => {
                    actions::start(&t);
                    set_paused(a, id, false);
                }
                "force-start" => {
                    actions::force_start(&t);
                    set_paused(a, id, false);
                }
                "pause" => {
                    actions::stop(&t);
                    set_paused(a, id, true);
                }
                "stop" => {
                    actions::stop(&t);
                    set_paused(a, id, false);
                }
                "queue-up" => t.queue_position(QueueMove::Up),
                "queue-down" => t.queue_position(QueueMove::Down),
                "queue-top" => t.queue_position(QueueMove::Top),
                "queue-bottom" => t.queue_position(QueueMove::Bottom),
                "recheck" => t.force_recheck(),
                "reannounce" => t.force_reannounce(),
                _ => {}
            }
        }
        a.handle.post_torrent_updates();
    });
}

/// The context menu's Properties: the Torrent Properties dialog for the selected torrents; OK
/// applies what changed to each one.
fn show_properties() {
    let ids = selected();
    let Some(Some((window, title, props))) = with_app(|a| {
        let ids: Vec<TorrentId> = ids.iter().copied().filter(|&id| a.list.get(id).is_some()).collect();
        if ids.is_empty() {
            return None;
        }
        let trackers =
            properties::fetch_trackers(&a.handle, &ids, &mut a.alerts, std::time::Duration::from_secs(2));
        let mut items = Vec::new();
        let mut names = Vec::new();
        for id in &ids {
            let Some(st) = a.list.get(*id) else { continue };
            items.push(TorrentProps::new(st, trackers.get(id).map(Vec::as_slice), &a.prefs));
            names.push(st.name.clone());
        }
        let defaults = GuiOptions::load(&a.prefs, &a.save_path).property_defaults();
        Some((a.window.clone(), crate::properties::title(&names), Properties::load(&items, defaults)))
    }) else {
        return;
    };
    // the alerts popped while waiting for the tracker lists
    glib::idle_add_local_once(on_alerts);
    crate::properties::show(&window, &title, props, move |changes| {
        let Some(changes) = changes.filter(|c| !c.is_empty()) else { return };
        with_app(|a| {
            let defaults = GuiOptions::load(&a.prefs, &a.save_path).property_defaults();
            for &id in &ids {
                let Some(st) = a.list.get(id) else { continue };
                let key = st.info_hashes.key_hex();
                changes.apply(&a.handle.torrent(id));
                changes.store(&mut a.prefs, &key, &defaults);
            }
            save_prefs(a);
            a.handle.post_torrent_updates();
        });
        request_detail();
    });
}

fn set_label(label: &str) {
    let ids = selected();
    with_app(|a| {
        for id in ids {
            a.list.set_label(id, label);
            if let Some(st) = a.list.get(id) {
                let key = format!("torrent.{}.label", st.info_hashes.key_hex());
                if label.is_empty() {
                    a.prefs.remove(&key);
                } else {
                    a.prefs.set(&key, label);
                }
            }
        }
        save_prefs(a);
    });
    refresh();
}

/// Bandwidth Allocation: the selected torrents' download or upload limit (bytes per second).
fn set_limit(download: bool, bytes: u32) {
    let ids = selected();
    with_app(|a| {
        for id in ids {
            let t = a.handle.torrent(id);
            if download {
                t.set_download_limit(bytes);
            } else {
                t.set_upload_limit(bytes);
            }
        }
        a.handle.post_torrent_updates();
    });
}

/// Advanced > Initial Seeding: on for all unless every selected torrent has it, as in µTorrent.
fn toggle_super_seeding() {
    let ids = selected();
    with_app(|a| {
        let (_, on) = super_seed_state(a, &ids);
        for id in ids {
            let t = a.handle.torrent(id);
            if on {
                t.unset_flags(TorrentFlags::SUPER_SEEDING);
            } else {
                t.set_flags(TorrentFlags::SUPER_SEEDING);
            }
        }
        a.handle.post_torrent_updates();
    });
}

/// Advanced > Set Download Location: a folder, then the selected torrents' files move there
/// (µTorrent 2.2.1 moves them; files already there are replaced, libtorrent's default).
fn set_download_location() {
    let ids = selected();
    let found = with_app(|a| {
        let st = a.list.get(*ids.first()?)?;
        Some((a.window.clone(), st.save_path.clone()))
    });
    let Some(Some((window, start))) = found else { return };
    let move_to = move |dir: String| {
        if dir.is_empty() {
            return;
        }
        with_app(|a| {
            for &id in &ids {
                let Some(st) = a.list.get(id) else { continue };
                if same_dir(&st.save_path, &dir) {
                    continue;
                }
                let name = st.name.clone();
                a.handle.torrent(id).move_storage(dir.clone(), MoveFlags::AlwaysReplaceFiles);
                a.detail.push_log(format!("{} {}", now_hms(), tr!(LogMovingTo, name, dir)));
            }
            a.handle.post_torrent_updates();
        });
        refresh_details();
    };
    if crate::remote::is_remote() {
        // a folder on the server, typed (the Web UI's question)
        let text = tr!(WebSetLocation);
        let (title, prompt) = text.split_once("\n\n").unwrap_or((text, text));
        return input(title, prompt, &start, move_to);
    }
    let dialog = gtk::FileDialog::builder().title(tr!(PickLocation)).modal(true).build();
    if std::path::Path::new(&start).is_dir() {
        dialog.set_initial_folder(Some(&gio::File::for_path(&start)));
    }
    dialog.select_folder(Some(&window), None::<&gio::Cancellable>, move |r| {
        if let Some(dir) = r.ok().and_then(|f| f.path()) {
            move_to(dir.to_string_lossy().into_owned());
        }
    });
}

fn set_paused(a: &mut App, id: TorrentId, paused: bool) {
    a.list.set_user_paused(id, paused);
    if let Some(st) = a.list.get(id) {
        let key = format!("torrent.{}.paused", st.info_hashes.key_hex());
        if paused {
            a.prefs.set_bool(&key, true);
        } else {
            a.prefs.remove(&key);
        }
    }
}

fn remove(with_data: bool) {
    let ids = selected();
    if ids.is_empty() {
        return;
    }
    let Some((window, confirm, names)) = with_app(|a| {
        let gui = GuiOptions::load(&a.prefs, &a.save_path);
        let names: Vec<String> =
            ids.iter().filter_map(|id| a.list.get(*id).map(|s| s.name.clone())).collect();
        (a.window.clone(), gui.confirm_delete || with_data, names)
    }) else {
        return;
    };
    let flags = if with_data {
        RemoveFlags::DELETE_FILES | RemoveFlags::DELETE_PARTFILE
    } else {
        RemoveFlags::DELETE_PARTFILE
    };
    let go = move || {
        with_app(|a| {
            for id in &ids {
                a.handle.remove_torrent(*id, flags);
            }
        });
    };
    if !confirm {
        go();
        return;
    }
    let message = match (names.as_slice(), with_data) {
        ([name], false) => tr!(AskRemove, name),
        ([name], true) => tr!(AskRemoveData, name),
        (_, false) => tr!(AskRemoveMany, count = names.len()),
        (_, true) => tr!(AskRemoveDataMany, count = names.len()),
    };
    let d = gtk::AlertDialog::builder()
        .message(message)
        .detail(if with_data { tr!(RemoveDeletesDataTrash) } else { tr!(RemoveKeepsData) })
        .buttons([tr!(BtnCancel), tr!(BtnRemove)])
        .cancel_button(0)
        .default_button(1)
        .modal(true)
        .build();
    d.choose(Some(&window), None::<&gio::Cancellable>, move |r| {
        if r == Ok(1) {
            go();
        }
    });
}

fn open_folder(ids: &[TorrentId]) {
    // (a remote window's double click on a finished torrent does nothing, as the Web UI's)
    if crate::remote::is_remote() {
        return;
    }
    let Some((window, path)) = with_app(|a| {
        let st = a.list.get(*ids.first()?)?;
        Some((a.window.clone(), PathBuf::from(&st.save_path).join(&st.name)))
    })
    .flatten() else {
        return;
    };
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
    launcher.open_containing_folder(Some(&window), None::<&gio::Cancellable>, |r| {
        if let Err(e) = r {
            eprintln!("cannot open the folder: {e}");
        }
    });
}

fn add_files() {
    let Some(window) = with_app(|a| a.window.clone()) else { return };
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(tr!(FilterTorrents)));
    filter.add_pattern("*.torrent");
    filter.add_mime_type("application/x-bittorrent");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder().title(tr!(PickTorrent)).modal(true).filters(&filters).build();
    dialog.open_multiple(Some(&window), None::<&gio::Cancellable>, |r| {
        let Ok(files) = r else { return };
        for f in files.iter::<gio::File>().flatten() {
            open_source(&f);
        }
    });
}

/// A `magnet:` link or the `http(s)://` URL of a `.torrent`.
fn open_link(link: String) {
    if link.starts_with("magnet:") {
        add_magnet(link);
    } else {
        download(link);
    }
}

/// A `.torrent` file from the command line, another instance or a dialog.
fn open_source(f: &gio::File) {
    let uri = f.uri().to_string();
    if uri.starts_with("http://") || uri.starts_with("https://") {
        download(uri);
    } else if let Some(path) = f.path() {
        match std::fs::read(&path) {
            Ok(bytes) => add_torrent_file(bytes),
            Err(e) => error(&tr!(CannotOpen, path = path.display()), &e.to_string()),
        }
    }
}

/// A `.torrent`'s contents, through the Add New Torrent dialog unless Preferences turned it off
/// (files the engine cannot read are handed to it anyway, so it reports why).
fn add_torrent_file(bytes: Vec<u8>) {
    let Some((window, show, setup)) = with_app(|a| {
        let gui = GuiOptions::load(&a.prefs, &a.save_path);
        let setup = add::Setup::new(&gui, &a.prefs, labels(a));
        (a.window.clone(), gui.show_add_dialog, setup)
    }) else {
        return;
    };
    let ti = match TorrentInfo::from_bytes(&bytes) {
        Ok(ti) if show => ti,
        _ => return add(TorrentSource::TorrentFile(bytes)),
    };
    let key = ti.info_hashes().key_hex();
    if with_app(|a| in_list(a, &key).is_some()) == Some(true) {
        info(&tr!(InfoAlreadyInList, name = ti.name()));
        return;
    }
    let ti2 = ti.clone();
    crate::add::show(&window, &ti, setup, move |choice| {
        let Some(choice) = choice else { return };
        with_app(|a| {
            add::remember_dir(&mut a.prefs, &choice.save_path);
            if !choice.label.is_empty() {
                // picked up when the torrent first shows up in the list, like a restored label
                a.prefs.set(&format!("torrent.{key}.label"), &choice.label);
            }
            if choice.top_of_queue {
                a.queue_top.insert(key);
            }
            save_prefs(a);
            let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(bytes), a.save_path.clone());
            GuiOptions::load(&a.prefs, &a.save_path).apply_to_new(&mut p);
            choice.apply_to(&ti2, &mut p);
            add_to_session(a, p);
        });
    });
}

/// A magnet link. With the Add New Torrent dialog on, the magnet joins the session in upload
/// mode to fetch its metadata, and the dialog opens once the file list is known
/// ([`add::AwaitingMetadata`], [`magnet_dialogs`]); otherwise it is added directly.
fn add_magnet(uri: String) {
    let show = with_app(|a| GuiOptions::load(&a.prefs, &a.save_path).show_add_dialog).unwrap_or(false);
    let key = match parse_magnet_uri(&uri) {
        Ok(m) if show => m.info_hashes.key_hex(),
        // the engine reports a broken link
        _ => return add(TorrentSource::Magnet(uri)),
    };
    if let Some(name) = with_app(|a| in_list(a, &key)).flatten() {
        info(&tr!(InfoAlreadyInList, name));
        return;
    }
    with_app(|a| {
        let save_path = a.save_path.clone();
        let mut p = AddTorrentParams::new(TorrentSource::Magnet(uri), save_path.clone());
        GuiOptions::load(&a.prefs, &a.save_path).apply_to_new(&mut p);
        add::AwaitingMetadata::prepare(&mut p);
        a.awaiting.insert(&mut a.prefs, &key, &save_path);
        save_prefs(a);
        add_to_session(a, p);
    });
}

/// The Add New Torrent dialog for each magnet whose metadata arrived, one at a time. OK gives
/// the torrent the choices and starts it (or leaves it stopped); Cancel removes it with its
/// part file (nothing else was written).
fn magnet_dialogs() {
    let next = with_app(|a| {
        if a.awaiting_dialog {
            return None;
        }
        let key = a.awaiting.ready().into_iter().next()?;
        let w = a.awaiting.get(&key)?.clone();
        let gui = GuiOptions::load(&a.prefs, &a.save_path);
        let setup = add::Setup::for_magnet(&gui, &a.prefs, labels(a), &w.save_path);
        a.awaiting_dialog = true;
        Some((a.window.clone(), key, w, setup))
    })
    .flatten();
    let Some((window, key, w, setup)) = next else { return };
    let (Some(id), Some(ti)) = (w.id, w.ti.clone()) else { return };
    crate::add::show(&window, &ti.clone(), setup, move |choice| {
        with_app(|a| {
            a.awaiting_dialog = false;
            a.awaiting.finish(&mut a.prefs, &key);
            match choice {
                Some(c) => {
                    add::remember_dir(&mut a.prefs, &c.save_path);
                    if !c.label.is_empty() {
                        a.prefs.set(&format!("torrent.{key}.label"), &c.label);
                        a.list.set_label(id, &c.label);
                    }
                    let seed_only = a.scheduler.mode() == Mode::SeedOnly;
                    c.apply_to_torrent(&a.handle.torrent(id), &ti, &w.save_path, seed_only);
                    a.handle.post_torrent_updates();
                }
                None => a.handle.remove_torrent(id, RemoveFlags::DELETE_PARTFILE),
            }
            save_prefs(a);
        });
        // the next one, if another is ready
        glib::idle_add_local_once(magnet_dialogs);
    });
}

/// Adds without the dialog, with the Preferences' defaults.
fn add(source: TorrentSource) {
    with_app(|a| {
        let mut p = AddTorrentParams::new(source, a.save_path.clone());
        GuiOptions::load(&a.prefs, &a.save_path).apply_to_new(&mut p);
        add_to_session(a, p);
    });
}

/// Hands a torrent to the engine (in upload mode during Seeding only hours); a `.torrent`'s copy
/// is stored once it is added.
fn add_to_session(a: &mut App, mut p: AddTorrentParams) {
    a.scheduler.apply_to_new(&mut p);
    a.torrent_files.adding(&a.torrent_folders, &p);
    a.handle.add_torrent(p);
}

/// Preferences > Directories "Automatically load .torrents from": the new files in the folder
/// are added without the Add New Torrent dialog, as µTorrent added them, and renamed to
/// `.torrent.loaded` (or deleted) once the engine has them.
fn autoload(a: &mut App) {
    let Some(dir) = a.torrent_folders.loads_from().map(PathBuf::from) else { return };
    let found = a.autoload.poll(&dir, std::time::Instant::now());
    if let Some(e) = a.autoload.take_error() {
        a.detail.push_log(format!("{} {}", now_hms(), tr!(LogAutoloadError, error = e)));
    }
    for path in found {
        let (bytes, ti) = match torrent_folders::read(&path) {
            Ok(r) => r,
            Err(e) => {
                let line = tr!(FoldersNotLoaded, path = path.display(), error = e);
                a.detail.push_log(format!("{} {line}", now_hms()));
                continue;
            }
        };
        if in_list(a, &ti.info_hashes().key_hex()).is_some() {
            let line = torrent_folders::loaded(ti.name(), &path, a.torrent_folders.autoload_delete);
            a.detail.push_log(format!("{} {}", now_hms(), tr!(LogAlreadyInList, line)));
            continue;
        }
        a.torrent_files.loading(&ti, path);
        let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(bytes), a.save_path.clone());
        GuiOptions::load(&a.prefs, &a.save_path).apply_to_new(&mut p);
        add_to_session(a, p);
    }
}

/// The Auto Shutdown items' check marks: the mode in force.
fn update_auto_shutdown_menu() {
    let Some((window, mode)) = with_app(|a| (a.window.clone(), a.auto_shutdown.mode())) else { return };
    for (i, &m) in AutoMode::MENU.iter().enumerate() {
        if let Some(a) =
            window.lookup_action(&format!("auto-shutdown-{i}")).and_downcast::<gio::SimpleAction>()
        {
            a.set_state(&(m == mode).to_variant());
        }
    }
}

/// Options > Auto Shutdown: the timer's step. The countdown opens when the work is done and
/// closes when time is up (then it acts) or new work comes.
fn auto_shutdown_step(step: Step) {
    match step {
        Step::Wait => {}
        Step::Start(secs) => {
            let Some(Some((window, mode))) =
                with_app(|a| a.auto_shutdown.mode().map(|m| (a.window.clone(), m)))
            else {
                return;
            };
            log_line(&tr!(LogShutdownDone, what = mode.action.doing(), secs));
            shutdown::open(&window, mode, secs, || {
                with_app(|a| a.auto_shutdown.set_mode(None));
                log_line(tr!(LogShutdownCancelled));
                update_auto_shutdown_menu();
            });
        }
        Step::Tick(secs) => shutdown::tick(secs),
        Step::Abort => {
            shutdown::close();
            log_line(tr!(LogShutdownNewWork));
        }
        Step::Act(action) => {
            shutdown::close();
            auto_shutdown_act(action);
        }
    }
}

/// Quit as File > Exit does (state saved on the way out); Shut Down after that; Standby and
/// Hibernate once the engine has written the resume data (`ResumeDataSaved` in `on_alerts`).
fn auto_shutdown_act(action: auto_shutdown::Action) {
    log_line(&tr!(LogShutdown, what = action.doing()));
    update_auto_shutdown_menu();
    match Power::of(action) {
        Some(p @ (Power::Standby | Power::Hibernate)) => {
            with_app(|a| {
                save_prefs(a);
                a.suspend_pending = Some(p);
                a.handle.save_resume_data();
            });
        }
        p => {
            if let Some(p) = p {
                shutdown::after_exit(p);
            }
            quit_app();
        }
    }
}

fn log_line(text: &str) {
    with_app(|a| a.detail.push_log(format!("{} {text}", now_hms())));
}

/// The local clock: seconds since 1970 and the zone's offset east of UTC.
fn clock() -> Option<(i64, i32)> {
    let t = glib::DateTime::now_local().ok()?;
    Some((t.to_unix(), t.utc_offset().as_seconds() as i32))
}

/// Puts the scheduler's mode for this hour into effect. `applied`: the options just applied
/// (their settings go to the engine through the scheduler, even if the mode stays).
fn run_scheduler(a: &mut App, applied: Option<&Options>) {
    let Some((now, offset)) = clock() else {
        if let Some(o) = applied {
            a.handle.apply_settings(o.settings.clone());
        }
        return;
    };
    if applied.is_none() && a.schedule.mode_at(now, offset) == a.scheduler.mode() {
        return;
    }
    let loaded;
    let o = match applied {
        Some(o) => o,
        None => {
            loaded = Options::load(&a.prefs, &a.save_path);
            &loaded
        }
    };
    let old = a.scheduler.mode();
    let Some(c) = a.scheduler.update(&a.schedule, &o.settings, now, offset, applied.is_some()) else {
        return;
    };
    if let Some(s) = c.settings {
        a.handle.apply_settings(s);
    }
    match c.pause_session {
        Some(true) => a.handle.pause(),
        Some(false) => a.handle.resume(),
        None => {}
    }
    if let Some(on) = c.upload_mode {
        for id in a.list.ids() {
            let t = a.handle.torrent(id);
            if on {
                t.set_flags(TorrentFlags::UPLOAD_MODE);
            } else if !a.awaiting.holds(id) {
                // a magnet waiting for the Add dialog stays in upload mode until its OK
                t.unset_flags(TorrentFlags::UPLOAD_MODE);
            }
        }
    }
    if c.mode != old {
        a.detail.push_log(format!("{} {}", now_hms(), tr!(LogScheduler, mode = c.mode.label())));
    }
}

/// Preferences > Queueing's seeding goal (or the torrent's own, from Properties) for a finished
/// torrent.
fn seeding_goal(a: &mut App, st: &patorrent_core::TorrentStatus) {
    if !st.is_finished {
        return;
    }
    let own = SeedGoal::load_override(&a.prefs, &st.info_hashes.key_hex());
    let Some(action) = a.seeding_goal.check(st, own) else { return };
    action.apply(&a.handle.torrent(st.id));
    let what = match action {
        GoalAction::Stop => tr!(SeedingGoalStopped).to_owned(),
        GoalAction::LimitUpload(b) => tr!(SeedingGoalLimited, rate = fmt::speed_always(b)),
    };
    a.detail.push_log(format!("{} {}", now_hms(), tr!(LogSeedingGoal, name = st.name, what)));
}

/// The name of the torrent in the list with info-hash `key` (`InfoHashes::key_hex`).
fn in_list(a: &App, key: &str) -> Option<String> {
    a.list
        .ids()
        .find_map(|id| a.list.get(id).filter(|s| s.info_hashes.key_hex() == key))
        .map(|s| s.name.clone())
}

/// The labels in use, for the dialog's Label list.
fn labels(a: &App) -> Vec<String> {
    a.list.labels().into_iter().map(|(l, _)| l).collect()
}

fn save_prefs(a: &mut App) {
    if let Err(e) = a.prefs.save() {
        a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
    }
}

/// Fetches a `.torrent` on a worker thread, then adds it.
fn download(url: String) {
    std::thread::spawn(move || {
        let result = http_get(&url, &FetchOptions::default())
            .map_err(|e| e.to_string())
            .and_then(|r| if r.status == 200 { Ok(r.body) } else { Err(format!("HTTP {}", r.status)) });
        glib::MainContext::default().invoke(move || match result {
            Ok(bytes) => add_torrent_file(bytes),
            Err(e) => error(&tr!(CannotDownload, url), &e),
        });
    });
}

/// A one-line input box (New Label); `done` gets the trimmed text on OK.
fn input(title: &str, prompt: &str, initial: &str, done: impl Fn(String) + 'static) {
    let Some(window) = with_app(|a| a.window.clone()) else { return };
    let dialog = gtk::Window::builder()
        .title(title)
        .transient_for(&window)
        .modal(true)
        .default_width(360)
        .resizable(false)
        .build();
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    let ok = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnOk)));
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnCancel)));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&cancel);
    buttons.append(&ok);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    let label = gtk::Label::new(Some(prompt));
    label.set_xalign(0.0);
    label.set_wrap(true);
    body.append(&label);
    body.append(&entry);
    body.append(&buttons);
    dialog.set_child(Some(&body));
    dialog.set_default_widget(Some(&ok));
    let d = dialog.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(d) = d.upgrade() {
            d.close();
        }
    });
    let (d, e) = (dialog.downgrade(), entry.downgrade());
    ok.connect_clicked(move |_| {
        let (Some(d), Some(e)) = (d.upgrade(), e.upgrade()) else { return };
        let text = e.text().trim().to_owned();
        d.close();
        done(text);
    });
    dialog.present();
}

fn add_url() {
    let Some(window) = with_app(|a| a.window.clone()) else { return };
    let dialog = gtk::Window::builder()
        .title(tr!(AddUrlTitle))
        .transient_for(&window)
        .modal(true)
        .default_width(460)
        .resizable(false)
        .build();
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some(tr!(UrlPlaceholder)));
    entry.set_activates_default(true);
    // a link on the clipboard is offered
    let clip = entry.clipboard();
    let e = entry.downgrade();
    clip.read_text_async(None::<&gio::Cancellable>, move |r| {
        if let (Ok(Some(t)), Some(e)) = (r, e.upgrade()) {
            let t = t.trim();
            if e.text().is_empty() && (t.starts_with("magnet:") || t.starts_with("http")) {
                e.set_text(t);
                e.select_region(0, -1);
            }
        }
    });
    let ok = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnOk)));
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnCancel)));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&cancel);
    buttons.append(&ok);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    let prompt = gtk::Label::new(Some(tr!(AddUrlPrompt)));
    prompt.set_xalign(0.0);
    body.append(&prompt);
    body.append(&entry);
    body.append(&buttons);
    dialog.set_child(Some(&body));
    dialog.set_default_widget(Some(&ok));
    let d = dialog.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(d) = d.upgrade() {
            d.close();
        }
    });
    let (d, e) = (dialog.downgrade(), entry.downgrade());
    ok.connect_clicked(move |_| {
        let (Some(d), Some(e)) = (d.upgrade(), e.upgrade()) else { return };
        let url = e.text().trim().to_owned();
        d.close();
        if url.starts_with("magnet:") {
            add_magnet(url);
        } else if url.starts_with("http://") || url.starts_with("https://") {
            download(url);
        } else if !url.is_empty() {
            error(tr!(ErrNotALink), &url);
        }
    });
    dialog.present();
}

/// Options > Preferences. OK and Apply store the options and apply them without a restart.
fn preferences() {
    let Some((window, mut options)) = with_app(|a| (a.window.clone(), Options::load(&a.prefs, &a.save_path)))
    else {
        return;
    };
    // as the system has it now
    options.integration = crate::integration::read();
    let hooks = crate::preferences::Hooks {
        apply: Box::new(|o| {
            with_app(|a| apply_options(a, o));
            apply_integration(o);
        }),
        reload_ip_filter: Box::new(|path| {
            with_app(|a| {
                let r = a.handle.load_ip_filter(path).map_err(|e| e.to_string());
                log_ip_filter(a, path, &r);
                r
            })
            .unwrap_or_else(|| Err(tr!(ErrNotRunning).to_owned()))
        }),
        settings_dir: paths::settings_dir(),
        tray: tray_shown(),
    };
    crate::preferences::show(&window, options, hooks);
}

/// Stores `o` and hands it to the engine and the window.
fn apply_options(a: &mut App, o: &Options) {
    let old = Options::load(&a.prefs, &a.save_path);
    o.store(&mut a.prefs);
    save_prefs(a);
    if crate::remote::is_remote() && o.gui.language != old.gui.language {
        // one language for every window: it lives in the local client's settings
        let mut base = Prefs::load(paths::config_dir().join("patorrent.ini"));
        match o.gui.language.trim() {
            "" => base.remove("ui.language"),
            code => base.set("ui.language", code),
        }
        if let Err(e) = base.save() {
            a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
        }
    }
    a.save_path = o.gui.download_dir.clone();
    // the engine settings, as the scheduler's hour changes them
    a.schedule = o.gui.schedule.clone();
    run_scheduler(a, Some(o));
    o.gui.update_move_completed(&mut a.move_completed);
    a.seeding_goal = o.gui.seeding_goal();
    a.torrent_folders = o.gui.torrent_folders.clone();
    if o.gui.pex != old.gui.pex {
        for id in a.list.ids() {
            let t = a.handle.torrent(id);
            if o.gui.pex {
                t.unset_flags(TorrentFlags::DISABLE_PEX);
            } else {
                t.set_flags(TorrentFlags::DISABLE_PEX);
            }
        }
    }
    let dir = paths::settings_dir();
    let changed =
        (o.gui.ip_filter, o.gui.ip_filter_file(&dir)) != (old.gui.ip_filter, old.gui.ip_filter_file(&dir));
    // a server applies its own block list
    if changed && !crate::remote::is_remote() {
        if o.gui.ip_filter {
            load_ip_filter(a, &o.gui);
        } else {
            a.handle.set_ip_filter(IpFilter::new());
            a.detail.push_log(format!("{} {}", now_hms(), tr!(LogIpFilterOff)));
        }
    }
}

/// Loads the configured block list (`ipfilter.dat` in the settings folder by default).
fn load_ip_filter(a: &mut App, gui: &GuiOptions) {
    let path = gui.ip_filter_file(&paths::settings_dir());
    let r = a.handle.load_ip_filter(&path).map_err(|e| e.to_string());
    log_ip_filter(a, &path, &r);
}

fn log_ip_filter(a: &mut App, path: &std::path::Path, r: &Result<usize, String>) {
    let line = match r {
        Ok(n) => format!("{} {}", now_hms(), tr!(LogIpFilterRules, count = n, path = path.display())),
        Err(e) => format!("{} {}", now_hms(), tr!(LogIpFilterError, path = path.display(), error = e)),
    };
    a.detail.push_log(line);
}

// ---- RSS ------------------------------------------------------------------------------------
// The RSS Downloader (`patorrent_frontend::rss`): the timer starts the refreshes that are due,
// the fetches run on the worker's threads and wake the main loop, and `rss_collect` takes their
// results here and adds what the filters chose.

/// `rss.dat`, beside `patorrent.ini`.
fn rss_file() -> PathBuf {
    paths::settings_dir().join(patorrent_frontend::rss::store::FILE_NAME)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The proxy feeds and their downloads go through: the engine's, when it carries HTTP
/// tracker connections.
fn rss_proxy(a: &App) -> Option<FetchProxy> {
    FetchProxy::from_settings(&Options::load(&a.prefs, &a.save_path).settings)
}

/// Once a second: the refreshes that are due, and `rss.dat` when it changed.
fn rss_timer(a: &mut App) {
    let now = unix_now();
    if a.rss.next_due().is_some_and(|t| t <= now)
        && a.rss.due_feeds(now).iter().any(|&id| !a.rss_worker.is_busy(id))
    {
        let proxy = rss_proxy(a);
        a.rss_worker.refresh_due(&a.rss, now, proxy.as_ref());
    }
    if a.rss_dirty {
        a.rss_dirty = false;
        if let Err(e) = a.rss.save(&rss_file()) {
            let line = tr!(LogNotSaved, file = rss_file().display(), error = e);
            a.detail.push_log(format!("{} {line}", now_hms()));
        }
    }
}

/// Starts refreshing feed `feed` now (`None`: every enabled feed).
fn rss_update(a: &mut App, feed: Option<FeedId>) {
    let proxy = rss_proxy(a);
    let ids: Vec<FeedId> = match feed {
        Some(id) => vec![id],
        None => a.rss.feeds.iter().filter(|f| f.enabled).map(|f| f.id).collect(),
    };
    for id in ids {
        a.rss_worker.refresh(&a.rss, id, proxy.as_ref());
    }
}

/// A fetch finished: the results into the downloader, and their torrents into the session.
fn rss_collect() {
    let Some(changed) = with_app(|a| {
        let proxy = rss_proxy(a);
        let c = a.rss_worker.collect(&mut a.rss, unix_now(), proxy.as_ref());
        for line in c.log {
            a.detail.push_log(format!("{} {line}", now_hms()));
        }
        for (req, d) in c.adds {
            rss_add(a, req, d);
        }
        a.rss_dirty |= c.changed;
        c.changed
    }) else {
        return;
    };
    if changed {
        rss_changed();
    }
    refresh_details();
}

/// What follows the downloader: the open RSS Downloader window, the Feeds rows and the feed
/// items in the list.
fn rss_changed() {
    crate::rss::changed();
    refresh();
    refresh_feed_rows();
}

/// Shows feed items in the list (`Some(None)`: every feed's) or, with `None`, the torrents.
fn show_feed_items(view: Option<Option<FeedId>>) {
    let Some(ui) = ui() else { return };
    let Some(n) = with_app(|a| {
        a.feed_view = view;
        a.feed_rows = view.map(|f| rss_view::item_rows(&a.rss, f)).unwrap_or_default();
        a.feed_rows.len()
    }) else {
        return;
    };
    let child: gtk::Widget =
        if view.is_some() { ui.feed_table.view.clone().upcast() } else { ui.list.view.clone().upcast() };
    if ui.list_scroll.child().as_ref() != Some(&child) {
        ui.list_scroll.set_child(Some(&child));
    }
    if view.is_some() {
        ui.feed_table.unselect_all();
        ui.feed_table.update(n);
    }
}

/// The feed items again after a fetch (when the list shows them).
fn refresh_feed_rows() {
    let Some(ui) = ui() else { return };
    let n = with_app(|a| {
        let view = a.feed_view?;
        a.feed_rows = rss_view::item_rows(&a.rss, view);
        Some(a.feed_rows.len())
    })
    .flatten();
    if let Some(n) = n {
        ui.feed_table.update(n);
    }
}

/// Downloads the selected feed items (a double click, or the menu's Download).
fn download_feed_items() {
    let Some(rows) = ui().map(|ui| ui.feed_table.selected()) else { return };
    with_app(|a| {
        let now = unix_now();
        let picked: Vec<(FeedId, String)> =
            rows.iter().filter_map(|&r| a.feed_rows.get(r)).map(|r| (r.feed, r.id.clone())).collect();
        let reqs: Vec<AddRequest> =
            picked.into_iter().filter_map(|(feed, id)| a.rss.request_item(feed, &id, now)).collect();
        rss_download(a, reqs);
    });
    refresh_feed_rows();
    refresh_details();
}

/// The right-click menu of a Feeds row (µTorrent's: Add RSS Feed, Edit Feed, Disable Feed,
/// Update Feed, Delete Feed), for `feed` (`None`: the Feeds row).
fn feed_menu(feed: Option<FeedId>) -> gio::Menu {
    let enabled = with_app(|a| feed.and_then(|id| a.rss.feed(id)).is_none_or(|f| f.enabled)).unwrap_or(true);
    if let Some(window) = with_app(|a| a.window.clone()) {
        for name in ["feed-edit", "feed-toggle", "feed-remove"] {
            if let Some(a) = window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                a.set_enabled(feed.is_some());
            }
        }
    }
    let menu = gio::Menu::new();
    let feeds = gio::Menu::new();
    item(&feeds, tr!(MenuAddFeed), "win.feed-add");
    item(&feeds, tr!(MenuEditFeed), "win.feed-edit");
    item(&feeds, if enabled { tr!(MenuDisableFeed) } else { tr!(MenuEnableFeed) }, "win.feed-toggle");
    item(&feeds, tr!(MenuUpdateFeed), "win.feed-update");
    item(&feeds, tr!(MenuDeleteFeed), "win.feed-remove");
    menu.append_section(None, &feeds);
    let more = gio::Menu::new();
    item(&more, tr!(MenuRss), "win.feed-rss");
    menu.append_section(None, &more);
    menu
}

/// A Feeds row menu's command on the feed it was opened for.
fn feed_command(name: &str) {
    let Some((window, feed)) = with_app(|a| (a.window.clone(), a.menu_feed)) else { return };
    match name {
        "feed-add" => crate::rss::add_feed_dialog(window.upcast_ref(), |form| {
            with_app(|a| {
                let id = form.add_to(&mut a.rss);
                a.rss_dirty = true;
                rss_update(a, Some(id));
            });
            rss_changed();
        }),
        "feed-edit" => {
            let Some(id) = feed else { return };
            let Some(form) = with_app(|a| a.rss.feed(id).map(FeedForm::of)).flatten() else { return };
            crate::rss::feed_dialog(window.upcast_ref(), tr!(RssEditFeed), form, move |form| {
                with_app(|a| {
                    if form.store(&mut a.rss, id) {
                        rss_update(a, Some(id));
                    }
                    a.rss_dirty = true;
                });
                rss_changed();
            });
        }
        "feed-toggle" => {
            with_app(|a| {
                if let Some(f) = feed.and_then(|id| a.rss.feed_mut(id)) {
                    f.enabled = !f.enabled;
                    a.rss_dirty = true;
                }
            });
        }
        "feed-update" => {
            with_app(|a| rss_update(a, feed));
        }
        "feed-remove" => {
            let Some(id) = feed else { return };
            let name =
                with_app(|a| a.rss.feed(id).map(|f| f.name().to_owned())).flatten().unwrap_or_default();
            let d = gtk::AlertDialog::builder()
                .message(tr!(AskDeleteFeed, name))
                .buttons([tr!(BtnCancel), tr!(BtnOk)])
                .cancel_button(0)
                .default_button(1)
                .modal(true)
                .build();
            d.choose(Some(&window), None::<&gio::Cancellable>, move |r| {
                if r == Ok(1) {
                    with_app(|a| {
                        a.rss.remove_feed(id);
                        a.rss_dirty = true;
                    });
                    rss_changed();
                }
            });
        }
        "feed-rss" => show_rss(feed),
        _ => {}
    }
}

/// Adds a torrent the downloader asked for: its label, "highest priority" (the top of the
/// queue once added) and "paused state" (µTorrent's Pause) as the Add dialog's would be.
fn rss_add(a: &mut App, req: AddRequest, d: Download) {
    if let Some(key) = d.info_hashes().map(|h| h.key_hex()) {
        if in_list(a, &key).is_some() {
            a.detail.push_log(format!("{} {}", now_hms(), tr!(LogRssAlreadyInList, title = req.title)));
            return;
        }
        // picked up when the torrent first shows up in the list, like a restored label
        if !req.label.is_empty() {
            a.prefs.set(&format!("torrent.{key}.label"), &req.label);
        }
        if req.paused {
            a.prefs.set_bool(&format!("torrent.{key}.paused"), true);
        }
        if req.top_priority {
            a.queue_top.insert(key);
        }
    }
    let magnet = matches!(d, Download::Magnet(_));
    let mut p = AddTorrentParams::new(d.into_source(), a.save_path.clone());
    GuiOptions::load(&a.prefs, &a.save_path).apply_to_new(&mut p);
    req.apply_to(&mut p);
    if magnet {
        // the item's title until the metadata names it
        p.name = Some(req.title.clone());
    }
    add_to_session(a, p);
    save_prefs(a);
}

/// Starts the downloads `reqs` ask for (the magnet links are added at once).
fn rss_download(a: &mut App, reqs: Vec<AddRequest>) {
    if reqs.is_empty() {
        return;
    }
    for r in &reqs {
        let feed = a.rss.feed(r.feed).map(|f| f.name().to_owned()).unwrap_or_default();
        a.detail.push_log(format!("{} {}", now_hms(), tr!(RssLogDownloading, name = feed, title = r.title)));
    }
    let proxy = rss_proxy(a);
    for (req, d) in a.rss_worker.download(reqs, proxy.as_ref()) {
        rss_add(a, req, d);
    }
    a.rss_dirty = true;
}

/// Options > RSS Downloader, with `feed` picked. The window edits the live downloader; when it
/// closes the filters run over the feeds' current items (they may have changed).
fn show_rss(feed: Option<FeedId>) {
    let Some(window) = with_app(|a| a.window.clone()) else { return };
    crate::rss::show(&window, Rc::new(AppHost), feed, || {
        with_app(|a| {
            let now = unix_now();
            let ids: Vec<FeedId> = a.rss.feeds.iter().map(|f| f.id).collect();
            let reqs: Vec<AddRequest> = ids.into_iter().flat_map(|id| a.rss.process(id, now)).collect();
            rss_download(a, reqs);
            a.rss_dirty = true;
        });
        rss_changed();
        refresh_details();
    });
}

/// What the RSS Downloader window asks of the main window.
struct AppHost;

impl crate::rss::Host for AppHost {
    fn view(&self, f: &mut dyn FnMut(&RssDownloader)) {
        with_app(|a| f(&a.rss));
    }

    fn edit(&self, f: &mut dyn FnMut(&mut RssDownloader)) {
        with_app(|a| {
            f(&mut a.rss);
            a.rss_dirty = true;
        });
    }

    fn updating(&self, id: FeedId) -> bool {
        with_app(|a| a.rss_worker.is_busy(id)).unwrap_or(false)
    }

    fn update(&self, id: Option<FeedId>) {
        with_app(|a| rss_update(a, id));
    }

    fn labels(&self) -> Vec<String> {
        with_app(|a| labels(a)).unwrap_or_default()
    }
}

/// File > Create New Torrent; with "Start seeding" the new torrent is added with the source's
/// folder as its save path (the files are checked, then it seeds).
fn create_torrent() {
    let Some((window, r)) = with_app(|a| (a.window.clone(), create::Remembered::load(&a.prefs))) else {
        return;
    };
    crate::create::show(&window, r, |r, created| {
        with_app(|a| {
            r.store(&mut a.prefs);
            save_prefs(a);
            if let Some((created, Some(save))) = created {
                add_to_session(a, create::seed_params(created.bytes, &save));
            }
        });
    });
}

fn about() {
    let Some(window) = with_app(|a| a.window.clone()) else { return };
    gtk::AboutDialog::builder()
        .transient_for(&window)
        .modal(true)
        .program_name("PaTorrent")
        .logo_icon_name(APP_ID)
        .version(patorrent_core::VERSION)
        .comments(tr!(AboutShort))
        .website("https://github.com/Balls0fSteel/PaTorrent")
        .build()
        .present();
}
