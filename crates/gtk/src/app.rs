//! The application: the engine session, the main window (menu bar, toolbar, category list,
//! torrent list, detail tabs, status bar), the alert loop and the torrent actions.
//!
//! The engine's alert notification runs on the engine thread; it only asks the main loop to
//! drain the queue (`MainContext::invoke`). Everything else happens on the GTK thread, with
//! the state in a thread-local [`App`] (the widgets that draw from it in [`Ui`], so drawing
//! never runs while the state is borrowed).

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_core::fetch::{FetchOptions, http_get};
use patorrent_core::metainfo::{TorrentInfo, parse_magnet_uri};
use patorrent_core::{
    AddTorrentParams, Alert, IpFilter, MoveCompleted, QueueMove, RemoveFlags, Session, SessionHandle,
    SessionParams, SessionStats, TorrentFlags, TorrentId, TorrentSource,
};
use patorrent_frontend::columns::Column;
use patorrent_frontend::detail::{Detail, PRIORITIES, Tab};
use patorrent_frontend::lang::{self, gtk_mnemonic};
use patorrent_frontend::model::TorrentList;
use patorrent_frontend::options::{self, GuiOptions, Options};
use patorrent_frontend::prefs::Prefs;
use patorrent_frontend::seeding::{GoalAction, SeedGoal, SeedingGoal};
use patorrent_frontend::torrent_folders::{self, Poller, TorrentFiles, TorrentFolders};
use patorrent_frontend::{actions, add, fmt, settings, tr};

use crate::detail::{Details, Visit};
use crate::list::{Content, List};
use crate::paths;
use crate::sidebar::Sidebar;

/// The application id (D-Bus name, desktop file, single instance).
pub const APP_ID: &str = "io.github.balls0fsteel.PaTorrent";

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
}

/// The widgets that draw from [`App`].
struct Ui {
    list: List,
    sidebar: Rc<Sidebar>,
    status: [gtk::Label; 4],
    paned: gtk::Paned,
    details: Rc<Details>,
    /// The list over the detail tabs.
    split: gtk::Paned,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
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
    // `magnet:` argument into a file); a second start hands it to the first
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_startup(startup);
    app.connect_activate(|app| {
        if let Some(w) = app.active_window() {
            w.present();
        } else {
            build(app);
        }
    });
    app.connect_command_line(|app, cmdline| {
        app.activate();
        for arg in cmdline.arguments().iter().skip(1) {
            let arg = arg.to_string_lossy();
            if arg.starts_with("--") {
                continue;
            }
            if arg.starts_with("magnet:") || arg.starts_with("http://") || arg.starts_with("https://") {
                open_link(arg.into_owned());
            } else {
                // relative to the directory of the process that was started
                open_source(&cmdline.create_file_for_arg(&*arg));
            }
        }
        glib::ExitCode::SUCCESS
    });
    let code = app.run();
    ExitCode::from(u8::from(code))
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
    }
    let quit = gio::SimpleAction::new("quit", None);
    let weak = app.downgrade();
    quit.connect_activate(move |_, _| {
        if let Some(app) = weak.upgrade() {
            for w in app.windows() {
                w.close();
            }
        }
    });
    app.add_action(&quit);
    for (action, keys) in [
        ("win.add", &["<Control>o"][..]),
        ("win.add-url", &["<Control>u"]),
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
    let quit_section = gio::Menu::new();
    item(&quit_section, tr!(MenuExit), "app.quit");
    file.append_section(None, &quit_section);
    menubar.append_submenu(Some(&gtk_mnemonic(tr!(MenuFile))), &file);
    let options = gio::Menu::new();
    item(&options, tr!(MenuPreferences), "win.preferences");
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

/// The torrent context menu (right click on the list).
fn torrent_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let open = gio::Menu::new();
    item(&open, tr!(MenuOpenFolder), "win.open-folder");
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
    let check = gio::Menu::new();
    item(&check, tr!(MenuRecheck), "win.recheck");
    item(&check, tr!(MenuUpdateTracker), "win.reannounce");
    menu.append_section(None, &check);
    let remove = gio::Menu::new();
    item(&remove, tr!(MenuRemove), "win.remove");
    item(&remove, tr!(MenuRemoveAndDelete), "win.remove-data");
    menu.append_section(None, &remove);
    menu
}

/// The main window, on a new engine session.
fn build(app: &gtk::Application) {
    let config = paths::config_dir();
    let data = paths::data_dir();
    let mut prefs = Prefs::load(config.join("patorrent.ini"));
    // (the language was picked in `run`)
    let downloads = glib::user_special_dir(glib::UserDirectory::Downloads)
        .unwrap_or_else(|| glib::home_dir().join("Downloads"))
        .to_string_lossy()
        .into_owned();
    let mut opts = Options::load(&prefs, &downloads);
    if opts.gui.random_port {
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

    let session =
        match Session::start(SessionParams { settings: opts.settings.clone(), state_dir: Some(data) }) {
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
            Some(if column == Column::Done {
                Content::Progress(f64::from(st.progress_ppm) / 1e6, text)
            } else {
                Content::Text(text)
            })
        })
        .flatten()
    });
    let menu = torrent_menu();
    let popup: crate::list::ContextMenu = Rc::new(move |widget, x, y| {
        if let Some(ui) = ui() {
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
    let sidebar = Sidebar::new(|filter| {
        with_app(|a| a.list.set_filter(filter));
        refresh();
    });
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
        None,
        Some(("edit-delete", "win.remove", tr!(ToolRemove))),
        None,
        Some(("media-playback-start", "win.start", tr!(ToolStart))),
        Some(("media-playback-pause", "win.pause", tr!(ToolPause))),
        Some(("media-playback-stop", "win.stop", tr!(ToolStop))),
        None,
        Some(("go-up", "win.queue-up", tr!(ToolQueueUp))),
        Some(("go-down", "win.queue-down", tr!(ToolQueueDown))),
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
    let split = gtk::Paned::new(gtk::Orientation::Vertical);
    split.set_start_child(Some(&list_scroll));
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
    list.view.connect_activate(|_, position| {
        if let Some(id) = ui().and_then(|ui| ui.list.model.id_at(position)) {
            open_folder(&[id]);
        }
    });
    list.selection.connect_selection_changed(|_, _, _| selection_changed());

    add_actions(&window, &list);
    // magnets whose dialog was still to come at the last exit
    let awaiting = add::AwaitingMetadata::load(&prefs);
    let ui = Rc::new(Ui { list, sidebar, status, paned, details, split });
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
        })
    });
    with_app(|a| {
        let sort = a.prefs.sort();
        a.list.set_sort(sort);
        if opts.gui.ip_filter {
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
        with_app(autoload);
        glib::ControlFlow::Continue
    });
    handle.post_torrent_updates();
    handle.post_session_stats();

    window.connect_close_request(|_| {
        shutdown();
        glib::Propagation::Proceed
    });
    update_status();
    window.present();
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
    app.handle.clear_alert_notify();
    if let Some(s) = app.session.take() {
        s.shutdown();
    }
    UI.with(|u| u.borrow_mut().take());
}

fn on_alerts() {
    let mut errors = Vec::new();
    let mut magnet_ready = false;
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
                    let key = info_hashes.key_hex();
                    a.prefs.remove(&format!("torrent.{key}.label"));
                    a.prefs.remove(&format!("torrent.{key}.paused"));
                }
                Alert::TorrentFinished { id } => {
                    let name = a.list.get(id).map(|st| st.name.clone()).unwrap_or_default();
                    a.detail.push_log(format!("{} {}", now_hms(), tr!(LogFinished, name)));
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
                Alert::PeerInfo { .. }
                | Alert::TrackerList { .. }
                | Alert::FileList { .. }
                | Alert::FileProgress { .. }
                | Alert::PieceInfo { .. }
                | Alert::TorrentFile { .. }
                | Alert::ResumeDataSaved => {}
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
                ui.sidebar.update(&a.list);
            }
        }
    });
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
    let Some(texts) = with_app(|a| {
        let s = &a.stats;
        [
            if s.has_incoming_connections { tr!(NetOk) } else { tr!(NetNoIncoming) }.to_owned(),
            tr!(BarDht, count = s.dht_nodes),
            tr!(BarDown, rate = fmt::speed_always(s.download_rate), total = fmt::size(s.recv_payload_bytes)),
            tr!(BarUp, rate = fmt::speed_always(s.upload_rate), total = fmt::size(s.sent_payload_bytes)),
        ]
    }) else {
        return;
    };
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
                    c.apply_to_torrent(&a.handle.torrent(id), &ti, &w.save_path, false);
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

/// Hands a torrent to the engine; a `.torrent`'s copy is stored once it is added.
fn add_to_session(a: &mut App, p: AddTorrentParams) {
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
    let Some((window, options)) = with_app(|a| (a.window.clone(), Options::load(&a.prefs, &a.save_path)))
    else {
        return;
    };
    let hooks = crate::preferences::Hooks {
        apply: Box::new(|o| {
            with_app(|a| apply_options(a, o));
        }),
        reload_ip_filter: Box::new(|path| {
            with_app(|a| {
                let r = a.handle.load_ip_filter(path).map_err(|e| e.to_string());
                log_ip_filter(a, path, &r);
                r
            })
            .unwrap_or_else(|| Err(tr!(ErrNotRunning).to_owned()))
        }),
        settings_dir: paths::config_dir(),
    };
    crate::preferences::show(&window, options, hooks);
}

/// Stores `o` and hands it to the engine and the window.
fn apply_options(a: &mut App, o: &Options) {
    let old = Options::load(&a.prefs, &a.save_path);
    o.store(&mut a.prefs);
    save_prefs(a);
    a.save_path = o.gui.download_dir.clone();
    a.handle.apply_settings(o.settings.clone());
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
    let dir = paths::config_dir();
    if (o.gui.ip_filter, o.gui.ip_filter_file(&dir)) != (old.gui.ip_filter, old.gui.ip_filter_file(&dir)) {
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
    let path = gui.ip_filter_file(&paths::config_dir());
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

fn about() {
    let Some(window) = with_app(|a| a.window.clone()) else { return };
    gtk::AboutDialog::builder()
        .transient_for(&window)
        .modal(true)
        .program_name("PaTorrent")
        .version(patorrent_core::VERSION)
        .comments(tr!(AboutShort))
        .website("https://github.com/Balls0fSteel/PaTorrent")
        .build()
        .present();
}
