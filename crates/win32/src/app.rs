//! The µTorrent 2.2.1 main window: menu, toolbar, category tree, torrent list, detail tabs and
//! status bar, on the real engine.
//!
//! Threading: the engine runs on its own thread and only ever posts `WM_APP_ALERTS` here. All
//! window state lives in a thread-local [`App`]. Controls send notifications synchronously, so
//! state is borrowed only for short stretches and never across a call that can notify back
//! (list view selection changes, tree selection, message boxes, menus).

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use patorrent_core::fetch::FetchProxy;
use patorrent_core::metainfo::TorrentInfo;
use patorrent_core::util::time::{local_utc_offset, unix_now};
use patorrent_core::{
    AddTorrentParams, Alert, IpFilter, MockConfig, MoveCompleted, MoveFlags, QueueMove, RemoveFlags, Session,
    SessionHandle, SessionParams, SessionStats, Settings, TorrentFlags, TorrentId, TorrentSource, same_dir,
};
use patorrent_frontend::auto_shutdown::{self, AutoShutdown, Mode as AutoMode, Step};
use patorrent_frontend::columns::Column;
use patorrent_frontend::detail::{self, PRIORITIES};
use patorrent_frontend::icons::{network_icon, toolbar_size, tracker_icon};
use patorrent_frontend::import;
use patorrent_frontend::lang::{self, S};
use patorrent_frontend::model::{Filter, TorrentList};
use patorrent_frontend::options::{self, GuiOptions, Options};
use patorrent_frontend::prefs::Prefs;
use patorrent_frontend::properties::{self, Properties, TorrentProps};
use patorrent_frontend::rss::view::{self as rss_view, ItemRow};
use patorrent_frontend::rss::{AddRequest, Download, FeedId, RssDownloader, Worker};
use patorrent_frontend::scheduler::{Mode, Schedule, Scheduler};
use patorrent_frontend::scrape::Scrapes;
use patorrent_frontend::seeding::{GoalAction, SeedGoal, SeedingGoal};
use patorrent_frontend::status::{Category, status_icon};
use patorrent_frontend::torrent_folders::{self, Poller, TorrentFiles, TorrentFolders};
use patorrent_frontend::update::{self, Checker, Kind, Manifest, Version};
use patorrent_frontend::{BUILD, NAME_BUILD, actions, add, create, fmt, settings, tr};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
use windows_sys::Win32::System::Time::SystemTimeToFileTime;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_DELETE, VK_ESCAPE, VK_RETURN,
};
use windows_sys::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, FOLDERID_Downloads, HDROP, SHGetKnownFolderPath,
    ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::{PCWSTR, w};

use crate::controls;
use crate::details::{Detail, Page, PaintCtx};
use crate::dialogs;
use crate::icons::{self, Icon};
use crate::integration::{self, Integration, Keys};
use crate::shutdown::{self, Outcome, Power};
use crate::tray;
use crate::util::{child, copy_to, exe_path, point_from_lparam, rtl, scale, send, unscale, wide};

#[path = "bench.rs"]
mod bench;

pub const WM_APP_ALERTS: u32 = WM_APP + 1;
const WM_APP_TRAY: u32 = WM_APP + 2;
/// A URL download finished; `lParam` is a `Box<UrlResult>`.
const WM_APP_URL: u32 = WM_APP + 3;
/// Arguments forwarded by a second instance are waiting in `App::pending_args`.
const WM_APP_ARGS: u32 = WM_APP + 4;
/// Auto Shutdown: the work is done, show the countdown; `wParam` = seconds.
const WM_APP_COUNTDOWN: u32 = WM_APP + 5;
/// An RSS fetch finished (posted from its worker thread): `App::rss_worker` has a result.
const WM_APP_RSS: u32 = WM_APP + 7;
/// A magnet added through the Add New Torrent dialog has its metadata: `App::awaiting` has it.
const WM_APP_MAGNET: u32 = WM_APP + 8;
/// An update check finished (posted from its worker thread); `lParam` is a
/// `Box<Result<Manifest, String>>`.
const WM_APP_UPDATE: u32 = WM_APP + 9;
/// `COPYDATASTRUCT::dwData` for arguments forwarded by a second instance.
pub const COPYDATA_ARGS: usize = 0x4c54_4152;

/// The arguments a second instance forwarded, or `None` for another message or no data. A
/// `WM_COPYDATA` with `cbData` 0 may carry a null `lpData` (SEC-25).
pub(crate) fn copydata_args(cds: &COPYDATASTRUCT) -> Option<Vec<String>> {
    if cds.dwData != COPYDATA_ARGS || cds.lpData.is_null() || cds.cbData < 2 || cds.lpData as usize % 2 != 0 {
        return None;
    }
    // SAFETY: the sender's cbData bytes at lpData, valid while the message is handled
    let words = unsafe { std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2) };
    Some(dialogs::split_nul(words))
}

/// Takes `WM_COPYDATA` only from processes of this window's integrity level or above, the
/// default that `MSGFLT_ALLOW` used to lift (SEC-25): forwarded sources may be added without a
/// dialog, so a sandboxed (low integrity) process must not send them, nor a standard one to a
/// PaTorrent running as administrator.
pub(crate) unsafe fn refuse_lower_integrity_copydata(hwnd: HWND) {
    unsafe { ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_DISALLOW, null_mut()) };
}

pub const MAIN_CLASS: PCWSTR = w!("PaTorrentMainWnd");
/// A remote window's class: a second local start looks for [`MAIN_CLASS`] only, so it never
/// hands its torrents to a remote window.
const REMOTE_CLASS: PCWSTR = w!("PaTorrentRemoteWnd");
const PANEL_CLASS: PCWSTR = w!("PaTorrentPanel");
const TIMER_REFRESH: usize = 1;

const IDC_TOOLBAR: usize = 10;
const IDC_TREE: usize = 11;
const IDC_LIST: usize = 12;
const IDC_TABS: usize = 13;
const IDC_STATUS: usize = 14;
/// The toolbar's search box.
const IDC_SEARCH: usize = 15;
const IDC_PAGE: usize = 20;
/// The pieces list inside the Pieces panel.
const IDC_PIECE_LIST: usize = 30;
/// Tree item parameters from here on are the Feeds branch (`App::feed_tree` index).
const FEED_PARAM: usize = 1_000_000;

mod cmd {
    pub const ADD: u16 = 100;
    pub const ADD_URL: u16 = 101;
    pub const CREATE: u16 = 102;
    pub const EXIT: u16 = 103;
    pub const PREFS: u16 = 104;
    pub const SHOW_TOOLBAR: u16 = 105;
    pub const SHOW_DETAILS: u16 = 106;
    pub const SHOW_STATUS: u16 = 107;
    pub const SHOW_TREE: u16 = 108;
    pub const ABOUT: u16 = 109;
    pub const REMOVE: u16 = 110;
    pub const REMOVE_DATA: u16 = 111;
    pub const START: u16 = 112;
    pub const FORCE_START: u16 = 113;
    pub const PAUSE: u16 = 114;
    pub const STOP: u16 = 115;
    pub const UP: u16 = 116;
    pub const DOWN: u16 = 117;
    pub const RECHECK: u16 = 118;
    pub const REANNOUNCE: u16 = 119;
    pub const OPEN_FOLDER: u16 = 120;
    pub const SELECT_ALL: u16 = 121;
    pub const TRAY_TOGGLE: u16 = 122;
    pub const START_ALL: u16 = 123;
    pub const STOP_ALL: u16 = 124;
    pub const LABEL_NEW: u16 = 125;
    pub const LABEL_NONE: u16 = 126;
    pub const CLOSE_TO_TRAY: u16 = 127;
    pub const MIN_TO_TRAY: u16 = 128;
    pub const QUEUE_TOP: u16 = 129;
    pub const QUEUE_BOTTOM: u16 = 130;
    pub const SUPER_SEED: u16 = 131;
    pub const SET_LOCATION: u16 = 132;
    pub const PROPERTIES: u16 = 133;
    pub const RSS: u16 = 134;
    pub const FEED_ADD: u16 = 135;
    pub const FEED_EDIT: u16 = 136;
    pub const FEED_TOGGLE: u16 = 137;
    pub const FEED_UPDATE: u16 = 138;
    pub const FEED_REMOVE: u16 = 139;
    pub const ITEM_DOWNLOAD: u16 = 140;
    pub const CONNECT_REMOTE: u16 = 141;
    pub const NOTICES: u16 = 142;
    pub const CHECK_UPDATES: u16 = 143;
    pub const WEBSITE: u16 = 144;
    /// Ctrl+F: to the search box.
    pub const FIND: u16 = 145;
    pub const LABEL_BASE: u16 = 1000;
    pub const DL_LIMIT_BASE: u16 = 2000;
    pub const UL_LIMIT_BASE: u16 = 2100;
    pub const COLUMN_BASE: u16 = 3000;
    pub const FILE_PRIO_BASE: u16 = 3100;
    /// Options > Auto Shutdown, by `auto_shutdown::AutoMode::MENU` index.
    pub const AUTO_SHUTDOWN_BASE: u16 = 3200;
    /// File > Import, by `import::Client::ALL` index.
    pub const IMPORT_BASE: u16 = 3300;
}

use patorrent_frontend::actions::LIMITS;

const TOOLBAR: [Option<(Icon, u16, S)>; 15] = [
    Some((Icon::AddTorrent, cmd::ADD, S::ToolAddTorrent)),
    Some((Icon::AddUrl, cmd::ADD_URL, S::ToolAddUrl)),
    Some((Icon::Create, cmd::CREATE, S::ToolCreate)),
    None,
    Some((Icon::Remove, cmd::REMOVE, S::ToolRemove)),
    None,
    Some((Icon::Start, cmd::START, S::ToolStart)),
    Some((Icon::Pause, cmd::PAUSE, S::ToolPause)),
    Some((Icon::Stop, cmd::STOP, S::ToolStop)),
    None,
    Some((Icon::Up, cmd::UP, S::ToolQueueUp)),
    Some((Icon::Down, cmd::DOWN, S::ToolQueueDown)),
    None,
    Some((Icon::Rss, cmd::RSS, S::ToolRss)),
    Some((Icon::Preferences, cmd::PREFS, S::ToolPreferences)),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Split {
    Tree,
    Details,
}

struct UrlResult {
    url: String,
    data: Result<UrlBody, String>,
}

/// What a URL gave: a `.torrent`'s contents, or the `magnet:` link it redirected to.
enum UrlBody {
    Torrent(Vec<u8>),
    Magnet(String),
}

struct App {
    hwnd: HWND,
    session: Option<Session>,
    handle: SessionHandle,
    list: TorrentList,
    stats: SessionStats,
    prefs: Prefs,
    save_path: String,
    dpi: u32,
    font: HFONT,
    bold: HFONT,
    icon_big: HICON,
    icon_small: HICON,
    images: (HIMAGELIST, HIMAGELIST),
    /// The state icons (the torrent list's Name column, the Trackers tab) and their size.
    states: (HIMAGELIST, usize),
    /// The status bar's network indicator: no incoming connection seen yet, one seen.
    net_icons: [HICON; 2],
    /// Which of them the status bar shows (`None`: not since the icons were made).
    net_shown: Option<bool>,

    toolbar: HWND,
    /// The search box at the toolbar's right end.
    search: HWND,
    tree: HWND,
    lv: HWND,
    tabs: HWND,
    status: HWND,
    pages: [HWND; 7],
    /// The Pieces panel's list of pieces being downloaded (a child of the panel).
    piece_list: HWND,

    show_toolbar: bool,
    show_details: bool,
    show_status: bool,
    show_tree: bool,
    /// At 96 DPI.
    tree_w: i32,
    detail_h: i32,
    split_tree: RECT,
    split_details: RECT,
    drag: Option<Split>,

    /// Every column in display order with its width at 96 DPI (negative = hidden).
    columns: Vec<(Column, i32)>,
    /// List view column index → column.
    shown: Vec<Column>,

    page: Page,
    detail: Detail,
    /// The torrent whose file list the Files tab has requested.
    files_requested: Option<TorrentId>,
    tree_items: Vec<(HTREEITEM, Filter)>,
    tree_texts: Vec<String>,
    tree_labels: Vec<String>,
    /// `list.generation()` when the tree counts were last computed.
    tree_generation: Option<u64>,
    /// Labels offered by the last context menu, by command offset.
    menu_labels: Vec<String>,

    known: HashSet<TorrentId>,
    /// Torrents seen incomplete in this session (for the completion balloon).
    downloading: HashSet<TorrentId>,
    alerts: Vec<Alert>,
    /// Selection is being restored after a re-sort; ignore the change notifications.
    restoring: bool,
    tray_tip: String,
    status_texts: Vec<String>,
    quitting: bool,
    taskbar_created: u32,
    /// Arguments from a second instance, added once its `WM_COPYDATA` has returned.
    pending_args: Vec<String>,
    /// Torrents (by `InfoHashes::key_hex`) to move to the top of the queue once added.
    queue_top: HashSet<String>,
    /// Magnets waiting for their metadata before the Add New Torrent dialog opens, and whether
    /// such a dialog is open now.
    awaiting: add::AwaitingMetadata,
    awaiting_dialog: bool,
    /// Preferences > Directories "Move completed downloads to", fed every alert.
    move_completed: Option<MoveCompleted>,
    /// Preferences > Scheduler, and the mode it has put into effect.
    schedule: Schedule,
    scheduler: Scheduler,
    /// Preferences > Directories "Location of .torrents", the helper that stores and moves the
    /// copies (fed every alert) and the auto-load folder's watcher (run from the timer).
    torrent_folders: TorrentFolders,
    torrent_files: TorrentFiles,
    autoload: Poller,
    /// Preferences > Queueing "Seeding Goal", checked on every status update.
    seeding_goal: SeedingGoal,
    /// The periodic scrape that gives stopped and queued torrents their swarm counts (run from
    /// the timer).
    scrapes: Scrapes,
    /// The RSS Downloader (`rss.dat` beside `patorrent.ini`), its fetches, whether it changed
    /// since it was saved, and its dialog while that is open.
    rss: RssDownloader,
    rss_worker: Worker,
    rss_dirty: bool,
    rss_dialog: HWND,
    /// The tree's Feeds branch: the Feeds node (`None`), then one node per feed.
    feed_tree: Vec<(HTREEITEM, Option<FeedId>)>,
    /// The Feeds branch is being rebuilt: its selection changes are not the user's.
    feed_tree_busy: bool,
    /// The list shows feed items instead of torrents (`Some(None)`: every feed's), as µTorrent
    /// did when a feed was picked in the tree; their rows.
    feed_view: Option<Option<FeedId>>,
    feed_rows: Vec<ItemRow>,
    /// Options > Auto Shutdown (this run only), checked on the timer.
    auto_shutdown: AutoShutdown,
    /// Standby or hibernation waiting for the engine's `ResumeDataSaved`.
    suspend_pending: Option<Power>,
    /// The update check (Preferences > General > Privacy, Help > Check for Updates) with the
    /// status bar's notice, and "Update to beta versions".
    updates: Checker,
    update_beta: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

/// Where settings and state live: [`base_data_dir`], or a remote window's own folder in it
/// (`remote_ui::data_dir`).
pub fn data_dir() -> PathBuf {
    match crate::remote_ui::get() {
        Some(r) => crate::remote_ui::data_dir(&base_data_dir(), &r.label),
        None => base_data_dir(),
    }
}

/// The settings folder: beside the exe when `patorrent.ini` is there (portable, like
/// µTorrent's `settings.dat`), else `%APPDATA%\PaTorrent`.
pub fn base_data_dir() -> PathBuf {
    let exe_dir = exe_path().parent().map(Path::to_path_buf).unwrap_or_default();
    if exe_dir.join("patorrent.ini").exists() {
        return exe_dir;
    }
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("PaTorrent")).unwrap_or(exe_dir)
}

fn downloads_folder() -> String {
    unsafe {
        let mut p: *mut u16 = null_mut();
        if SHGetKnownFolderPath(&FOLDERID_Downloads, 0, null_mut(), &mut p) == 0 && !p.is_null() {
            let s = crate::util::from_wide_ptr(p);
            windows_sys::Win32::System::Com::CoTaskMemFree(p as *const c_void);
            return s;
        }
    }
    std::env::var("USERPROFILE").map(|h| format!("{h}\\Downloads")).unwrap_or_else(|_| ".".into())
}

/// Seconds east of UTC, from the difference between local and UTC wall-clock time.
fn utc_offset() -> i32 {
    unsafe {
        let (mut st_l, mut st_u) = (zeroed(), zeroed());
        GetLocalTime(&mut st_l);
        GetSystemTime(&mut st_u);
        let (mut fl, mut fu) = (zeroed(), zeroed());
        SystemTimeToFileTime(&st_l, &mut fl);
        SystemTimeToFileTime(&st_u, &mut fu);
        let t = |f: windows_sys::Win32::Foundation::FILETIME| {
            ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64
        };
        // 100 ns units; round to whole minutes.
        (((t(fl) - t(fu)) / 10_000_000 + 30) / 60 * 60) as i32
    }
}

fn now_hms() -> String {
    let st = unsafe {
        let mut st = zeroed();
        GetLocalTime(&mut st);
        st
    };
    format!("[{:02}:{:02}:{:02}]", st.wHour, st.wMinute, st.wSecond)
}

pub struct Args {
    pub demo: Option<usize>,
    /// Drive a patorrent-nox elsewhere (`remote_ui`) instead of an engine of our own.
    pub remote: bool,
    /// Start in the tray, without showing the window.
    pub minimized: bool,
    pub sources: Vec<String>,
}

pub fn parse_args(args: &[String]) -> Args {
    let mut out = Args { demo: None, remote: false, minimized: false, sources: Vec::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--demo" => out.demo = it.next().and_then(|n| n.parse().ok()),
            "--remote" => out.remote = true,
            // the shell's one argument; what follows is a link split at a raw `"` (SEC-18)
            integration::SHELL => {
                out.sources.extend(it.next().cloned());
                break;
            }
            s if s.eq_ignore_ascii_case(integration::MINIMIZED) => out.minimized = true,
            s if s.starts_with("--") => {}
            s => out.sources.push(s.to_owned()),
        }
    }
    out
}

/// Picks the language from `patorrent.ini` in [`base_data_dir`] (one language for every
/// window, remote ones included); what went wrong, for the Logger.
fn init_language() -> Vec<String> {
    let problems = lang::init(&language_choice());
    if lang::right_to_left() {
        // every window created from here on is mirrored, its controls with it
        unsafe { SetProcessDefaultLayout(LAYOUT_RTL) };
    }
    problems
}

/// Preferences > General's language (`ui.language`), which lives in [`base_data_dir`]'s
/// `patorrent.ini` even for a remote window (whose other options are in its own folder).
fn language_choice() -> String {
    Prefs::load(base_data_dir().join("patorrent.ini")).get("ui.language").unwrap_or_default().to_owned()
}

pub fn run(args: Args) -> i32 {
    // before any window or message box
    let lang_problems = init_language();
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES
                | ICC_BAR_CLASSES
                | ICC_TAB_CLASSES
                | ICC_LISTVIEW_CLASSES
                | ICC_TREEVIEW_CLASSES,
        };
        InitCommonControlsEx(&icc);
    }
    fmt::set_utc_offset(utc_offset());
    let remote_session = match args.remote {
        true => match crate::remote_ui::connect_interactive() {
            Some(s) => Some(s),
            None => return 0,
        },
        false => None,
    };
    let local = args.demo.is_none() && remote_session.is_none();
    let dir = data_dir();
    let mut prefs = Prefs::load(dir.join("patorrent.ini"));
    if let Some(r) = crate::remote_ui::get() {
        // the server's engine settings and default folder are this window's
        let _ = std::fs::create_dir_all(&dir);
        settings::store(&mut prefs, &r.hello.settings);
        prefs.set("dir.download", &r.hello.save_path);
        // Preferences shows the language every window has (`language_choice`)
        match language_choice().as_str() {
            "" => prefs.remove("ui.language"),
            code => prefs.set("ui.language", code),
        }
    }
    let mut opts = Options::load(&prefs, &downloads_folder());
    controls::set_alternate_rows(opts.gui.alternate_rows);
    if opts.gui.random_port && local {
        // µTorrent's "Randomize port each start" keeps the new port as the configured one
        settings::set_listen_port(&mut opts.settings, options::random_port());
        settings::store(&mut prefs, &opts.settings);
    }

    let session = match (remote_session, args.demo) {
        (Some(s), _) => s,
        (None, Some(n)) => Session::start_mock(
            Settings::default(),
            MockConfig { initial_torrents: n, active_fraction: 0.02, ..MockConfig::default() },
        ),
        (None, None) => {
            let settings = opts.settings.clone();
            match Session::start(SessionParams { settings, state_dir: Some(dir.join("state")) }) {
                Ok(s) => s,
                Err(e) => {
                    dialogs::error(null_mut(), &tr!(ErrEngineStart, error = e));
                    return 1;
                }
            }
        }
    };

    unsafe {
        let instance = GetModuleHandleW(null());
        let mut wc: WNDCLASSEXW = zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = instance;
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.hbrBackground = (COLOR_BTNFACE + 1) as usize as HBRUSH;
        let class = if crate::remote_ui::is_remote() { REMOTE_CLASS } else { MAIN_CLASS };
        wc.lpszClassName = class;
        RegisterClassExW(&wc);
        wc.lpfnWndProc = Some(panel_proc);
        wc.hbrBackground = null_mut();
        wc.style = CS_HREDRAW | CS_VREDRAW;
        wc.lpszClassName = PANEL_CLASS;
        RegisterClassExW(&wc);

        let title = wide(NAME_BUILD);
        let hwnd = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            class,
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            null_mut(),
            main_menu(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return 1;
        }
        init(hwnd, session, prefs);
        for p in lang_problems {
            log_line(&p);
        }
        if let Some(r) = crate::remote_ui::get() {
            let title = wide(&tr!(TitleRemote, version = BUILD, label = r.label));
            SetWindowTextW(hwnd, title.as_ptr());
        }
        // the server applies its own block list
        if opts.gui.ip_filter && local {
            with_app(|a| load_ip_filter(a, &opts.gui));
        }
        restore_placement(hwnd, args.minimized);
        if local {
            // an association from before `--shell` (SEC-18); failing leaves it as it was
            let _ = integration::upgrade(&Keys::user(), &exe_path());
        }
        // not at a login's minimized start, where nobody asked for the window
        if opts.gui.check_association && local && !args.minimized {
            check_association(hwnd);
        }
        add_sources(hwnd, &args.sources);

        let accels = [
            ACCEL { fVirt: FCONTROL | FVIRTKEY, key: b'O' as u16, cmd: cmd::ADD },
            ACCEL { fVirt: FCONTROL | FVIRTKEY, key: b'U' as u16, cmd: cmd::ADD_URL },
            ACCEL { fVirt: FCONTROL | FVIRTKEY, key: b'N' as u16, cmd: cmd::CREATE },
            ACCEL { fVirt: FCONTROL | FVIRTKEY, key: b'P' as u16, cmd: cmd::PREFS },
            ACCEL { fVirt: FCONTROL | FVIRTKEY, key: b'A' as u16, cmd: cmd::SELECT_ALL },
            ACCEL { fVirt: FCONTROL | FVIRTKEY, key: b'F' as u16, cmd: cmd::FIND },
            ACCEL { fVirt: FVIRTKEY, key: VK_DELETE, cmd: cmd::REMOVE },
            ACCEL { fVirt: FSHIFT | FVIRTKEY, key: VK_DELETE, cmd: cmd::REMOVE_DATA },
            ACCEL { fVirt: FCONTROL | FVIRTKEY, key: b'S' as u16, cmd: cmd::START },
            ACCEL { fVirt: FCONTROL | FVIRTKEY, key: b'T' as u16, cmd: cmd::STOP },
            ACCEL { fVirt: FCONTROL | FSHIFT | FVIRTKEY, key: b'P' as u16, cmd: cmd::PAUSE },
            // Properties; only from the torrent list, see the loop
            ACCEL { fVirt: FVIRTKEY, key: VK_RETURN, cmd: cmd::PROPERTIES },
        ];
        let haccel = CreateAcceleratorTableW(accels.as_ptr(), accels.len() as i32);
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if search_key(&msg) {
                continue;
            }
            // typing in the search box: Delete and Ctrl+A edit its text, not the list; Enter
            // is the list's Properties key, every other control keeps its own Enter
            let (in_search, in_list) =
                with_app(|a| (a.search == msg.hwnd, a.lv == msg.hwnd)).unwrap_or((false, false));
            let enter = msg.message == WM_KEYDOWN && msg.wParam as u16 == VK_RETURN;
            if in_search || (enter && !in_list) || TranslateAcceleratorW(hwnd, haccel, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        DestroyAcceleratorTable(haccel);
    }
    // Auto Shutdown's Shut Down, once the session has saved its state and stopped
    if let Some(Err(e)) = shutdown::take_after_exit().map(shutdown::perform) {
        dialogs::error(null_mut(), &e);
    }
    0
}

unsafe fn main_menu() -> HMENU {
    unsafe {
        let bar = CreateMenu();
        let file = CreatePopupMenu();
        append(file, cmd::ADD, &shortcut(tr!(MenuAddTorrent), Hotkey::Ctrl('O')));
        append(file, cmd::ADD_URL, &shortcut(tr!(MenuAddUrl), Hotkey::Ctrl('U')));
        append(file, cmd::CREATE, &shortcut(tr!(MenuCreate), Hotkey::Ctrl('N')));
        separator(file);
        // a remote server would read its own disk, not this computer's
        let remote = crate::remote_ui::is_remote();
        for (i, c) in import::Client::ALL.iter().enumerate() {
            append_flags(file, grayed(remote), cmd::IMPORT_BASE + i as u16, c.menu_text());
        }
        separator(file);
        append(file, cmd::CONNECT_REMOTE, tr!(MenuConnectRemote));
        separator(file);
        append(file, cmd::EXIT, tr!(MenuExit));
        let options = CreatePopupMenu();
        append(options, cmd::PREFS, &shortcut(tr!(MenuPreferences), Hotkey::Ctrl('P')));
        append(options, cmd::RSS, tr!(MenuRss));
        separator(options);
        append(options, cmd::SHOW_TOOLBAR, tr!(MenuShowToolbar));
        append(options, cmd::SHOW_DETAILS, tr!(MenuShowDetails));
        append(options, cmd::SHOW_STATUS, tr!(MenuShowStatus));
        append(options, cmd::SHOW_TREE, tr!(MenuShowTree));
        separator(options);
        let auto = CreatePopupMenu();
        for (i, &m) in AutoMode::MENU.iter().enumerate() {
            let off = m.is_some_and(|m| !shutdown::available(m.action));
            append_flags(auto, grayed(off), cmd::AUTO_SHUTDOWN_BASE + i as u16, AutoMode::label(m));
            // Disabled | the client | the computer
            if i == 0 || i == 2 {
                separator(auto);
            }
        }
        submenu(options, auto, tr!(MenuAutoShutdown));
        separator(options);
        append(options, cmd::CLOSE_TO_TRAY, tr!(MenuCloseToTray));
        append(options, cmd::MIN_TO_TRAY, tr!(MenuMinimizeToTray));
        let help = CreatePopupMenu();
        append(help, cmd::WEBSITE, tr!(MenuWebsite));
        if update::AVAILABLE {
            append(help, cmd::CHECK_UPDATES, tr!(MenuCheckUpdates));
        }
        separator(help);
        append(help, cmd::ABOUT, tr!(MenuAbout));
        append(help, cmd::NOTICES, tr!(MenuNotices));
        submenu(bar, file, tr!(MenuFile));
        submenu(bar, options, tr!(MenuOptions));
        submenu(bar, help, tr!(MenuHelp));
        bar
    }
}

/// A keyboard shortcut as the menus write it, with the key names in the current language.
#[derive(Clone, Copy)]
enum Hotkey {
    Ctrl(char),
    CtrlShift(char),
    Del,
    ShiftDel,
    Enter,
}

impl Hotkey {
    fn label(self) -> String {
        match self {
            Hotkey::Ctrl(k) => format!("{}+{k}", tr!(KeyCtrl)),
            Hotkey::CtrlShift(k) => format!("{}+{}+{k}", tr!(KeyCtrl), tr!(KeyShift)),
            Hotkey::Del => tr!(KeyDel).to_owned(),
            Hotkey::ShiftDel => format!("{}+{}", tr!(KeyShift), tr!(KeyDel)),
            Hotkey::Enter => tr!(KeyEnter).to_owned(),
        }
    }
}

/// A menu item's text with its shortcut after a tab.
fn shortcut(text: &str, keys: Hotkey) -> String {
    format!("{text}\t{}", keys.label())
}

unsafe fn append(menu: HMENU, id: u16, text: &str) {
    unsafe { append_flags(menu, 0, id, text) }
}

unsafe fn append_flags(menu: HMENU, flags: u32, id: u16, text: &str) {
    let t = wide(text);
    unsafe { AppendMenuW(menu, MF_STRING | flags, id as usize, t.as_ptr()) };
}

unsafe fn separator(menu: HMENU) {
    unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, null()) };
}

unsafe fn submenu(menu: HMENU, sub: HMENU, text: &str) {
    let t = wide(text);
    unsafe { AppendMenuW(menu, MF_POPUP, sub as usize, t.as_ptr()) };
}

fn checked(on: bool) -> u32 {
    if on { MF_CHECKED } else { 0 }
}

fn grayed(off: bool) -> u32 {
    if off { MF_GRAYED } else { 0 }
}

unsafe fn init(hwnd: HWND, session: Session, prefs: Prefs) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        let toolbar = child(
            hwnd,
            TOOLBARCLASSNAMEW,
            "",
            WS_CHILD
                | WS_CLIPSIBLINGS
                | TBSTYLE_FLAT
                | TBSTYLE_TOOLTIPS
                | CCS_NODIVIDER as u32
                | CCS_TOP as u32,
            0,
            IDC_TOOLBAR,
        );
        send(toolbar, TB_BUTTONSTRUCTSIZE, size_of::<TBBUTTON>(), 0);
        send(toolbar, TB_SETMAXTEXTROWS, 0, 0);
        let buttons: Vec<TBBUTTON> = TOOLBAR
            .iter()
            .map(|b| {
                let mut t: TBBUTTON = zeroed();
                match b {
                    Some((icon, id, text)) => {
                        t.iBitmap = Icon::ALL.iter().position(|i| i == icon).unwrap_or(0) as i32;
                        t.idCommand = *id as i32;
                        t.fsState = TBSTATE_ENABLED as u8;
                        t.fsStyle = BTNS_BUTTON as u8;
                        // The toolbar keeps the pointer: leak the few strings once.
                        t.iString = Box::leak(wide(lang::text(*text)).into_boxed_slice()).as_ptr() as isize;
                    }
                    None => t.fsStyle = BTNS_SEP as u8,
                }
                t
            })
            .collect();
        send(toolbar, TB_ADDBUTTONSW, buttons.len(), buttons.as_ptr() as LPARAM);
        // where 2.2.1 had its web search box: here it filters the list by name
        let search = child(
            hwnd,
            w!("EDIT"),
            "",
            WS_CHILD | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            IDC_SEARCH,
        );
        let cue = wide(tr!(SearchCue));
        send(search, EM_SETCUEBANNER, 0, cue.as_ptr() as LPARAM);

        let tree = child(
            hwnd,
            WC_TREEVIEWW,
            "",
            WS_CHILD
                | WS_TABSTOP
                | WS_CLIPSIBLINGS
                | TVS_SHOWSELALWAYS
                | TVS_HASBUTTONS
                | TVS_LINESATROOT
                | TVS_FULLROWSELECT,
            WS_EX_CLIENTEDGE,
            IDC_TREE,
        );
        let lv = controls::create_report(hwnd, IDC_LIST, true);
        // the state icon goes in the Name column wherever it is
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, LVS_EX_SUBITEMIMAGES as usize, LVS_EX_SUBITEMIMAGES as isize);
        let tabs = child(hwnd, WC_TABCONTROLW, "", WS_CHILD | WS_CLIPSIBLINGS | WS_TABSTOP, 0, IDC_TABS);
        let mut pages = [null_mut(); 7];
        for page in Page::ALL {
            let i = page.index();
            let mut text = wide(lang::text(page.title()));
            let mut item: TCITEMW = zeroed();
            item.mask = TCIF_TEXT;
            item.pszText = text.as_mut_ptr();
            send(tabs, TCM_INSERTITEMW, i, &item as *const _ as LPARAM);
            pages[i] = if page.is_list() {
                controls::create_report(hwnd, IDC_PAGE + i, false)
            } else {
                let p = child(
                    hwnd,
                    PANEL_CLASS,
                    "",
                    WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                    0,
                    IDC_PAGE + i,
                );
                SetWindowLongPtrW(p, GWLP_USERDATA, i as isize);
                p
            };
        }
        let piece_list = controls::create_report(pages[Page::Pieces.index()], IDC_PIECE_LIST, false);
        ShowWindow(piece_list, SW_SHOW);
        let status =
            child(hwnd, STATUSCLASSNAMEW, "", WS_CHILD | SBARS_SIZEGRIP | SBARS_TOOLTIPS, 0, IDC_STATUS);

        let handle = session.handle();
        let target = hwnd as isize;
        // Runs on the engine thread: only post a message.
        handle.set_alert_notify(move || {
            PostMessageW(target as HWND, WM_APP_ALERTS, 0, 0);
        });

        let save_path = prefs.get("dir.download").map(str::to_owned).unwrap_or_else(downloads_folder);
        let gui = GuiOptions::load(&prefs, &save_path);
        let move_completed = gui.move_completed();
        let seeding_goal = gui.seeding_goal();
        let schedule = Schedule::load(&prefs);
        let awaiting = add::AwaitingMetadata::load(&prefs);
        let mut list = TorrentList::new();
        list.set_sort(prefs.sort());
        let page =
            Page::ALL.into_iter().find(|p| prefs.get("ui.tab") == Some(p.key())).unwrap_or(Page::General);
        let rss = RssDownloader::load(&rss_file());
        // Runs on a fetch's worker thread: only post a message.
        let rss_worker = Worker::new(move || {
            PostMessageW(target as HWND, WM_APP_RSS, 0, 0);
        });
        let icon_big = icons::app_icon(scale(32, dpi) as usize);
        let icon_small = icons::app_icon(scale(16, dpi) as usize);
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon_big as LPARAM);
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon_small as LPARAM);

        let app = App {
            hwnd,
            session: Some(session),
            handle,
            list,
            stats: SessionStats::default(),
            show_toolbar: prefs.get_bool("ui.toolbar", true),
            show_details: prefs.get_bool("ui.details", true),
            show_status: prefs.get_bool("ui.status", true),
            show_tree: prefs.get_bool("ui.tree", true),
            tree_w: prefs.get_i32("ui.tree_width").unwrap_or(150),
            detail_h: prefs.get_i32("ui.detail_height").unwrap_or(230),
            columns: prefs.columns(),
            prefs,
            save_path,
            dpi,
            font: null_mut(),
            bold: null_mut(),
            icon_big,
            icon_small,
            images: (0, 0),
            states: (0, 0),
            net_icons: [null_mut(); 2],
            net_shown: None,
            toolbar,
            search,
            tree,
            lv,
            tabs,
            status,
            pages,
            piece_list,
            split_tree: zeroed(),
            split_details: zeroed(),
            drag: None,
            shown: Vec::new(),
            page,
            detail: Detail::default(),
            files_requested: None,
            tree_items: Vec::new(),
            tree_texts: Vec::new(),
            tree_labels: Vec::new(),
            tree_generation: None,
            menu_labels: Vec::new(),
            known: HashSet::new(),
            downloading: HashSet::new(),
            alerts: Vec::new(),
            restoring: false,
            tray_tip: String::new(),
            status_texts: Vec::new(),
            quitting: false,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            pending_args: Vec::new(),
            queue_top: HashSet::new(),
            awaiting,
            awaiting_dialog: false,
            move_completed,
            schedule,
            scheduler: Scheduler::new(),
            torrent_folders: gui.torrent_folders,
            torrent_files: TorrentFiles::new(),
            autoload: Poller::new(),
            seeding_goal,
            scrapes: Scrapes::default(),
            rss,
            rss_worker,
            rss_dirty: false,
            rss_dialog: null_mut(),
            feed_tree: Vec::new(),
            feed_tree_busy: false,
            feed_view: None,
            feed_rows: Vec::new(),
            auto_shutdown: AutoShutdown::default(),
            suspend_pending: None,
            updates: Checker::new(gui.check_updates, std::time::Instant::now()),
            update_beta: gui.update_beta,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));

        apply_dpi(hwnd, dpi);
        build_tree();
        for page in Page::ALL {
            if page.is_list() {
                let cols: Vec<(&str, i32, bool)> =
                    page.columns().iter().map(|&(t, w, r)| (lang::text(t), scale(w, dpi), r)).collect();
                controls::set_columns(pages[page.index()], &cols);
            }
        }
        let cols: Vec<(&str, i32, bool)> =
            detail::PIECE_COLUMNS.iter().map(|&(t, w, r)| (lang::text(t), scale(w, dpi), r)).collect();
        controls::set_columns(piece_list, &cols);
        send(tabs, TCM_SETCURSEL, page.index(), 0);
        DragAcceptFiles(hwnd, 1);
        tray::add(hwnd, icon_small, WM_APP_TRAY, NAME_BUILD);
        refuse_lower_integrity_copydata(hwnd);
        update_visibility(hwnd);
        update_menu(hwnd);
        update_toolbar();
        SetTimer(hwnd, TIMER_REFRESH, 1000, None);
        // Alerts queued before the notify callback was set never trigger it (it fires on
        // empty → non-empty only), so drain once now.
        PostMessageW(hwnd, WM_APP_ALERTS, 0, 0);
        on_timer();
        bench::start(hwnd);
    }
}

/// Preferences > General "Check association on startup": asks to make PaTorrent the program
/// for `.torrent` files and `magnet:` links when it is not.
fn check_association(hwnd: HWND) {
    let (keys, exe) = (Keys::user(), exe_path());
    let status = integration::read(&keys, &exe);
    if status.opens_torrents() && status.opens_magnets() {
        return;
    }
    if !dialogs::confirm(hwnd, tr!(AskAssociate)) {
        return;
    }
    let want = Integration { torrent: true, magnet: true, ..status.set };
    match integration::write(&keys, &exe, &status.set, &want) {
        Ok(after) if !(after.opens_torrents() && after.opens_magnets()) => {
            if dialogs::confirm(hwnd, &tr!(AskDefaultApps, note = after.note())) {
                integration::open_default_apps(hwnd);
            }
        }
        Ok(_) => {}
        Err(e) => dialogs::error(hwnd, &tr!(ErrAssociate, error = e)),
    }
}

/// Places the window where it was; `hidden` leaves it in the tray.
unsafe fn restore_placement(hwnd: HWND, hidden: bool) {
    unsafe {
        let (rect, max) = with_app(|a| {
            let r = a
                .prefs
                .get("window.rect")
                .map(|s| s.split(',').filter_map(|v| v.trim().parse::<i32>().ok()).collect::<Vec<_>>());
            (r, a.prefs.get_bool("window.maximized", false))
        })
        .unwrap_or((None, false));
        let dpi = GetDpiForWindow(hwnd);
        let mut wp: WINDOWPLACEMENT = zeroed();
        wp.length = size_of::<WINDOWPLACEMENT>() as u32;
        GetWindowPlacement(hwnd, &mut wp);
        match rect.as_deref() {
            Some(&[l, t, r, b]) if r - l > 200 && b - t > 150 => {
                wp.rcNormalPosition = RECT { left: l, top: t, right: r, bottom: b };
            }
            _ => {
                let r = &mut wp.rcNormalPosition;
                r.right = r.left + scale(900, dpi);
                r.bottom = r.top + scale(620, dpi);
            }
        }
        if hidden {
            wp.showCmd = SW_HIDE as u32;
            SetWindowPlacement(hwnd, &wp);
            return;
        }
        wp.showCmd = if max { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL } as u32;
        SetWindowPlacement(hwnd, &wp);
        ShowWindow(hwnd, if max { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL });
    }
}

unsafe fn save_placement(app: &mut App) {
    unsafe {
        let mut wp: WINDOWPLACEMENT = zeroed();
        wp.length = size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(app.hwnd, &mut wp) != 0 {
            let r = wp.rcNormalPosition;
            app.prefs.set("window.rect", format!("{},{},{},{}", r.left, r.top, r.right, r.bottom));
            app.prefs.set_bool("window.maximized", wp.showCmd == SW_SHOWMAXIMIZED as u32);
        }
    }
}

/// Fonts, toolbar images and column widths for `dpi`.
unsafe fn apply_dpi(hwnd: HWND, dpi: u32) {
    unsafe {
        let mut ncm: NONCLIENTMETRICSW = zeroed();
        ncm.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            &mut ncm as *mut _ as *mut c_void,
            0,
            dpi,
        );
        let font = CreateFontIndirectW(&ncm.lfMessageFont);
        let mut lf = ncm.lfMessageFont;
        lf.lfWeight = FW_BOLD as i32;
        let bold = CreateFontIndirectW(&lf);
        // 24 scaled, down to the nearest size the artwork is exported at (24, 24, 32, 48 px
        // at 100, 125, 150, 200 %)
        let size = toolbar_size(scale(24, dpi).max(0) as u32) as usize;
        let images = icons::toolbar_images(size);
        let state_px = scale(16, dpi) as usize;
        let states = icons::state_images(state_px);
        let net_icons = [false, true].map(|on| icons::state_icon(network_icon(on), state_px));
        let Some((old, old_states, controls_list, toolbar, trackers)) = with_app(|a| {
            // Keep column widths in 96-DPI units across the change.
            sync_columns(a);
            a.dpi = dpi;
            let old = (
                std::mem::replace(&mut a.font, font),
                std::mem::replace(&mut a.bold, bold),
                std::mem::replace(&mut a.images, images),
            );
            let old_states =
                (std::mem::replace(&mut a.states, states).0, std::mem::replace(&mut a.net_icons, net_icons));
            a.net_shown = None;
            let mut list = vec![a.tree, a.lv, a.tabs, a.status, a.search];
            list.extend(a.pages);
            (old, old_states, list, a.toolbar, a.pages[Page::Trackers.index()])
        }) else {
            return;
        };
        for c in controls_list {
            send(c, WM_SETFONT, font as WPARAM, 0);
        }
        send(toolbar, TB_SETIMAGELIST, 0, images.0);
        send(toolbar, TB_SETDISABLEDIMAGELIST, 0, images.1);
        send(toolbar, TB_SETBITMAPSIZE, 0, ((size << 16) | size) as LPARAM);
        send(toolbar, TB_AUTOSIZE, 0, 0);
        let (old_font, old_bold, old_images) = old;
        for f in [old_font, old_bold] {
            if !f.is_null() {
                DeleteObject(f);
            }
        }
        if old_images.0 != 0 {
            ImageList_Destroy(old_images.0);
            ImageList_Destroy(old_images.1);
        }
        // the lists share the state icons (LVS_SHAREIMAGELISTS): the old ones go once replaced
        send(trackers, LVM_SETIMAGELIST, LVSIL_SMALL as usize, states.0);
        rebuild_columns();
        update_status_bar();
        let (old_states, old_net) = old_states;
        if old_states != 0 {
            ImageList_Destroy(old_states);
        }
        for icon in old_net.into_iter().filter(|i| !i.is_null()) {
            DestroyIcon(icon);
        }
        // Detail list columns scale too.
        let (pages, piece_list) = with_app(|a| (a.pages, a.piece_list)).unwrap_or_default();
        for page in Page::ALL {
            if page.is_list() {
                let lv = pages[page.index()];
                let n = page.columns().len();
                for (i, (_, w)) in controls::column_layout(lv, n).into_iter().enumerate() {
                    let _ = w;
                    let base = page.columns()[i].1;
                    send(lv, LVM_SETCOLUMNWIDTH, i, scale(base, dpi) as isize);
                }
            }
        }
        for (i, &(_, base, _)) in detail::PIECE_COLUMNS.iter().enumerate() {
            send(piece_list, LVM_SETCOLUMNWIDTH, i, scale(base, dpi) as isize);
        }
        layout(hwnd);
    }
}

/// Reads column order and widths back from the list view into `app.columns`.
fn sync_columns(app: &mut App) {
    // while the list shows feed items its columns are those of the items
    if app.shown.is_empty() || app.feed_view.is_some() {
        return;
    }
    let layout = unsafe { controls::column_layout(app.lv, app.shown.len()) };
    let mut displayed = layout.into_iter().map(|(i, w)| (app.shown[i], unscale(w, app.dpi).max(16)));
    for slot in app.columns.iter_mut() {
        if slot.1 > 0 {
            if let Some(next) = displayed.next() {
                *slot = next;
            }
        }
    }
}

unsafe fn rebuild_columns() {
    // the feed items have no state icons
    if let Some((lv, images)) = with_app(|a| (a.lv, if a.feed_view.is_some() { 0 } else { a.states.0 })) {
        unsafe { send(lv, LVM_SETIMAGELIST, LVSIL_SMALL as usize, images) };
    }
    if with_app(|a| a.feed_view.is_some()) == Some(true) {
        return unsafe { set_feed_columns() };
    }
    let Some((lv, cols, sort, n)) = with_app(|a| {
        a.shown = a.columns.iter().filter(|(_, w)| *w > 0).map(|(c, _)| *c).collect();
        let cols: Vec<(&'static str, i32, bool)> = a
            .columns
            .iter()
            .filter(|(_, w)| *w > 0)
            .map(|&(c, w)| {
                (c.title(), scale(w, a.dpi), c.align() == patorrent_frontend::columns::Align::Right)
            })
            .collect();
        let sort = a.list.sort().and_then(|(c, d)| Some((a.shown.iter().position(|&s| s == c)?, d)));
        (a.lv, cols, sort, a.list.view().len())
    }) else {
        return;
    };
    unsafe {
        controls::set_columns(lv, &cols);
        controls::set_sort_arrow(lv, cols.len(), sort.map(|s| s.0), sort.is_some_and(|s| s.1));
        controls::set_count(lv, n);
        InvalidateRect(lv, null(), 1);
    }
}

unsafe fn layout(hwnd: HWND) {
    unsafe {
        let Some(a) = with_app(|a| {
            (
                a.toolbar,
                a.tree,
                a.lv,
                a.tabs,
                a.status,
                a.pages,
                a.page,
                a.show_toolbar,
                a.show_details,
                a.show_status,
                a.show_tree,
                scale(a.tree_w, a.dpi),
                scale(a.detail_h, a.dpi),
                a.dpi,
                a.updates.notice().map(|v| tr!(UpdateNotice, version = v)),
                a.search,
            )
        }) else {
            return;
        };
        let (
            toolbar,
            tree,
            lv,
            tabs,
            status,
            pages,
            page,
            show_tb,
            show_det,
            show_st,
            show_tree,
            tree_w,
            detail_h,
            dpi,
            notice,
            search,
        ) = a;
        let mut rc: RECT = zeroed();
        GetClientRect(hwnd, &mut rc);
        let gap = scale(4, dpi);
        let mut top = rc.top;
        let mut bottom = rc.bottom;
        if show_tb {
            send(toolbar, TB_AUTOSIZE, 0, 0);
            let mut r: RECT = zeroed();
            GetWindowRect(toolbar, &mut r);
            let tb_h = r.bottom - r.top;
            // the search box over the toolbar's right end, centred in its height
            let (w, h) = (scale(200, dpi), scale(22, dpi).min(tb_h));
            SetWindowPos(search, HWND_TOP, rc.right - w - gap, top + (tb_h - h) / 2, w, h, 0);
            top += tb_h;
        }
        if show_st {
            send(status, WM_SIZE, 0, 0);
            let mut r: RECT = zeroed();
            GetWindowRect(status, &mut r);
            bottom -= r.bottom - r.top;
            let w = rc.right;
            let mut parts = vec![w - scale(560, dpi), w - scale(400, dpi), w - scale(200, dpi), -1];
            if let Some(text) = notice {
                // the update notice first, as wide as its icon and text, out of the network
                // part's room: for its minute that part may shrink to its icon (a narrow
                // window shows the rest of the notice as its tooltip)
                let want = text_width(status, &text) + scale(36, dpi);
                parts.insert(0, want.min(parts[0] - scale(28, dpi)).max(0));
            }
            send(status, SB_SETPARTS, parts.len(), parts.as_ptr() as LPARAM);
        }
        top += gap / 2;
        let mut split_details: RECT = zeroed();
        if show_det {
            let h = detail_h.clamp(scale(60, dpi), ((bottom - top) - scale(80, dpi)).max(scale(60, dpi)));
            let dtop = bottom - h;
            MoveWindow(tabs, rc.left, dtop, rc.right - rc.left, h, 1);
            let mut area = RECT { left: rc.left, top: dtop, right: rc.right, bottom };
            send(tabs, TCM_ADJUSTRECT, 0, &mut area as *mut _ as LPARAM);
            for p in Page::ALL {
                let w = pages[p.index()];
                if p == page {
                    SetWindowPos(
                        w,
                        HWND_TOP,
                        area.left,
                        area.top,
                        area.right - area.left,
                        area.bottom - area.top,
                        SWP_SHOWWINDOW,
                    );
                } else {
                    ShowWindow(w, SW_HIDE);
                }
            }
            split_details = RECT { left: rc.left, top: dtop - gap, right: rc.right, bottom: dtop };
            bottom = dtop - gap;
        }
        let mut split_tree: RECT = zeroed();
        let mut left = rc.left;
        if show_tree {
            let w = tree_w.clamp(scale(60, dpi), (rc.right / 2).max(scale(60, dpi)));
            MoveWindow(tree, rc.left, top, w, bottom - top, 1);
            split_tree = RECT { left: rc.left + w, top, right: rc.left + w + gap, bottom };
            left = rc.left + w + gap;
        }
        MoveWindow(lv, left, top, rc.right - left, bottom - top, 1);
        with_app(|a| {
            a.split_tree = split_tree;
            a.split_details = split_details;
        });
        InvalidateRect(hwnd, null(), 1);
    }
}

unsafe fn update_visibility(hwnd: HWND) {
    unsafe {
        let Some((items, pages, page)) = with_app(|a| {
            (
                [
                    (a.toolbar, a.show_toolbar),
                    (a.tree, a.show_tree),
                    (a.lv, true),
                    (a.tabs, a.show_details),
                    (a.status, a.show_status),
                    (a.search, a.show_toolbar),
                ],
                a.pages,
                a.page,
            )
        }) else {
            return;
        };
        for (w, on) in items {
            ShowWindow(w, if on { SW_SHOW } else { SW_HIDE });
        }
        if !items[3].1 {
            for p in pages {
                ShowWindow(p, SW_HIDE);
            }
        }
        let _ = page;
        layout(hwnd);
    }
}

unsafe fn update_menu(hwnd: HWND) {
    unsafe {
        let menu = GetMenu(hwnd);
        let Some(states) = with_app(|a| {
            [
                (cmd::SHOW_TOOLBAR, a.show_toolbar),
                (cmd::SHOW_DETAILS, a.show_details),
                (cmd::SHOW_STATUS, a.show_status),
                (cmd::SHOW_TREE, a.show_tree),
                (cmd::CLOSE_TO_TRAY, a.prefs.get_bool("ui.close_to_tray", true)),
                (cmd::MIN_TO_TRAY, a.prefs.get_bool("ui.minimize_to_tray", false)),
            ]
        }) else {
            return;
        };
        for (id, on) in states {
            CheckMenuItem(menu, id as u32, MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED });
        }
        if let Some(mode) = with_app(|a| a.auto_shutdown.mode()) {
            check_auto_shutdown(menu, mode);
        }
    }
}

/// The radio mark on the Auto Shutdown item of `mode`.
unsafe fn check_auto_shutdown(menu: HMENU, mode: Option<AutoMode>) {
    let i = AutoMode::MENU.iter().position(|&m| m == mode).unwrap_or(0) as u32;
    let first = cmd::AUTO_SHUTDOWN_BASE as u32;
    let last = first + AutoMode::MENU.len() as u32 - 1;
    unsafe { CheckMenuRadioItem(menu, first, last, first + i, MF_BYCOMMAND) };
}

/// Category tree: Torrents with its four categories, then Labels.
unsafe fn build_tree() {
    unsafe {
        let Some(tree) = with_app(|a| a.tree) else { return };
        send(tree, TVM_DELETEITEM, 0, TVI_ROOT);
        let mut items = Vec::new();
        let root = insert_tree(tree, TVI_ROOT, tr!(CatTorrents), 0);
        items.push((root, Filter::Category(Category::All)));
        for c in [Category::Downloading, Category::Completed, Category::Active, Category::Inactive] {
            let h = insert_tree(tree, root, c.label(), items.len());
            items.push((h, Filter::Category(c)));
        }
        let labels = insert_tree(tree, TVI_ROOT, tr!(CatLabels), items.len());
        items.push((labels, Filter::Category(Category::All)));
        let none = insert_tree(tree, labels, tr!(CatNoLabel), items.len());
        items.push((none, Filter::Label(String::new())));
        send(tree, TVM_EXPAND, TVE_EXPAND as usize, root);
        send(tree, TVM_EXPAND, TVE_EXPAND as usize, labels);
        send(tree, TVM_SELECTITEM, TVGN_CARET as usize, root);
        with_app(|a| {
            a.tree_texts = vec![String::new(); items.len()];
            a.tree_items = items;
            a.tree_labels.clear();
            a.feed_tree.clear();
        });
        update_feed_tree();
    }
}

unsafe fn insert_tree(tree: HWND, parent: HTREEITEM, text: &str, param: usize) -> HTREEITEM {
    let mut t = wide(text);
    unsafe {
        let mut ins: TVINSERTSTRUCTW = zeroed();
        ins.hParent = parent;
        ins.hInsertAfter = TVI_LAST;
        ins.Anonymous.item.mask = TVIF_TEXT | TVIF_PARAM;
        ins.Anonymous.item.pszText = t.as_mut_ptr();
        ins.Anonymous.item.lParam = param as isize;
        send(tree, TVM_INSERTITEMW, 0, &ins as *const _ as LPARAM)
    }
}

/// Updates the counts in the tree and adds or removes label entries.
unsafe fn update_tree() {
    let _timing = bench::span("update_tree");
    unsafe {
        let Some((tree, labels, changed)) = with_app(|a| {
            let generation = Some(a.list.generation());
            if a.tree_generation == generation {
                return None;
            }
            a.tree_generation = generation;
            let labels = a.list.labels();
            let names: Vec<String> = labels.iter().map(|(l, _)| l.clone()).collect();
            let changed = names != a.tree_labels;
            Some((a.tree, labels, changed))
        })
        .flatten() else {
            return;
        };
        if changed {
            // Label entries follow the fixed ones; drop them and add the current set.
            let Some((stale, parent, selected_label)) = with_app(|a| {
                let fixed = 7;
                let stale: Vec<HTREEITEM> = a.tree_items.iter().skip(fixed).map(|(h, _)| *h).collect();
                let sel = match a.list.filter() {
                    Filter::Label(l) if !l.is_empty() => Some(l.clone()),
                    _ => None,
                };
                (stale, a.tree_items[5].0, sel)
            }) else {
                return;
            };
            for h in stale {
                send(tree, TVM_DELETEITEM, 0, h);
            }
            let mut new_items = Vec::new();
            for (i, (l, _)) in labels.iter().enumerate() {
                let h = insert_tree(tree, parent, l, 7 + i);
                new_items.push((h, Filter::Label(l.clone())));
            }
            let reselect = selected_label
                .and_then(|l| new_items.iter().find(|(_, f)| *f == Filter::Label(l.clone())).map(|x| x.0));
            let names = labels.iter().map(|(l, _)| l.clone()).collect();
            with_app(|a| {
                a.tree_items.truncate(7);
                a.tree_texts.truncate(7);
                a.tree_texts.extend(std::iter::repeat_n(String::new(), new_items.len()));
                a.tree_items.extend(new_items);
                a.tree_labels = names;
            });
            match reselect {
                Some(h) => {
                    send(tree, TVM_SELECTITEM, TVGN_CARET as usize, h);
                }
                None => {
                    // The selected label went away: show everything.
                    let gone = with_app(|a| matches!(a.list.filter(), Filter::Label(l) if !l.is_empty()));
                    if gone == Some(true) {
                        let root = with_app(|a| a.tree_items[0].0).unwrap_or(0);
                        send(tree, TVM_SELECTITEM, TVGN_CARET as usize, root);
                    }
                }
            }
        }
        let Some(updates) = with_app(|a| {
            let label_counts = a.list.labels();
            let no_label = a.list.unlabeled();
            let mut out = Vec::new();
            for (i, (h, f)) in a.tree_items.iter().enumerate() {
                let text = match (i, f) {
                    (5, _) => tr!(CatLabels).to_owned(),
                    (_, Filter::Category(c)) => format!("{} ({})", c.label(), a.list.count(*c)),
                    (_, Filter::Label(l)) if l.is_empty() => format!("{} ({no_label})", tr!(CatNoLabel)),
                    (_, Filter::Label(l)) => {
                        let n = label_counts.iter().find(|(x, _)| x == l).map_or(0, |x| x.1);
                        format!("{l} ({n})")
                    }
                };
                if a.tree_texts[i] != text {
                    a.tree_texts[i] = text.clone();
                    out.push((*h, text));
                }
            }
            out
        }) else {
            return;
        };
        for (h, text) in updates {
            let mut t = wide(&text);
            let mut item: TVITEMW = zeroed();
            item.mask = TVIF_TEXT;
            item.hItem = h;
            item.pszText = t.as_mut_ptr();
            send(tree, TVM_SETITEMW, 0, &item as *const _ as LPARAM);
        }
    }
}

/// Rebuilds the visible rows after an update, keeping the selection on the same torrents.
///
/// Runs on every status update (once a second): when only cell values changed (the model's
/// view kept its order) it just repaints the visible rows; the selection is carried over only
/// when rows moved, joined or left.
unsafe fn refresh_list() {
    let _timing = bench::span("refresh_list");
    unsafe {
        let Some(lv) = with_app(|a| a.feed_view.is_none().then_some(a.lv)).flatten() else { return };
        let Some((refreshed, rows_moved)) = with_app(|a| {
            let _timing = bench::span("model.refresh");
            let generation = a.list.view_generation();
            let refreshed = a.list.refresh();
            (refreshed, a.list.view_generation() != generation)
        }) else {
            return;
        };
        // Nothing changed: no repaint either.
        if !refreshed {
            return;
        }
        if rows_moved {
            carry_selection(lv);
        }
        InvalidateRect(lv, null(), 0);
    }
}

/// Where the selection goes after the view changed.
enum NewSelection {
    /// Every row (all were selected and no row joined).
    All,
    Rows(Vec<usize>),
}

/// After the view changed: the row count, then the selection and focus moved to the rows their
/// torrents went to (`TorrentList::previous_view` is what the list showed until now).
unsafe fn carry_selection(lv: HWND) {
    unsafe {
        let selected = controls::selected_count(lv);
        let all = selected > 0 && with_app(|a| a.list.previous_view().len() == selected) == Some(true);
        // Select All with 100,000 rows: no walk of the selection
        let sel = if all { Vec::new() } else { controls::selected(lv) };
        let focus = controls::focused(lv);
        let Some((count, new_sel, new_focus, moved)) = with_app(|a| {
            let (old, view) = (a.list.previous_view(), a.list.view());
            let focus_id = focus.and_then(|i| old.get(i).copied());
            let index = |id: TorrentId| view.iter().position(|&v| v == id);
            let new_sel = if all {
                let old: HashSet<TorrentId> = old.iter().copied().collect();
                if view.iter().all(|id| old.contains(id)) {
                    NewSelection::All
                } else {
                    NewSelection::Rows(
                        view.iter().enumerate().filter(|(_, id)| old.contains(id)).map(|(i, _)| i).collect(),
                    )
                }
            } else {
                let sel_ids = sel.iter().filter_map(|&i| old.get(i).copied());
                NewSelection::Rows(if sel.len() > 64 {
                    let map: std::collections::HashMap<TorrentId, usize> =
                        view.iter().enumerate().map(|(i, &id)| (id, i)).collect();
                    sel_ids.filter_map(|id| map.get(&id).copied()).collect()
                } else {
                    sel_ids.filter_map(index).collect()
                })
            };
            let new_focus = focus_id.and_then(index);
            let moved = new_focus != focus
                || match &new_sel {
                    NewSelection::All => view.len() != old.len(),
                    NewSelection::Rows(rows) => *rows != sel,
                };
            if moved {
                a.restoring = true;
            }
            (view.len(), new_sel, new_focus, moved)
        }) else {
            return;
        };
        controls::set_count(lv, count);
        if moved {
            match new_sel {
                NewSelection::All => controls::set_state(lv, None, true, false),
                NewSelection::Rows(rows) => {
                    controls::set_state(lv, None, false, false);
                    for i in rows {
                        controls::set_state(lv, Some(i), true, false);
                    }
                }
            }
            if let Some(f) = new_focus {
                let mut item: LVITEMW = zeroed();
                item.stateMask = LVIS_FOCUSED;
                item.state = LVIS_FOCUSED;
                send(lv, LVM_SETITEMSTATE, f, &item as *const _ as LPARAM);
            }
            with_app(|a| a.restoring = false);
            selection_changed();
        }
    }
}

fn selected_ids() -> Vec<TorrentId> {
    let Some(lv) = with_app(|a| a.feed_view.is_none().then_some(a.lv)).flatten() else { return Vec::new() };
    let rows = unsafe { controls::selected(lv) };
    with_app(|a| rows.iter().filter_map(|&i| a.list.view().get(i).copied()).collect()).unwrap_or_default()
}

/// The torrent the detail tabs show: the focused selected row, else the first selected.
unsafe fn selection_changed() {
    unsafe {
        let Some((lv, restoring)) = with_app(|a| (a.lv, a.restoring)) else { return };
        if restoring {
            return;
        }
        // the focused row and the first selected one, never a walk of a 100,000-row selection
        let focus = controls::focused(lv).filter(|&f| controls::is_selected(lv, f));
        let row = focus.or_else(|| controls::first_selected(lv));
        let changed = with_app(|a| {
            // feed items have no detail
            let id = row.filter(|_| a.feed_view.is_none()).and_then(|r| a.list.view().get(r).copied());
            let changed = a.detail.id != id;
            a.detail.select(id);
            if changed {
                a.files_requested = None;
            }
            changed
        });
        update_toolbar();
        if changed == Some(true) {
            update_detail_lists();
            request_detail();
            invalidate_page();
        }
    }
}

unsafe fn update_toolbar() {
    unsafe {
        let Some((toolbar, lv, feeds)) = with_app(|a| (a.toolbar, a.lv, a.feed_view.is_some())) else {
            return;
        };
        // feed items are not torrents
        let any = !feeds && controls::selected_count(lv) > 0;
        for id in [cmd::REMOVE, cmd::START, cmd::PAUSE, cmd::STOP, cmd::UP, cmd::DOWN] {
            send(toolbar, TB_ENABLEBUTTON, id as usize, any as isize);
        }
    }
}

/// Asks the engine for what the visible detail tab needs.
fn request_detail() {
    with_app(|a| {
        if !a.show_details {
            return;
        }
        let Some(id) = a.detail.id else { return };
        let t = a.handle.torrent(id);
        match a.page {
            Page::General | Page::Pieces => t.post_piece_info(),
            Page::Trackers => t.post_trackers(),
            Page::Peers => t.post_peer_info(),
            Page::Files => {
                if a.files_requested != Some(id) {
                    a.files_requested = Some(id);
                    t.post_file_list();
                }
                t.post_file_progress();
            }
            Page::Speed | Page::Logger => {}
        }
    });
}

unsafe fn update_detail_lists() {
    unsafe {
        let Some(items) = with_app(|a| {
            let mut items: Vec<_> = Page::ALL
                .iter()
                .filter(|p| p.is_list())
                .map(|&p| (a.pages[p.index()], a.detail.rows(p), p == Page::Logger))
                .collect();
            items.push((a.piece_list, a.detail.rows(Page::Pieces), false));
            items
        }) else {
            return;
        };
        place_piece_list();
        for (lv, n, is_log) in items {
            let before = send(lv, LVM_GETITEMCOUNT, 0, 0) as usize;
            controls::set_count(lv, n);
            if is_log && n > before {
                send(lv, LVM_ENSUREVISIBLE, n.saturating_sub(1), 0);
            }
            InvalidateRect(lv, null(), 0);
        }
    }
}

/// Puts the pieces list under the Pieces panel's piece map, which takes the height
/// [`crate::details::piece_map_height`] gives it.
unsafe fn place_piece_list() {
    unsafe {
        let Some((panel, lv, top)) = with_app(|a| {
            let panel = a.pages[Page::Pieces.index()];
            let mut rc: RECT = zeroed();
            GetClientRect(panel, &mut rc);
            (panel, a.piece_list, crate::details::piece_map_height(&a.detail, rc.right, rc.bottom, a.dpi))
        }) else {
            return;
        };
        let mut rc: RECT = zeroed();
        GetClientRect(panel, &mut rc);
        let mut now: RECT = zeroed();
        GetWindowRect(lv, &mut now);
        let mut corner = POINT { x: now.left, y: now.top };
        ScreenToClient(panel, &mut corner);
        let (w, h) = (rc.right, (rc.bottom - top).max(0));
        if corner.x != 0 || corner.y != top || now.right - now.left != w || now.bottom - now.top != h {
            MoveWindow(lv, 0, top, w, h, 1);
            InvalidateRect(panel, null(), 0);
        }
    }
}

fn invalidate_page() {
    if let Some(w) = with_app(|a| a.pages[a.page.index()]) {
        unsafe { InvalidateRect(w, null(), 0) };
    }
}

fn on_timer() {
    let _timing = bench::span("timer");
    let step = with_app(|a| {
        a.handle.post_torrent_updates();
        a.handle.post_session_stats();
        run_scheduler(a, None);
        // no scrapes while the scheduler has the session turned off
        if a.scheduler.mode() != Mode::Off {
            let list = &a.list;
            a.scrapes.run(list.ids().filter_map(|id| list.get(id)), std::time::Instant::now(), &a.handle);
        }
        autoload(a);
        rss_timer(a);
        let list = &a.list;
        let now = std::time::Instant::now();
        let step = a.auto_shutdown.check(list.ids().filter_map(|id| list.get(id)), now);
        (a.hwnd, step, a.updates.due(now), a.updates.expire(now))
    });
    request_detail();
    if let Some((hwnd, step, check, notice_over)) = step {
        if check {
            start_update_check(hwnd);
        }
        if notice_over {
            unsafe { show_update_notice(hwnd) };
        }
        unsafe { auto_shutdown_step(hwnd, step) };
    }
}

/// Options > Auto Shutdown: the timer's step. The countdown opens from its own message, not
/// inside the timer; it closes when time is up (its opener then acts) or new work comes.
unsafe fn auto_shutdown_step(hwnd: HWND, step: Step) {
    match step {
        Step::Wait => {}
        Step::Start(secs) => unsafe {
            PostMessageW(hwnd, WM_APP_COUNTDOWN, secs as usize, 0);
        },
        Step::Tick(secs) => shutdown::tick(secs),
        Step::Abort => {
            shutdown::close(Outcome::Aborted);
            log_line(tr!(LogShutdownNewWork));
        }
        Step::Act(_) if shutdown::is_open() => shutdown::close(Outcome::TimeUp),
        Step::Act(action) => unsafe { auto_shutdown_act(hwnd, action) },
    }
}

/// `WM_APP_COUNTDOWN`: the countdown dialog, then what it ended with.
unsafe fn auto_shutdown_countdown(hwnd: HWND, secs: u32) {
    let Some(Some(mode)) = with_app(|a| a.auto_shutdown.mode().filter(|_| a.auto_shutdown.counting_down()))
    else {
        return;
    };
    if shutdown::is_open() {
        return;
    }
    log_line(&tr!(LogShutdownDone, what = mode.action.doing(), secs));
    match shutdown::countdown(hwnd, mode, secs) {
        Outcome::Cancelled => {
            with_app(|a| a.auto_shutdown.set_mode(None));
            log_line(tr!(LogShutdownCancelled));
            unsafe { update_menu(hwnd) };
        }
        Outcome::Aborted => {}
        Outcome::TimeUp => unsafe { auto_shutdown_act(hwnd, mode.action) },
    }
}

/// Quit as File > Exit does (state saved on the way out); Shut Down after that; Standby and
/// Hibernate once the engine has written the resume data (`ResumeDataSaved` in `on_alerts`).
unsafe fn auto_shutdown_act(hwnd: HWND, action: auto_shutdown::Action) {
    log_line(&tr!(LogShutdown, what = action.doing()));
    unsafe { update_menu(hwnd) };
    match Power::of(action) {
        Some(p @ (Power::Standby | Power::Hibernate)) => {
            with_app(|a| {
                if let Err(e) = a.prefs.save() {
                    a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
                }
                a.suspend_pending = Some(p);
                a.handle.save_resume_data();
            });
        }
        p => {
            if let Some(p) = p {
                shutdown::after_exit(p);
            }
            with_app(|a| a.quitting = true);
            unsafe { DestroyWindow(hwnd) };
        }
    }
}

fn log_line(text: &str) {
    with_app(|a| a.detail.push_log(format!("{} {text}", now_hms())));
}

/// The search box's keys the edit control has no use for: Escape clears it, Enter goes to the
/// list, Ctrl+A selects its text. Returns whether `msg` was one.
unsafe fn search_key(msg: &MSG) -> bool {
    unsafe {
        if msg.message != WM_KEYDOWN {
            return false;
        }
        let Some((search, lv)) = with_app(|a| (a.search, a.lv)) else { return false };
        if msg.hwnd != search {
            return false;
        }
        let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
        match msg.wParam as u16 {
            VK_ESCAPE => {
                SetWindowTextW(search, w!(""));
                SetFocus(lv);
            }
            VK_RETURN => {
                SetFocus(lv);
            }
            k if ctrl && k == b'A' as u16 => {
                send(search, EM_SETSEL, 0, -1);
            }
            _ => return false,
        }
        true
    }
}

/// The search box changed: the list shows the names containing its text.
fn on_search() {
    let Some(search) = with_app(|a| a.search) else { return };
    let text = unsafe { crate::util::window_text(search) };
    with_app(|a| a.list.set_search(&text));
    unsafe { refresh_list() };
}

/// File > Import: the client's torrents with their data, statistics and labels.
unsafe fn import_from(hwnd: HWND, client: import::Client) {
    let mut picked: Option<PathBuf> = None;
    let found = loop {
        match import::begin(client, picked.as_deref()) {
            import::Begin::Refuse(e) => return dialogs::error(hwnd, &e),
            import::Begin::PickFolder(title) => match unsafe { crate::create::pick_folder(hwnd, &title) } {
                Some(dir) => picked = Some(PathBuf::from(dir)),
                None => return,
            },
            import::Begin::Confirm(question, found) => {
                if !dialogs::ask(hwnd, &question) {
                    return;
                }
                break found;
            }
        }
    };
    let Some((done, handle)) = with_app(|a| {
        let known: HashSet<String> =
            a.list.ids().filter_map(|id| a.list.get(id)).map(|st| st.info_hashes.key_hex()).collect();
        let done = import::finish(client, found, &mut a.prefs, |k| known.contains(k));
        let _ = a.prefs.save();
        (done, a.handle.clone())
    }) else {
        return;
    };
    for line in &done.log {
        log_line(line);
    }
    for p in done.add {
        handle.add_torrent(p);
    }
    dialogs::info(hwnd, &done.summary);
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
        let key = ti.info_hashes().key_hex();
        if a.list.ids().any(|id| a.list.get(id).is_some_and(|s| s.info_hashes.key_hex() == key)) {
            let line = torrent_folders::loaded(ti.name(), &key, &path, a.torrent_folders.autoload_delete);
            a.detail.push_log(format!("{} {}", now_hms(), tr!(LogAlreadyInList, line)));
            continue;
        }
        a.torrent_files.loading(&ti, path);
        let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(bytes), a.save_path.clone());
        p.flags = TorrentFlags::DEFAULT;
        GuiOptions::load(&a.prefs, &a.save_path).apply_to_new(&mut p);
        a.scheduler.apply_to_new(&mut p);
        add_to_session(a, p);
    }
}

/// Hands a torrent to the engine; a `.torrent`'s copy is stored once it is added.
fn add_to_session(a: &mut App, p: AddTorrentParams) {
    a.torrent_files.adding(&a.torrent_folders, &p);
    a.handle.add_torrent(p);
}

/// Puts the scheduler's mode for this hour into effect. `applied`: the options just applied
/// (their settings go to the engine through the scheduler, even if the mode stays).
fn run_scheduler(a: &mut App, applied: Option<&Options>) {
    let now = unix_now();
    let offset = local_utc_offset(now);
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
        a.status_texts.clear();
    }
}

unsafe fn on_alerts(hwnd: HWND) {
    let _timing = bench::span("on_alerts");
    let mut errors: Vec<String> = Vec::new();
    let mut balloons: Vec<String> = Vec::new();
    let mut suspend = None;
    let mut magnet_ready = false;
    let Some((lists_changed, removed_any, repaint_page)) = with_app(|a| {
        let mut alerts = std::mem::take(&mut a.alerts);
        a.handle.pop_alerts(&mut alerts);
        let mut lists_changed = false;
        let mut removed_any = false;
        let generation = a.list.generation();
        let mut repaint_page = false;
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
                            // First sight: restore what only the frontend knows.
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
                    repaint_page |= a.page == Page::Speed;
                }
                Alert::ResumeDataSaved => suspend = a.suspend_pending.take(),
                Alert::TorrentAdded { id, info_hashes, ref name } => {
                    a.detail.push_log(format!("{} {}", now_hms(), tr!(LogAdded, name)));
                    if a.queue_top.remove(&info_hashes.key_hex()) {
                        a.handle.torrent(id).queue_position(QueueMove::Top);
                    }
                    a.handle.post_torrent_updates();
                    lists_changed = true;
                }
                Alert::AddTorrentFailed { ref name, ref error } => {
                    a.detail.push_log(format!("{} {}", now_hms(), alert.message()));
                    errors.push(tr!(ErrAddTorrent, name, error));
                    lists_changed = true;
                }
                Alert::TorrentRemoved { id, info_hashes } => {
                    a.list.remove(id);
                    a.known.remove(&id);
                    a.downloading.remove(&id);
                    let key = info_hashes.key_hex();
                    a.prefs.remove(&format!("torrent.{key}.label"));
                    a.prefs.remove(&format!("torrent.{key}.paused"));
                    SeedGoal::store_override(&mut a.prefs, &key, None);
                    removed_any = true;
                }
                Alert::RemoteDisconnected { ref message } => {
                    a.detail.push_log(format!("{} {}", now_hms(), alert.message()));
                    let label = crate::remote_ui::get().map_or("", |r| r.label.as_str());
                    let title = wide(&tr!(TitleDisconnected, version = BUILD, label));
                    // SAFETY: the main window is live while the app state is.
                    unsafe { SetWindowTextW(a.hwnd, title.as_ptr()) };
                    errors.push(tr!(ErrRemoteLost, label, message));
                }
                Alert::TorrentFinished { id } => {
                    let name = a.list.get(id).map(|st| st.name.clone()).unwrap_or_default();
                    a.detail.push_log(format!("{} {}", now_hms(), tr!(LogFinished, name)));
                    if a.downloading.remove(&id) {
                        balloons.push(name);
                    }
                    lists_changed = true;
                }
                Alert::StorageMoved { id, .. } | Alert::StorageMovedFailed { id, .. } => {
                    let name = a.list.get(id).map(|st| st.name.clone()).unwrap_or_default();
                    a.detail.push_log(format!("{} {name}: {}", now_hms(), alert.message()));
                    if let Alert::StorageMovedFailed { ref error, ref file, .. } = alert {
                        let mut text = tr!(ErrMoveFiles, name, error);
                        if !file.is_empty() {
                            text.push('\n');
                            text.push_str(file);
                        }
                        errors.push(text);
                    }
                    // the new save path (or the old one, after a failure) and the end of "Moving"
                    a.handle.post_torrent_updates();
                    lists_changed = true;
                }
                Alert::FileRenamed { id, .. } | Alert::FileRenameFailed { id, .. } => {
                    let name = a.list.get(id).map(|st| st.name.clone()).unwrap_or_default();
                    a.detail.push_log(format!("{} {name}: {}", now_hms(), alert.message()));
                    if a.detail.id == Some(id) {
                        a.handle.torrent(id).post_file_list();
                    }
                    lists_changed = true;
                }
                Alert::PeerInfo { id, peers } if a.detail.id == Some(id) => {
                    a.detail.set_peers(peers);
                    lists_changed = true;
                }
                Alert::TrackerList { id, trackers } if a.detail.id == Some(id) => {
                    a.detail.trackers = trackers;
                    lists_changed = true;
                }
                Alert::FileList { id, files } if a.detail.id == Some(id) => {
                    a.detail.set_files(files);
                    lists_changed = true;
                }
                Alert::FileProgress { id, done } if a.detail.id == Some(id) => {
                    a.detail.file_done = done;
                    lists_changed = true;
                }
                Alert::PieceInfo { id, have, availability, downloading } if a.detail.id == Some(id) => {
                    a.detail.have = Some(have);
                    a.detail.availability = availability;
                    a.detail.partial = downloading;
                    repaint_page = true;
                    lists_changed = true;
                }
                Alert::PeerInfo { .. }
                | Alert::TrackerList { .. }
                | Alert::FileList { .. }
                | Alert::FileProgress { .. }
                | Alert::PieceInfo { .. }
                | Alert::TorrentFile { .. } => {}
                // one per stopped or queued torrent every half hour (`frontend::scrape`); the
                // counts show in the Seeds and Peers columns
                Alert::ScrapeReply { .. } | Alert::ScrapeFailed { .. } => {}
                // the mappers' step-by-step chatter (SOAP bodies included) stays out of the
                // logger; the mapped ports, failures and the external address still show
                Alert::PortmapLog { .. } => {}
                other => {
                    let name = other.torrent().and_then(|id| a.list.get(id)).map(|st| st.name.clone());
                    let line = match name {
                        Some(n) => format!("{} {n}: {}", now_hms(), other.message()),
                        None => format!("{} {}", now_hms(), other.message()),
                    };
                    a.detail.push_log(line);
                    lists_changed = true;
                }
            }
        }
        a.alerts = alerts;
        // The General tab shows the selected torrent's status.
        repaint_page |= a.list.generation() != generation;
        (lists_changed, removed_any, repaint_page)
    }) else {
        return;
    };
    unsafe {
        refresh_list();
        update_tree();
        update_status_bar();
        if lists_changed || removed_any {
            update_detail_lists();
        }
        if repaint_page {
            invalidate_page();
        }
        if removed_any {
            selection_changed();
            with_app(|a| {
                let _ = a.prefs.save();
            });
        }
        if with_app(|a| a.prefs.get_bool("ui.balloons", true)) != Some(true) {
            balloons.clear();
        }
        for name in balloons {
            tray::balloon(hwnd, tr!(BalloonComplete), &name);
        }
        for e in errors {
            dialogs::error(hwnd, &e);
        }
        if magnet_ready {
            // posted: the dialog's message loop must not run inside this alert batch
            PostMessageW(hwnd, WM_APP_MAGNET, 0, 0);
        }
        bench::after_alerts();
        // Auto Shutdown's Standby / Hibernate; returns once the computer is awake again
        if let Some(Err(e)) = suspend.map(shutdown::perform) {
            log_line(&tr!(LogShutdown, what = e.replace('\n', " ")));
            dialogs::error(hwnd, &e);
        }
    }
}

unsafe fn update_status_bar() {
    let Some((status, hwnd, parts, tip, changed, net_icon, first)) = with_app(|a| {
        let s = &a.stats;
        let has = s.has_incoming_connections;
        let net_icon = (a.net_shown != Some(has)).then(|| {
            a.net_shown = Some(has);
            a.net_icons[has as usize]
        });
        let net = if s.has_incoming_connections { tr!(NetOk) } else { tr!(NetNoIncoming) };
        let net = match a.scheduler.status_text() {
            Some(sched) => format!("{net} | {sched}"),
            None => net.to_owned(),
        };
        let parts = [
            net,
            tr!(BarDht, count = s.dht_nodes),
            tr!(BarDown, rate = fmt::speed_always(s.download_rate), total = fmt::size(s.recv_payload_bytes)),
            tr!(BarUp, rate = fmt::speed_always(s.upload_rate), total = fmt::size(s.sent_payload_bytes)),
        ];
        let tip = tr!(
            TrayTip,
            version = BUILD,
            down = fmt::speed_always(s.download_rate),
            up = fmt::speed_always(s.upload_rate)
        );
        let changed = tip != a.tray_tip;
        if changed {
            a.tray_tip = tip.clone();
        }
        let parts: Vec<(usize, String)> =
            parts.into_iter().enumerate().filter(|(i, p)| a.status_texts.get(*i) != Some(p)).collect();
        for (i, p) in &parts {
            if a.status_texts.len() <= *i {
                a.status_texts.resize(*i + 1, String::new());
            }
            a.status_texts[*i] = p.clone();
        }
        // the update notice, when there is one, is the first part
        let first = usize::from(a.updates.notice().is_some());
        (a.status, a.hwnd, parts, tip, changed, net_icon, first)
    }) else {
        return;
    };
    unsafe {
        if let Some(icon) = net_icon {
            send(status, SB_SETICON, first, icon as LPARAM);
        }
        for (i, text) in &parts {
            let w = wide(text);
            send(status, SB_SETTEXTW, first + *i, w.as_ptr() as LPARAM);
        }
    }
    if changed {
        tray::set_tip(hwnd, &tip);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wparam == TIMER_REFRESH => {
                on_timer();
                0
            }
            WM_TIMER if wparam == bench::TIMER_BENCH => {
                bench::on_timer(hwnd);
                0
            }
            WM_APP_ALERTS => {
                on_alerts(hwnd);
                0
            }
            WM_APP_TRAY => {
                match lparam as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => toggle_window(hwnd),
                    WM_RBUTTONUP => tray_menu(hwnd),
                    _ => {}
                }
                0
            }
            WM_APP_URL => {
                url_done(hwnd, *Box::from_raw(lparam as *mut UrlResult));
                0
            }
            WM_APP_COUNTDOWN => {
                auto_shutdown_countdown(hwnd, wparam as u32);
                0
            }
            WM_APP_ARGS => {
                let args = with_app(|a| std::mem::take(&mut a.pending_args)).unwrap_or_default();
                add_sources(hwnd, &args);
                0
            }
            WM_APP_RSS => {
                rss_collect();
                0
            }
            WM_APP_MAGNET => {
                magnet_dialogs(hwnd);
                0
            }
            WM_APP_UPDATE => {
                update_done(hwnd, *Box::from_raw(lparam as *mut Result<Manifest, String>));
                0
            }
            WM_SETCURSOR if over_update_notice(wparam as HWND) => {
                SetCursor(LoadCursorW(null_mut(), IDC_HAND));
                1
            }
            WM_COMMAND if crate::util::loword(wparam) as usize == IDC_SEARCH => {
                if crate::util::hiword(wparam) as u32 == EN_CHANGE {
                    on_search();
                }
                0
            }
            WM_COMMAND => {
                on_command(hwnd, crate::util::loword(wparam));
                0
            }
            WM_NOTIFY => on_notify(hwnd, lparam as *const NMHDR),
            WM_CONTEXTMENU => {
                on_context_menu(hwnd, wparam as HWND, lparam);
                0
            }
            WM_SIZE => {
                if wparam as u32 == SIZE_MINIMIZED {
                    if with_app(|a| a.prefs.get_bool("ui.minimize_to_tray", false)) == Some(true) {
                        ShowWindow(hwnd, SW_HIDE);
                    }
                } else {
                    layout(hwnd);
                }
                0
            }
            WM_GETMINMAXINFO => {
                let mmi = &mut *(lparam as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd);
                mmi.ptMinTrackSize = POINT { x: scale(420, dpi), y: scale(300, dpi) };
                0
            }
            WM_DPICHANGED => {
                let r = &*(lparam as *const RECT);
                apply_dpi(hwnd, crate::util::hiword(wparam) as u32);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                0
            }
            WM_SETCURSOR if wparam as HWND == hwnd && (lparam as u32 & 0xffff) == HTCLIENT => {
                let mut p: POINT = zeroed();
                GetCursorPos(&mut p);
                windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
                match split_at(p.x, p.y) {
                    Some(Split::Tree) => {
                        SetCursor(LoadCursorW(null_mut(), IDC_SIZEWE));
                        1
                    }
                    Some(Split::Details) => {
                        SetCursor(LoadCursorW(null_mut(), IDC_SIZENS));
                        1
                    }
                    None => DefWindowProcW(hwnd, msg, wparam, lparam),
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point_from_lparam(lparam);
                if let Some(s) = split_at(x, y) {
                    with_app(|a| a.drag = Some(s));
                    SetCapture(hwnd);
                }
                0
            }
            WM_MOUSEMOVE => {
                let (x, y) = point_from_lparam(lparam);
                let moved = with_app(|a| {
                    let Some(drag) = a.drag else { return false };
                    let mut rc: RECT = zeroed();
                    GetClientRect(hwnd, &mut rc);
                    match drag {
                        Split::Tree => a.tree_w = unscale(x, a.dpi).max(40),
                        Split::Details => {
                            let mut st: RECT = zeroed();
                            if a.show_status {
                                GetWindowRect(a.status, &mut st);
                            }
                            let bottom = rc.bottom - (st.bottom - st.top);
                            a.detail_h = unscale(bottom - y, a.dpi).max(40);
                        }
                    }
                    true
                });
                if moved == Some(true) {
                    layout(hwnd);
                }
                0
            }
            WM_LBUTTONUP => {
                if with_app(|a| a.drag.take().is_some()) == Some(true) {
                    ReleaseCapture();
                }
                0
            }
            WM_DROPFILES => {
                let drop = wparam as HDROP;
                let n = DragQueryFileW(drop, u32::MAX, null_mut(), 0);
                let mut files = Vec::new();
                for i in 0..n {
                    let len = DragQueryFileW(drop, i, null_mut(), 0);
                    let mut buf = vec![0u16; len as usize + 1];
                    DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32);
                    files.push(crate::util::from_wide(&buf));
                }
                DragFinish(drop);
                add_sources(hwnd, &files);
                0
            }
            WM_COPYDATA => {
                if lparam == 0 {
                    return 0;
                }
                let Some(args) = copydata_args(&*(lparam as *const COPYDATASTRUCT)) else { return 0 };
                show_window(hwnd);
                // The sender waits for this message to return: the Add New Torrent dialog must
                // not run inside it.
                with_app(|a| a.pending_args.extend(args));
                PostMessageW(hwnd, WM_APP_ARGS, 0, 0);
                1
            }
            WM_CLOSE => {
                let to_tray = with_app(|a| !a.quitting && a.prefs.get_bool("ui.close_to_tray", true));
                if to_tray == Some(true) {
                    ShowWindow(hwnd, SW_HIDE);
                } else {
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_DESTROY => {
                on_destroy(hwnd);
                0
            }
            m if m != 0 && Some(m) == with_app(|a| a.taskbar_created) => {
                // Explorer restarted: the tray icon is gone.
                if let Some((icon, tip)) = with_app(|a| (a.icon_small, a.tray_tip.clone())) {
                    tray::add(hwnd, icon, WM_APP_TRAY, if tip.is_empty() { NAME_BUILD } else { &tip });
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn split_at(x: i32, y: i32) -> Option<Split> {
    with_app(|a| {
        let inside = |r: &RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        if a.show_tree && inside(&a.split_tree) {
            Some(Split::Tree)
        } else if a.show_details && inside(&a.split_details) {
            Some(Split::Details)
        } else {
            None
        }
    })
    .flatten()
}

unsafe fn on_destroy(hwnd: HWND) {
    unsafe {
        KillTimer(hwnd, TIMER_REFRESH);
        ShowWindow(hwnd, SW_HIDE);
        tray::remove(hwnd);
        with_app(|a| {
            sync_columns(a);
            save_placement(a);
            let cols = a.columns.clone();
            a.prefs.set_columns(&cols);
            a.prefs.set_sort(a.list.sort());
            a.prefs.set("ui.tree_width", a.tree_w);
            a.prefs.set("ui.detail_height", a.detail_h);
            a.prefs.set_bool("ui.toolbar", a.show_toolbar);
            a.prefs.set_bool("ui.details", a.show_details);
            a.prefs.set_bool("ui.status", a.show_status);
            a.prefs.set_bool("ui.tree", a.show_tree);
            a.prefs.set("ui.tab", a.page.key());
            if let Err(e) = a.prefs.save() {
                eprintln!("cannot save preferences: {e}");
            }
            if let Err(e) = a.rss.save(&rss_file()) {
                eprintln!("cannot save {}: {e}", rss_file().display());
            }
        });
        // Dropping the session saves resume data and stops the engine; take it out of the
        // RefCell first so nothing is borrowed while that blocks.
        let app = APP.with(|a| a.borrow_mut().take());
        if let Some(mut app) = app {
            app.handle.clear_alert_notify();
            if let Some(s) = app.session.take() {
                s.shutdown();
            }
            DeleteObject(app.font);
            DeleteObject(app.bold);
            ImageList_Destroy(app.images.0);
            ImageList_Destroy(app.images.1);
            ImageList_Destroy(app.states.0);
            for icon in [app.icon_big, app.icon_small].into_iter().chain(app.net_icons) {
                DestroyIcon(icon);
            }
        }
        PostQuitMessage(0);
    }
}

unsafe fn show_window(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else {
            ShowWindow(hwnd, SW_SHOW);
        }
        SetForegroundWindow(hwnd);
    }
}

unsafe fn toggle_window(hwnd: HWND) {
    unsafe {
        if IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 {
            ShowWindow(hwnd, SW_HIDE);
        } else {
            show_window(hwnd);
        }
    }
}

unsafe fn popup(hwnd: HWND, menu: HMENU, x: i32, y: i32) -> u16 {
    unsafe {
        let id =
            TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | rtl(TPM_LAYOUTRTL), x, y, 0, hwnd, null());
        DestroyMenu(menu);
        id as u16
    }
}

unsafe fn tray_menu(hwnd: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        let visible = IsWindowVisible(hwnd) != 0;
        append(menu, cmd::TRAY_TOGGLE, if visible { tr!(MenuHide) } else { tr!(MenuShow) });
        separator(menu);
        append(menu, cmd::ADD, tr!(MenuAddTorrent));
        append(menu, cmd::ADD_URL, tr!(MenuAddUrl));
        separator(menu);
        append(menu, cmd::START_ALL, tr!(MenuStartAll));
        append(menu, cmd::STOP_ALL, tr!(MenuStopAll));
        separator(menu);
        append(menu, cmd::EXIT, tr!(MenuExit));
        let mut p: POINT = zeroed();
        GetCursorPos(&mut p);
        // Without this the menu does not close when clicking elsewhere.
        SetForegroundWindow(hwnd);
        let id = popup(hwnd, menu, p.x, p.y);
        if id != 0 {
            on_command(hwnd, id);
        }
    }
}

/// Menu position for `WM_CONTEXTMENU`: the mouse, or under the focused row for Shift+F10.
unsafe fn menu_point(target: HWND, lparam: LPARAM) -> POINT {
    unsafe {
        if lparam != -1 {
            let (x, y) = point_from_lparam(lparam);
            return POINT { x, y };
        }
        let mut r: RECT = zeroed();
        GetWindowRect(target, &mut r);
        POINT { x: r.left + 20, y: r.top + 20 }
    }
}

unsafe fn on_context_menu(hwnd: HWND, target: HWND, lparam: LPARAM) {
    unsafe {
        let Some((lv, files_lv, tree, feeds)) =
            with_app(|a| (a.lv, a.pages[Page::Files.index()], a.tree, a.feed_view.is_some()))
        else {
            return;
        };
        let p = menu_point(target, lparam);
        if target == controls::header(lv) && !feeds {
            column_menu(hwnd, p);
        } else if target == lv && feeds {
            feed_item_menu(hwnd, p);
        } else if target == lv {
            torrent_menu(hwnd, p);
        } else if target == tree {
            tree_menu(hwnd, tree, p, lparam == -1);
        } else if target == files_lv {
            files_menu(hwnd, files_lv, p);
        }
    }
}

unsafe fn column_menu(hwnd: HWND, p: POINT) {
    unsafe {
        let Some(cols) = with_app(|a| {
            sync_columns(a);
            a.columns.clone()
        }) else {
            return;
        };
        let menu = CreatePopupMenu();
        for c in Column::ALL {
            let shown = cols.iter().any(|&(d, w)| d == c && w > 0);
            let idx = Column::ALL.iter().position(|&d| d == c).unwrap_or(0) as u16;
            append_flags(menu, checked(shown) | grayed(c == Column::Name), cmd::COLUMN_BASE + idx, c.title());
        }
        let id = popup(hwnd, menu, p.x, p.y);
        if id >= cmd::COLUMN_BASE && ((id - cmd::COLUMN_BASE) as usize) < Column::ALL.len() {
            let col = Column::ALL[(id - cmd::COLUMN_BASE) as usize];
            with_app(|a| {
                if let Some(slot) = a.columns.iter_mut().find(|(c, _)| *c == col) {
                    slot.1 = -slot.1;
                }
            });
            rebuild_columns();
        }
    }
}

unsafe fn torrent_menu(hwnd: HWND, p: POINT) {
    unsafe {
        let ids = selected_ids();
        if ids.is_empty() {
            return;
        }
        let Some((labels, current_label, dl, ul, single_done, (seeds, super_seeding), moving)) =
            with_app(|a| {
                let labels: Vec<String> = a.list.labels().into_iter().map(|(l, _)| l).collect();
                let first = a.list.get(ids[0]);
                let same = |f: &dyn Fn(TorrentId) -> String| {
                    let v = f(ids[0]);
                    ids.iter().all(|&id| f(id) == v).then_some(v)
                };
                let label = same(&|id| a.list.extra(id).label.to_owned());
                a.menu_labels = labels.clone();
                (
                    labels,
                    label,
                    first.map_or(0, |s| s.download_limit),
                    first.map_or(0, |s| s.upload_limit),
                    ids.len() == 1 && first.is_some_and(|s| s.is_finished),
                    super_seed_state(a, &ids),
                    ids.iter().any(|&id| a.list.get(id).is_some_and(|s| s.moving_storage)),
                )
            })
        else {
            return;
        };
        let menu = CreatePopupMenu();
        append_flags(
            menu,
            grayed(crate::remote_ui::is_remote()),
            cmd::OPEN_FOLDER,
            if single_done { tr!(MenuOpenFolder) } else { tr!(MenuOpenFolderMany) },
        );
        separator(menu);
        append(menu, cmd::FORCE_START, tr!(MenuForceStart));
        append(menu, cmd::START, &shortcut(tr!(MenuStart), Hotkey::Ctrl('S')));
        append(menu, cmd::PAUSE, &shortcut(tr!(MenuPause), Hotkey::CtrlShift('P')));
        append(menu, cmd::STOP, &shortcut(tr!(MenuStop), Hotkey::Ctrl('T')));
        separator(menu);
        append(menu, cmd::UP, tr!(MenuQueueUp));
        append(menu, cmd::DOWN, tr!(MenuQueueDown));
        append(menu, cmd::QUEUE_TOP, tr!(MenuQueueTop));
        append(menu, cmd::QUEUE_BOTTOM, tr!(MenuQueueBottom));
        separator(menu);
        append(menu, cmd::REMOVE, &shortcut(tr!(MenuRemove), Hotkey::Del));
        let remove_and = CreatePopupMenu();
        append(remove_and, cmd::REMOVE_DATA, &shortcut(tr!(MenuDeleteData), Hotkey::ShiftDel));
        submenu(menu, remove_and, tr!(MenuRemoveAnd));
        separator(menu);
        append(menu, cmd::RECHECK, tr!(MenuRecheck));
        append(menu, cmd::REANNOUNCE, tr!(MenuUpdateTracker));
        separator(menu);
        let adv = CreatePopupMenu();
        append_flags(adv, grayed(moving), cmd::SET_LOCATION, tr!(MenuSetLocation));
        append_flags(adv, checked(super_seeding) | grayed(!seeds), cmd::SUPER_SEED, tr!(MenuInitialSeeding));
        submenu(menu, adv, tr!(MenuAdvanced));
        let bw = CreatePopupMenu();
        let dl_menu = CreatePopupMenu();
        let ul_menu = CreatePopupMenu();
        for (i, &kb) in LIMITS.iter().enumerate() {
            let text = actions::limit_text(kb);
            append_flags(dl_menu, checked(dl == kb * 1024), cmd::DL_LIMIT_BASE + i as u16, &text);
            append_flags(ul_menu, checked(ul == kb * 1024), cmd::UL_LIMIT_BASE + i as u16, &text);
        }
        submenu(bw, dl_menu, tr!(MenuSetDownLimit));
        submenu(bw, ul_menu, tr!(MenuSetUpLimit));
        submenu(menu, bw, tr!(MenuBandwidth));
        let lm = CreatePopupMenu();
        append(lm, cmd::LABEL_NEW, tr!(MenuNewLabel));
        append_flags(lm, grayed(current_label.as_deref() == Some("")), cmd::LABEL_NONE, tr!(MenuRemoveLabel));
        if !labels.is_empty() {
            separator(lm);
        }
        for (i, l) in labels.iter().enumerate() {
            append_flags(
                lm,
                checked(current_label.as_deref() == Some(l.as_str())),
                cmd::LABEL_BASE + i as u16,
                l,
            );
        }
        submenu(menu, lm, tr!(MenuLabel));
        separator(menu);
        append(menu, cmd::PROPERTIES, &shortcut(tr!(MenuProperties), Hotkey::Enter));
        let id = popup(hwnd, menu, p.x, p.y);
        if id != 0 {
            on_command(hwnd, id);
        }
    }
}

unsafe fn files_menu(hwnd: HWND, files_lv: HWND, p: POINT) {
    unsafe {
        let rows = controls::selected(files_lv);
        if rows.is_empty() {
            return;
        }
        let menu = CreatePopupMenu();
        for (i, (text, _)) in PRIORITIES.iter().enumerate() {
            append(menu, cmd::FILE_PRIO_BASE + i as u16, lang::text(*text));
        }
        let id = popup(hwnd, menu, p.x, p.y);
        if id < cmd::FILE_PRIO_BASE || (id - cmd::FILE_PRIO_BASE) as usize >= PRIORITIES.len() {
            return;
        }
        let prio = PRIORITIES[(id - cmd::FILE_PRIO_BASE) as usize].1;
        with_app(|a| {
            let Some(tid) = a.detail.id else { return };
            let t = a.handle.torrent(tid);
            for r in &rows {
                if let Some(f) = a.detail.file_index(*r) {
                    t.set_file_priority(f, prio);
                }
            }
            // Read the list back to show the new priorities.
            a.files_requested = None;
        });
        request_detail();
    }
}

unsafe fn on_command(hwnd: HWND, id: u16) {
    unsafe {
        match id {
            cmd::ADD => {
                let files = dialogs::open_torrents(hwnd);
                add_sources(hwnd, &files);
            }
            cmd::ADD_URL => {
                let clip = dialogs::clipboard_text(hwnd)
                    .map(|s| s.trim().to_owned())
                    .filter(|s| {
                        s.starts_with("magnet:") || s.starts_with("http://") || s.starts_with("https://")
                    })
                    .unwrap_or_default();
                if let Some(url) = dialogs::input(hwnd, tr!(AddUrlTitle), tr!(AddUrlPrompt), &clip) {
                    if !url.is_empty() {
                        add_sources(hwnd, &[url]);
                    }
                }
            }
            cmd::CREATE => create_torrent(hwnd),
            cmd::FIND => {
                if let Some(search) = with_app(|a| a.show_toolbar.then_some(a.search)).flatten() {
                    SetFocus(search);
                    send(search, EM_SETSEL, 0, -1);
                }
            }
            id if (cmd::IMPORT_BASE..cmd::IMPORT_BASE + import::Client::ALL.len() as u16).contains(&id) => {
                import_from(hwnd, import::Client::ALL[(id - cmd::IMPORT_BASE) as usize]);
            }
            id if (cmd::AUTO_SHUTDOWN_BASE..cmd::AUTO_SHUTDOWN_BASE + AutoMode::MENU.len() as u16)
                .contains(&id) =>
            {
                let mode = AutoMode::MENU[(id - cmd::AUTO_SHUTDOWN_BASE) as usize];
                with_app(|a| a.auto_shutdown.set_mode(mode));
                log_line(&tr!(LogShutdown, what = lang::plain(AutoMode::label(mode))));
                update_menu(hwnd);
            }
            cmd::PREFS => show_preferences(hwnd),
            cmd::RSS => {
                let feed = with_app(|a| a.feed_view.flatten()).flatten();
                show_rss(hwnd, feed);
            }
            cmd::EXIT => {
                with_app(|a| a.quitting = true);
                DestroyWindow(hwnd);
            }
            cmd::CONNECT_REMOTE => crate::remote_ui::start_remote_window(hwnd),
            cmd::ABOUT => dialogs::about(
                hwnd,
                &tr!(
                    AboutText,
                    version = BUILD,
                    path = with_app(|a| a.prefs.path().map(|p| p.display().to_string()))
                        .flatten()
                        .unwrap_or_default()
                ),
            ),
            cmd::NOTICES => crate::notices::show(hwnd),
            cmd::WEBSITE => open_url(hwnd, update::WEBSITE_URL),
            cmd::CHECK_UPDATES => {
                if with_app(|a| a.updates.start_manual()) == Some(true) {
                    start_update_check(hwnd);
                }
            }
            cmd::SHOW_TOOLBAR | cmd::SHOW_DETAILS | cmd::SHOW_STATUS | cmd::SHOW_TREE => {
                with_app(|a| {
                    let v = match id {
                        cmd::SHOW_TOOLBAR => &mut a.show_toolbar,
                        cmd::SHOW_DETAILS => &mut a.show_details,
                        cmd::SHOW_STATUS => &mut a.show_status,
                        _ => &mut a.show_tree,
                    };
                    *v = !*v;
                });
                update_visibility(hwnd);
                update_menu(hwnd);
                request_detail();
            }
            cmd::CLOSE_TO_TRAY | cmd::MIN_TO_TRAY => {
                with_app(|a| {
                    let (key, default) = if id == cmd::CLOSE_TO_TRAY {
                        ("ui.close_to_tray", true)
                    } else {
                        ("ui.minimize_to_tray", false)
                    };
                    let v = !a.prefs.get_bool(key, default);
                    a.prefs.set_bool(key, v);
                });
                update_menu(hwnd);
            }
            cmd::TRAY_TOGGLE => toggle_window(hwnd),
            cmd::SELECT_ALL => {
                if let Some(lv) = with_app(|a| a.lv) {
                    controls::set_state(lv, None, true, false);
                }
            }
            cmd::START_ALL | cmd::STOP_ALL => {
                with_app(|a| {
                    let ids: Vec<TorrentId> = a.list.ids().collect();
                    for tid in ids {
                        let t = a.handle.torrent(tid);
                        if id == cmd::START_ALL {
                            actions::start(&t);
                        } else {
                            actions::stop(&t);
                        }
                        set_paused(a, tid, false);
                    }
                    a.handle.post_torrent_updates();
                });
            }
            cmd::REMOVE | cmd::REMOVE_DATA => remove_selected(hwnd, id == cmd::REMOVE_DATA),
            cmd::LABEL_NEW => {
                if let Some(l) = dialogs::input(hwnd, tr!(NewLabelTitle), tr!(NewLabelPrompt), "") {
                    if !l.is_empty() {
                        set_label(&l);
                    }
                }
            }
            cmd::LABEL_NONE => set_label(""),
            id if (cmd::LABEL_BASE..cmd::LABEL_BASE + 900).contains(&id) => {
                let l = with_app(|a| a.menu_labels.get((id - cmd::LABEL_BASE) as usize).cloned()).flatten();
                if let Some(l) = l {
                    set_label(&l);
                }
            }
            id if (cmd::DL_LIMIT_BASE..cmd::DL_LIMIT_BASE + LIMITS.len() as u16).contains(&id)
                || (cmd::UL_LIMIT_BASE..cmd::UL_LIMIT_BASE + LIMITS.len() as u16).contains(&id) =>
            {
                let download = id < cmd::UL_LIMIT_BASE;
                let base = if download { cmd::DL_LIMIT_BASE } else { cmd::UL_LIMIT_BASE };
                let bytes = LIMITS[(id - base) as usize] * 1024;
                let ids = selected_ids();
                with_app(|a| {
                    for tid in ids {
                        let t = a.handle.torrent(tid);
                        if download {
                            t.set_download_limit(bytes);
                        } else {
                            t.set_upload_limit(bytes);
                        }
                    }
                    a.handle.post_torrent_updates();
                });
            }
            cmd::SET_LOCATION => set_download_location(hwnd),
            // Enter on a feed item downloads it, as its double-click does
            cmd::PROPERTIES if with_app(|a| a.feed_view.is_some()) == Some(true) => download_feed_items(),
            cmd::PROPERTIES => show_properties(hwnd),
            cmd::SUPER_SEED => {
                let ids = selected_ids();
                with_app(|a| {
                    // µTorrent toggles: on for all unless every selected torrent has it.
                    let (_, on) = super_seed_state(a, &ids);
                    for tid in ids {
                        let t = a.handle.torrent(tid);
                        if on {
                            t.unset_flags(TorrentFlags::SUPER_SEEDING);
                        } else {
                            t.set_flags(TorrentFlags::SUPER_SEEDING);
                        }
                    }
                    a.handle.post_torrent_updates();
                });
            }
            cmd::START
            | cmd::FORCE_START
            | cmd::PAUSE
            | cmd::STOP
            | cmd::RECHECK
            | cmd::REANNOUNCE
            | cmd::UP
            | cmd::DOWN
            | cmd::QUEUE_TOP
            | cmd::QUEUE_BOTTOM
            | cmd::OPEN_FOLDER => torrent_action(id),
            _ => {}
        }
    }
}

/// A queued seed that reached its seeding goal (its own, else Preferences > Queueing's) is
/// stopped or held to the goal's upload rate.
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

/// The context menu's Properties (and a double-click on a torrent that is not finished): the
/// Torrent Properties dialog for the selected torrents; OK applies what changed to each one.
unsafe fn show_properties(hwnd: HWND) {
    let ids = selected_ids();
    let Some(Some((title, props))) = with_app(|a| {
        let ids: Vec<TorrentId> = ids.iter().copied().filter(|&id| a.list.get(id).is_some()).collect();
        if ids.is_empty() {
            return None;
        }
        let trackers =
            properties::fetch_trackers(&a.handle, &ids, &mut a.alerts, std::time::Duration::from_secs(2));
        let items: Vec<TorrentProps> = ids
            .iter()
            .map(|id| {
                let st = a.list.get(*id).expect("filtered");
                TorrentProps::new(st, trackers.get(id).map(Vec::as_slice), &a.prefs)
            })
            .collect();
        let names: Vec<String> =
            ids.iter().map(|id| a.list.get(*id).expect("filtered").name.clone()).collect();
        let defaults = GuiOptions::load(&a.prefs, &a.save_path).property_defaults();
        Some((crate::properties::title(&names), Properties::load(&items, defaults)))
    }) else {
        return;
    };
    // the alerts popped while waiting for the tracker lists
    unsafe { PostMessageW(hwnd, WM_APP_ALERTS, 0, 0) };
    let Some(changes) = crate::properties::show(hwnd, &title, props) else { return };
    if changes.is_empty() {
        return;
    }
    with_app(|a| {
        let defaults = GuiOptions::load(&a.prefs, &a.save_path).property_defaults();
        for &id in &ids {
            let Some(st) = a.list.get(id) else { continue };
            let key = st.info_hashes.key_hex();
            changes.apply(&a.handle.torrent(id));
            changes.store(&mut a.prefs, &key, &defaults);
        }
        if let Err(e) = a.prefs.save() {
            a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
        }
        a.handle.post_torrent_updates();
    });
    request_detail();
}

/// Advanced > Set Download Location: a folder, then the selected torrents' files move there
/// (µTorrent 2.2.1 moves them; files already there are replaced, libtorrent's default).
unsafe fn set_download_location(hwnd: HWND) {
    let ids = selected_ids();
    let Some(start) = with_app(|a| ids.first().and_then(|&id| a.list.get(id)).map(|st| st.save_path.clone()))
    else {
        return;
    };
    let dir = if crate::remote_ui::is_remote() {
        // a folder on the server, typed (the Web UI's question)
        let text = tr!(WebSetLocation);
        let (title, prompt) = text.split_once("\n\n").unwrap_or((text, text));
        match dialogs::input(hwnd, title, prompt, start.as_deref().unwrap_or_default()) {
            Some(d) if !d.trim().is_empty() => d.trim().to_owned(),
            _ => return,
        }
    } else {
        let title = tr!(PickLocation);
        let Some(dir) = (unsafe { crate::create::pick_folder_at(hwnd, title, start.as_deref()) }) else {
            return;
        };
        dir
    };
    with_app(|a| {
        for id in ids {
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
    unsafe { update_detail_lists() };
}

/// (every torrent is a seed, every torrent is super seeding), for "Initial Seeding".
fn super_seed_state(a: &App, ids: &[TorrentId]) -> (bool, bool) {
    let all = |f: &dyn Fn(&patorrent_core::TorrentStatus) -> bool| {
        ids.iter().all(|&id| a.list.get(id).is_some_and(f))
    };
    (all(&|s| s.is_seeding), all(&|s| s.super_seeding))
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

fn set_label(label: &str) {
    let ids = selected_ids();
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
        let _ = a.prefs.save();
    });
    unsafe {
        refresh_list();
        update_tree();
    }
}

unsafe fn torrent_action(id: u16) {
    let ids = selected_ids();
    if ids.is_empty() {
        return;
    }
    let folder = with_app(|a| {
        // Queue moves go in an order that keeps the selection's relative order.
        let mut ids = ids.clone();
        let qpos =
            |a: &App, id: &TorrentId| a.list.get(*id).and_then(|s| s.queue_position).unwrap_or(u32::MAX);
        match id {
            cmd::UP | cmd::QUEUE_BOTTOM => ids.sort_by_key(|i| qpos(a, i)),
            cmd::DOWN | cmd::QUEUE_TOP => ids.sort_by_key(|i| std::cmp::Reverse(qpos(a, i))),
            _ => {}
        }
        let mut folder = None;
        for tid in ids {
            let t = a.handle.torrent(tid);
            match id {
                cmd::START => {
                    actions::start(&t);
                    set_paused(a, tid, false);
                }
                cmd::FORCE_START => {
                    actions::force_start(&t);
                    set_paused(a, tid, false);
                }
                cmd::PAUSE => {
                    actions::stop(&t);
                    set_paused(a, tid, true);
                }
                cmd::STOP => {
                    actions::stop(&t);
                    set_paused(a, tid, false);
                }
                cmd::RECHECK => t.force_recheck(),
                cmd::REANNOUNCE => t.force_reannounce(),
                cmd::UP => t.queue_position(QueueMove::Up),
                cmd::DOWN => t.queue_position(QueueMove::Down),
                cmd::QUEUE_TOP => t.queue_position(QueueMove::Top),
                cmd::QUEUE_BOTTOM => t.queue_position(QueueMove::Bottom),
                cmd::OPEN_FOLDER if folder.is_none() && !crate::remote_ui::is_remote() => {
                    folder = a.list.get(tid).and_then(|st| open_folder_target(&st.save_path, &st.name));
                }
                _ => {}
            }
        }
        a.handle.post_torrent_updates();
        let _ = a.prefs.save();
        folder
    })
    .flatten();
    if let Some((save_path, target)) = folder {
        unsafe { show_in_explorer(&target, &save_path) };
    }
}

/// What Open Folder selects: `<save path>\<name>`, or None when the name is more than one plain
/// path component (a magnet's `dn=` is the name until the metadata arrives; SEC-18), so the target
/// can never be a UNC share, another drive or a parent folder. Returns (save path, target).
fn open_folder_target(save_path: &str, name: &str) -> Option<(PathBuf, PathBuf)> {
    use std::path::Component;
    let save = PathBuf::from(save_path);
    if !save.is_absolute() {
        return None;
    }
    let mut parts = Path::new(name).components();
    let target = match (parts.next(), parts.next()) {
        (Some(Component::Normal(n)), None) => save.join(n),
        (None, _) => save.clone(),
        _ => return None,
    };
    Some((save, target))
}

/// Opens Explorer on `target`'s folder with it selected, or inside the save folder while the
/// target does not exist yet (selecting the save folder would show its parent). The shell takes
/// an item or a folder, not a command line, so nothing in the path is parsed as arguments.
unsafe fn show_in_explorer(target: &Path, save_path: &Path) {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows_sys::Win32::UI::Shell::{
        ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems, ShellExecuteW,
    };
    unsafe {
        let com = CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) >= 0;
        let pidl = ILCreateFromPathW(wide(&target.to_string_lossy()).as_ptr());
        if !pidl.is_null() {
            SHOpenFolderAndSelectItems(pidl, 0, null(), 0);
            ILFree(pidl);
        } else {
            let (verb, folder) = (wide("explore"), wide(&save_path.to_string_lossy()));
            ShellExecuteW(null_mut(), verb.as_ptr(), folder.as_ptr(), null(), null(), SW_SHOWNORMAL);
        }
        if com {
            CoUninitialize();
        }
    }
}

unsafe fn remove_selected(hwnd: HWND, delete_data: bool) {
    let ids = selected_ids();
    if ids.is_empty() {
        return;
    }
    let names =
        with_app(|a| ids.iter().filter_map(|id| a.list.get(*id).map(|s| s.name.clone())).collect::<Vec<_>>())
            .unwrap_or_default();
    let question = match (names.as_slice(), delete_data) {
        ([name], false) => tr!(AskRemove, name),
        ([name], true) => tr!(AskRemoveData, name),
        (_, false) => tr!(AskRemoveMany, count = names.len()),
        (_, true) => tr!(AskRemoveDataMany, count = names.len()),
    };
    let detail = if delete_data { tr!(RemoveDeletesData) } else { tr!(RemoveKeepsData) };
    let question = format!("{question}\n{detail}");
    let ask = with_app(|a| a.prefs.get_bool("ui.confirm_delete", true)).unwrap_or(true);
    if ask && !dialogs::confirm(hwnd, &question) {
        return;
    }
    // the part file only holds pieces of skipped files, useless without the torrent (as
    // qBittorrent and nox's Remove do)
    let flags = if delete_data { RemoveFlags::DELETE_FILES } else { RemoveFlags::DELETE_PARTFILE };
    with_app(|a| {
        for id in ids {
            a.handle.remove_torrent(id, flags);
        }
    });
}

/// Adds `.torrent` paths, `magnet:` links and `http(s)://` URLs.
unsafe fn add_sources(hwnd: HWND, sources: &[String]) {
    for s in sources {
        let s = s.trim().trim_matches('"');
        if s.is_empty() {
            continue;
        }
        if s.starts_with("magnet:") {
            add_magnet(hwnd, s);
        } else if s.starts_with("http://") || s.starts_with("https://") {
            download_url(hwnd, s.to_owned());
        } else {
            match std::fs::read(s) {
                Ok(bytes) => add_torrent_file(hwnd, bytes),
                Err(e) => dialogs::error(hwnd, &tr!(ErrOpenFile, path = s, error = e)),
            }
        }
    }
}

fn add_params(_hwnd: HWND, source: TorrentSource, name: Option<String>) {
    with_app(|a| {
        let mut p = AddTorrentParams::new(source, a.save_path.clone());
        p.name = name;
        // Queued and auto-managed: µTorrent's Start (unless Preferences says otherwise).
        p.flags = TorrentFlags::DEFAULT;
        GuiOptions::load(&a.prefs, &a.save_path).apply_to_new(&mut p);
        a.scheduler.apply_to_new(&mut p);
        add_to_session(a, p);
    });
}

/// A magnet link. With the Add New Torrent dialog on, the magnet joins the session in upload
/// mode to fetch its metadata, and the dialog opens once the file list is known
/// ([`add::AwaitingMetadata`], `WM_APP_MAGNET`); otherwise it is added directly.
fn add_magnet(hwnd: HWND, uri: &str) {
    let show = with_app(|a| GuiOptions::load(&a.prefs, &a.save_path).show_add_dialog).unwrap_or(false);
    let key = match patorrent_core::metainfo::parse_magnet_uri(uri) {
        Ok(m) if show => m.info_hashes.key_hex(),
        // the engine reports a broken link
        _ => return add_params(hwnd, TorrentSource::Magnet(uri.to_owned()), None),
    };
    let known = with_app(|a| {
        let st = a.list.ids().find_map(|id| a.list.get(id).filter(|s| s.info_hashes.key_hex() == key));
        st.map(|s| s.name.clone())
    })
    .flatten();
    if let Some(name) = known {
        dialogs::info(hwnd, &tr!(InfoAlreadyInList, name));
        return;
    }
    with_app(|a| {
        let save_path = a.save_path.clone();
        let mut p = AddTorrentParams::new(TorrentSource::Magnet(uri.to_owned()), save_path.clone());
        p.flags = TorrentFlags::DEFAULT;
        GuiOptions::load(&a.prefs, &a.save_path).apply_to_new(&mut p);
        a.scheduler.apply_to_new(&mut p);
        add::AwaitingMetadata::prepare(&mut p);
        a.awaiting.insert(&mut a.prefs, &key, &save_path);
        if let Err(e) = a.prefs.save() {
            a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
        }
        add_to_session(a, p);
    });
}

/// `WM_APP_MAGNET`: the Add New Torrent dialog for each magnet whose metadata arrived, one at a
/// time. OK gives the torrent the choices and starts it (or leaves it stopped); Cancel removes
/// it with its part file (nothing else was written).
unsafe fn magnet_dialogs(hwnd: HWND) {
    loop {
        let next = with_app(|a| {
            if a.awaiting_dialog {
                return None;
            }
            let key = a.awaiting.ready().into_iter().next()?;
            let w = a.awaiting.get(&key)?.clone();
            let gui = GuiOptions::load(&a.prefs, &a.save_path);
            let labels = a.list.labels().into_iter().map(|(l, _)| l).collect();
            let setup = add::Setup::for_magnet(&gui, &a.prefs, labels, &w.save_path);
            a.awaiting_dialog = true;
            Some((key, w, setup))
        })
        .flatten();
        let Some((key, w, setup)) = next else { return };
        let (Some(id), Some(ti)) = (w.id, w.ti.clone()) else { return };
        // the dialog runs its own message loop; nothing is borrowed across it
        let shown = crate::add::show(hwnd, &ti, setup);
        with_app(|a| {
            a.awaiting_dialog = false;
            a.awaiting.finish(&mut a.prefs, &key);
            crate::add::remember(&mut a.prefs, &shown);
            match shown.choice {
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
            if let Err(e) = a.prefs.save() {
                a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
            }
        });
    }
}

/// A `.torrent`'s contents, through the Add New Torrent dialog unless Preferences turned it off
/// (files the engine cannot read are handed to it anyway, so it reports why).
fn add_torrent_file(hwnd: HWND, bytes: Vec<u8>) {
    let Some((show, setup)) = with_app(|a| {
        let gui = GuiOptions::load(&a.prefs, &a.save_path);
        let setup = add::Setup::new(&gui, &a.prefs, a.list.labels().into_iter().map(|(l, _)| l).collect());
        (gui.show_add_dialog, setup)
    }) else {
        return;
    };
    let ti = match TorrentInfo::from_bytes(&bytes) {
        Ok(ti) if show => ti,
        _ => return add_params(hwnd, TorrentSource::TorrentFile(bytes), None),
    };
    let key = ti.info_hashes().key_hex();
    let known =
        with_app(|a| a.list.ids().any(|id| a.list.get(id).is_some_and(|s| s.info_hashes.key_hex() == key)));
    if known == Some(true) {
        dialogs::info(hwnd, &tr!(InfoAlreadyInList, name = ti.name()));
        return;
    }
    // the dialog runs its own message loop; nothing is borrowed across it
    let shown = crate::add::show(hwnd, &ti, setup);
    with_app(|a| {
        crate::add::remember(&mut a.prefs, &shown);
        let Some(choice) = shown.choice else {
            if let Err(e) = a.prefs.save() {
                a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
            }
            return;
        };
        add::remember_dir(&mut a.prefs, &choice.save_path);
        if !choice.label.is_empty() {
            // picked up when the torrent first shows up in the list, like a restored label
            a.prefs.set(&format!("torrent.{key}.label"), &choice.label);
        }
        if choice.top_of_queue {
            a.queue_top.insert(key);
        }
        if let Err(e) = a.prefs.save() {
            a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
        }
        let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(bytes), a.save_path.clone());
        p.flags = TorrentFlags::DEFAULT;
        GuiOptions::load(&a.prefs, &a.save_path).apply_to_new(&mut p);
        a.scheduler.apply_to_new(&mut p);
        choice.apply_to(&ti, &mut p);
        add_to_session(a, p);
    });
}

/// Options → Preferences. OK and Apply store the options and apply them without a restart.
unsafe fn show_preferences(hwnd: HWND) {
    let Some(mut opts) = with_app(|a| Options::load(&a.prefs, &a.save_path)) else { return };
    opts.gui.language = language_choice();
    let dir = data_dir();
    // the dialog runs its own message loop; the hooks borrow the app only while they run
    let hooks = crate::preferences::Hooks {
        apply: Box::new(|o: &Options| {
            with_app(|a| apply_options(a, o));
        }),
        reload_ip_filter: Box::new(|path: &Path| {
            with_app(|a| {
                let r = a.handle.load_ip_filter(path).map_err(|e| e.to_string());
                log_ip_filter(a, path, &r);
                r
            })
            .unwrap_or_else(|| Err(tr!(ErrNotRunning).to_owned()))
        }),
        integration: Box::new(|change| {
            let (keys, exe) = (Keys::user(), exe_path());
            match change {
                None => Ok(integration::read(&keys, &exe)),
                Some((old, new)) => integration::write(&keys, &exe, old, new).map_err(|e| e.to_string()),
            }
        }),
    };
    if crate::preferences::show(hwnd, opts, &dir, hooks) {
        unsafe { update_menu(hwnd) };
    }
}

/// Stores `o` and hands it to the engine and the window.
fn apply_options(a: &mut App, o: &Options) {
    let old = Options::load(&a.prefs, &a.save_path);
    o.store(&mut a.prefs);
    if let Err(e) = a.prefs.save() {
        a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
    }
    if crate::remote_ui::is_remote() && o.gui.language != language_choice() {
        let mut base = Prefs::load(base_data_dir().join("patorrent.ini"));
        match o.gui.language.trim() {
            "" => base.remove("ui.language"),
            code => base.set("ui.language", code),
        }
        if let Err(e) = base.save() {
            a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
        }
    }
    a.save_path = o.gui.download_dir.clone();
    if o.gui.alternate_rows != old.gui.alternate_rows {
        // every list repaints with the new shading (deferred: the state is borrowed here)
        // every list repaints with the new shading (deferred: the state is borrowed here)
        unsafe { RedrawWindow(a.hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN) };
    }
    a.schedule = o.gui.schedule.clone();
    run_scheduler(a, Some(o));
    o.gui.update_move_completed(&mut a.move_completed);
    a.seeding_goal = o.gui.seeding_goal();
    a.torrent_folders = o.gui.torrent_folders.clone();
    a.updates.set_automatic(o.gui.check_updates, std::time::Instant::now());
    a.update_beta = o.gui.update_beta;
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
    let dir = data_dir();
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
    let path = gui.ip_filter_file(&data_dir());
    let r = a.handle.load_ip_filter(&path).map_err(|e| e.to_string());
    log_ip_filter(a, &path, &r);
}

fn log_ip_filter(a: &mut App, path: &Path, r: &Result<usize, String>) {
    let line = match r {
        Ok(n) => format!("{} {}", now_hms(), tr!(LogIpFilterRules, count = n, path = path.display())),
        Err(e) => format!("{} {}", now_hms(), tr!(LogIpFilterError, path = path.display(), error = e)),
    };
    a.detail.push_log(line);
}

/// File → Create New Torrent; with "Start seeding" the new torrent is added with the source's
/// folder as its save path (the files are checked, then it seeds).
unsafe fn create_torrent(hwnd: HWND) {
    let Some(mut r) = with_app(|a| create::Remembered::load(&a.prefs)) else { return };
    // the dialog runs its own message loop; nothing is borrowed across it
    let done = crate::create::show(hwnd, &mut r);
    with_app(|a| r.store(&mut a.prefs));
    let Some(done) = done else { return };
    if let Some(save) = done.seed_from {
        with_app(|a| add_to_session(a, create::seed_params(done.created.bytes, &save)));
    }
}

/// Fetches a `.torrent` over HTTP(S) on a worker thread with the engine's HTTP client
/// (`fetch::http_get`: our User-Agent, redirects followed, through the session's proxy when it
/// carries tracker connections, as the RSS Downloader's fetches do), then posts the result back
/// to the window (`WM_APP_URL`).
fn download_url(hwnd: HWND, url: String) {
    let target = hwnd as isize;
    let proxy = with_app(|a| rss_proxy(a)).flatten();
    std::thread::spawn(move || {
        let data = fetch_torrent(&url, proxy);
        let result = Box::into_raw(Box::new(UrlResult { url, data }));
        unsafe {
            if PostMessageW(target as HWND, WM_APP_URL, 0, result as LPARAM) == 0 {
                drop(Box::from_raw(result));
            }
        }
    });
}

/// `GET url`: the body, or the `magnet:` link an indexer redirected to.
fn fetch_torrent(url: &str, proxy: Option<FetchProxy>) -> Result<UrlBody, String> {
    let opts = patorrent_core::fetch::FetchOptions { proxy, ..Default::default() };
    let r = patorrent_core::fetch::http_get(url, &opts).map_err(|e| e.to_string())?;
    match r.redirect() {
        Some(to) if to.trim_start().starts_with("magnet:") => Ok(UrlBody::Magnet(to.trim().to_owned())),
        // a redirect `fetch` does not follow, to something that is neither http(s) nor magnet
        Some(to) => Err(format!("HTTP {} -> {to}", r.status)),
        None => Ok(UrlBody::Torrent(r.body)),
    }
}

/// `WM_APP_URL`: adds what the download gave, or says why it failed.
fn url_done(hwnd: HWND, r: UrlResult) {
    match r.data {
        Ok(UrlBody::Torrent(bytes)) => add_torrent_file(hwnd, bytes),
        Ok(UrlBody::Magnet(uri)) => add_magnet(hwnd, &uri),
        Err(e) => url_error(hwnd, &tr!(ErrDownloadUrl, url = r.url, error = e)),
    }
}

#[cfg(not(test))]
fn url_error(hwnd: HWND, text: &str) {
    dialogs::error(hwnd, text);
}

#[cfg(test)]
thread_local! {
    /// The errors `url_error` would have shown (no message box in a test).
    static URL_ERRORS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn url_error(_: HWND, text: &str) {
    URL_ERRORS.with(|e| e.borrow_mut().push(text.to_owned()));
}

/// File > Add Torrent from URL end to end, in a hidden main window over the simulated engine:
/// a `.torrent` behind a redirect on a loopback server lands in the list, an unreachable URL
/// reaches the error path.
#[cfg(test)]
mod url_tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    use super::*;

    /// A one-file v1 torrent named `url-test.bin`.
    fn torrent() -> Vec<u8> {
        let mut t = b"d4:infod6:lengthi16384e4:name12:url-test.bin12:piece lengthi16384e6:pieces20:".to_vec();
        t.extend_from_slice(&[7; 20]);
        t.extend_from_slice(b"ee");
        t
    }

    /// Serves `/t` as a redirect to `/file.torrent`, which is the torrent; returns the port and
    /// the request heads it saw.
    fn serve() -> (u16, std::sync::mpsc::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.ends_with(b"\r\n\r\n") {
                    match c.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let reply = if head.starts_with("GET /t ") {
                    b"HTTP/1.1 302 Found\r\nLocation: /file.torrent\r\nContent-Length: 0\r\n\r\n".to_vec()
                } else {
                    let body = torrent();
                    let mut r = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/x-bittorrent\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    r.extend_from_slice(&body);
                    r
                };
                let _ = c.write_all(&reply);
                let _ = tx.send(head);
            }
        });
        (port, rx)
    }

    /// Pumps this thread's messages until `done` or 15 s.
    fn pump_until(mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) {
            unsafe {
                let mut msg: MSG = zeroed();
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn add_torrent_from_url_through_fetch() {
        // keep rss.dat and the rest out of the real %APPDATA%\PaTorrent
        let dir = std::env::temp_dir().join(format!("patorrent-url-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: no other test of this crate reads APPDATA.
        unsafe { std::env::set_var("APPDATA", &dir) };
        let (port, heads) = serve();
        unsafe {
            let icc = INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_WIN95_CLASSES
                    | ICC_BAR_CLASSES
                    | ICC_TAB_CLASSES
                    | ICC_LISTVIEW_CLASSES
                    | ICC_TREEVIEW_CLASSES,
            };
            InitCommonControlsEx(&icc);
            let instance = GetModuleHandleW(null());
            let mut wc: WNDCLASSEXW = zeroed();
            wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
            wc.lpfnWndProc = Some(wndproc);
            wc.hInstance = instance;
            wc.lpszClassName = w!("PaTorrentUrlTestWnd");
            RegisterClassExW(&wc);
            wc.lpfnWndProc = Some(panel_proc);
            wc.lpszClassName = PANEL_CLASS;
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                0,
                w!("PaTorrentUrlTestWnd"),
                w!("PaTorrent URL test"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                0,
                0,
                800,
                600,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!hwnd.is_null());
            let session = Session::start_mock(
                Settings::default(),
                MockConfig { initial_torrents: 0, active_fraction: 0.0, ..MockConfig::default() },
            );
            let mut prefs = Prefs::default();
            prefs.set_bool("ui.show_add_dialog", false);
            prefs.set("dir.download", dir.to_string_lossy());
            init(hwnd, session, prefs);

            // the toolbar's command with a URL typed in is `add_sources`
            add_sources(hwnd, &[format!("http://127.0.0.1:{port}/t")]);
            let names = || {
                with_app(|a| a.list.ids().filter_map(|id| a.list.get(id)).map(|s| s.name.clone()).collect())
                    .unwrap_or_else(Vec::new)
            };
            assert!(pump_until(|| names().iter().any(|n| n == "url-test.bin")), "{:?}", names());
            let first = heads.recv_timeout(Duration::from_secs(5)).unwrap();
            let second = heads.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(first.starts_with("GET /t "), "{first}");
            assert!(second.starts_with("GET /file.torrent "), "the redirect was followed: {second}");
            let ua = format!("\r\nUser-Agent: {}\r\n", patorrent_core::USER_AGENT);
            assert!(first.contains(&ua), "{first}");
            URL_ERRORS.with(|e| assert!(e.borrow().is_empty(), "{:?}", e.borrow()));

            // nothing listens on port 1: the error path, the list unchanged
            add_sources(hwnd, &["http://127.0.0.1:1/x.torrent".to_owned()]);
            assert!(pump_until(|| URL_ERRORS.with(|e| !e.borrow().is_empty())));
            let err = URL_ERRORS.with(|e| e.borrow()[0].clone());
            assert!(err.contains("http://127.0.0.1:1/x.torrent"), "{err}");
            assert_eq!(names().len(), 1);

            DestroyWindow(hwnd);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---- RSS ------------------------------------------------------------------------------------
// The RSS Downloader (`patorrent_frontend::rss`): the timer starts the refreshes that are due,
// the fetches run on the worker's threads and post `WM_APP_RSS`, and `rss_collect` takes their
// results here and adds what the filters chose. The tree's Feeds branch lists the feeds;
// picking one shows its items in the torrent list (µTorrent's way), where a double-click
// downloads one.

/// `rss.dat`, beside `patorrent.ini`.
fn rss_file() -> PathBuf {
    data_dir().join(patorrent_frontend::rss::store::FILE_NAME)
}

/// The proxy feeds and their downloads go through: the engine's, when it carries HTTP
/// tracker connections.
fn rss_proxy(a: &App) -> Option<FetchProxy> {
    FetchProxy::from_settings(&Options::load(&a.prefs, &a.save_path).settings)
}

/// Asks the website for the releases on a worker thread (`update::fetch`, through the proxy
/// the RSS feeds use), which posts the result back (`WM_APP_UPDATE`).
fn start_update_check(hwnd: HWND) {
    let target = hwnd as isize;
    let proxy = with_app(|a| rss_proxy(a)).flatten();
    std::thread::spawn(move || {
        let result = Box::into_raw(Box::new(update::fetch(proxy)));
        unsafe {
            if PostMessageW(target as HWND, WM_APP_UPDATE, 0, result as LPARAM) == 0 {
                drop(Box::from_raw(result));
            }
        }
    });
}

/// `WM_APP_UPDATE`: an automatic check's newer version goes to the status bar for a minute and
/// to the Logger, as does its failure (the Logger only); Help > Check for Updates answers in a
/// message box.
unsafe fn update_done(hwnd: HWND, result: Result<Manifest, String>) {
    let current = Version::current();
    let Some((kind, answer)) = with_app(|a| {
        let (kind, answer) = a.updates.finish(result, &current, a.update_beta, std::time::Instant::now());
        let line = match (&kind, &answer) {
            (Kind::Automatic, Ok(Some(v))) => {
                Some(tr!(LogUpdateAvailable, version = v, url = v.release_url()))
            }
            (Kind::Automatic, Err(e)) => Some(tr!(LogUpdateFailed, error = e)),
            _ => None,
        };
        if let Some(line) = line {
            a.detail.push_log(format!("{} {line}", now_hms()));
        }
        (kind, answer)
    }) else {
        return;
    };
    match (kind, answer) {
        (Kind::Automatic, Ok(Some(_))) => unsafe { show_update_notice(hwnd) },
        (Kind::Automatic, _) => {}
        (Kind::Manual, Ok(Some(v))) => {
            if dialogs::ask(hwnd, &tr!(UpdateNewer, version = v, current)) {
                open_url(hwnd, &v.release_url());
            }
        }
        (Kind::Manual, Ok(None)) => dialogs::info(hwnd, &tr!(UpdateNewest, version = current)),
        (Kind::Manual, Err(e)) => dialogs::error(hwnd, &tr!(UpdateFailed, error = e)),
    }
}

/// Shows or hides the status bar's update notice (`App::updates`): its own first part, with the
/// application icon, before the network part; the other parts move along.
unsafe fn show_update_notice(hwnd: HWND) {
    let Some((status, notice, icon)) = with_app(|a| {
        // every part's text and the network icon are sent again at their new places
        a.status_texts.clear();
        a.net_shown = None;
        (a.status, a.updates.notice().map(|v| tr!(UpdateNotice, version = v)), a.icon_small)
    }) else {
        return;
    };
    unsafe {
        layout(hwnd);
        // the tooltip shows a part's text when it does not fit
        let w = wide(notice.as_deref().unwrap_or_default());
        send(status, SB_SETTIPTEXTW, 0, w.as_ptr() as LPARAM);
        if notice.is_some() {
            send(status, SB_SETTEXTW, 0, w.as_ptr() as LPARAM);
            send(status, SB_SETICON, 0, icon as LPARAM);
        }
        update_status_bar();
    }
}

/// Whether the mouse is over the status bar's update notice (`child` is the window under it).
unsafe fn over_update_notice(child: HWND) -> bool {
    let Some(Some(status)) = with_app(|a| a.updates.notice().is_some().then_some(a.status)) else {
        return false;
    };
    if child != status {
        return false;
    }
    unsafe {
        let mut r: RECT = zeroed();
        send(status, SB_GETRECT, 0, &mut r as *mut RECT as LPARAM);
        let mut p: POINT = zeroed();
        GetCursorPos(&mut p);
        ScreenToClient(status, &mut p);
        PtInRect(&r, p) != 0
    }
}

/// How wide `text` is in `hwnd`'s font.
unsafe fn text_width(hwnd: HWND, text: &str) -> i32 {
    unsafe {
        let dc = GetDC(hwnd);
        let old = SelectObject(dc, send(hwnd, WM_GETFONT, 0, 0) as HGDIOBJ);
        let w: Vec<u16> = text.encode_utf16().collect();
        let mut size: SIZE = zeroed();
        GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32, &mut size);
        SelectObject(dc, old);
        ReleaseDC(hwnd, dc);
        size.cx
    }
}

/// Opens `url` in the default browser.
fn open_url(hwnd: HWND, url: &str) {
    let u = wide(url);
    unsafe { ShellExecuteW(hwnd, w!("open"), u.as_ptr(), null(), null(), SW_SHOWNORMAL) };
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

/// `WM_APP_RSS`: the finished fetches into the downloader, and their torrents into the session.
unsafe fn rss_collect() {
    let Some((changed, logged, dialog)) = with_app(|a| {
        let proxy = rss_proxy(a);
        let c = a.rss_worker.collect(&mut a.rss, unix_now(), proxy.as_ref());
        let logged = !c.log.is_empty();
        for line in c.log {
            a.detail.push_log(format!("{} {line}", now_hms()));
        }
        for (req, d) in c.adds {
            rss_add(a, req, d);
        }
        a.rss_dirty |= c.changed;
        (c.changed, logged, a.rss_dialog)
    }) else {
        return;
    };
    unsafe {
        if changed {
            update_feed_tree();
            refresh_feed_rows();
            if !dialog.is_null() {
                PostMessageW(dialog, crate::rss::WM_RSS_CHANGED, 0, 0);
            }
        }
        if logged {
            update_detail_lists();
        }
    }
}

/// Adds a torrent the downloader asked for: its label, "highest priority" (the top of the
/// queue once added) and "paused state" (µTorrent's Pause) as the Add dialog's would be.
fn rss_add(a: &mut App, req: AddRequest, d: Download) {
    if let Some(key) = d.info_hashes().map(|h| h.key_hex()) {
        if a.list.ids().any(|id| a.list.get(id).is_some_and(|s| s.info_hashes.key_hex() == key)) {
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
    p.flags = TorrentFlags::DEFAULT;
    GuiOptions::load(&a.prefs, &a.save_path).apply_to_new(&mut p);
    a.scheduler.apply_to_new(&mut p);
    req.apply_to(&mut p);
    if magnet {
        // the item's title until the metadata names it
        p.name = Some(req.title.clone());
    }
    add_to_session(a, p);
    if let Err(e) = a.prefs.save() {
        a.detail.push_log(format!("{} {}", now_hms(), tr!(LogPrefsNotSaved, error = e)));
    }
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

/// Shows feed items in the list (`Some(None)`: every feed's) or, with `None`, the torrents.
unsafe fn show_feed_items(view: Option<Option<FeedId>>) {
    unsafe {
        let Some((lv, was)) = with_app(|a| {
            let was = a.feed_view;
            if was.is_none() && view.is_some() {
                // keep the torrent columns' widths and order for the way back
                sync_columns(a);
            }
            a.feed_view = view;
            a.feed_rows = view.map(|f| rss_view::item_rows(&a.rss, f)).unwrap_or_default();
            (a.lv, was)
        }) else {
            return;
        };
        if was.is_some() == view.is_some() {
            if view.is_some() {
                controls::set_state(lv, None, false, false);
                let n = with_app(|a| a.feed_rows.len()).unwrap_or(0);
                controls::set_count(lv, n);
                InvalidateRect(lv, null(), 1);
            }
            return;
        }
        controls::set_state(lv, None, false, false);
        rebuild_columns();
        if view.is_none() {
            refresh_list();
        }
        selection_changed();
        update_toolbar();
    }
}

/// The feed item columns at the window's DPI.
unsafe fn set_feed_columns() {
    let Some((lv, cols, n)) = with_app(|a| {
        let cols: Vec<(&'static str, i32, bool)> = rss_view::ITEM_COLUMNS
            .iter()
            .map(|&(t, w, right)| (lang::text(t), scale(w, a.dpi), right))
            .collect();
        (a.lv, cols, a.feed_rows.len())
    }) else {
        return;
    };
    unsafe {
        controls::set_columns(lv, &cols);
        controls::set_sort_arrow(lv, cols.len(), None, false);
        controls::set_count(lv, n);
        InvalidateRect(lv, null(), 1);
    }
}

/// The feed items again after a fetch (when the list shows them).
unsafe fn refresh_feed_rows() {
    let Some((lv, n)) = with_app(|a| {
        let view = a.feed_view?;
        a.feed_rows = rss_view::item_rows(&a.rss, view);
        Some((a.lv, a.feed_rows.len()))
    })
    .flatten() else {
        return;
    };
    unsafe {
        controls::set_count(lv, n);
        InvalidateRect(lv, null(), 0);
    }
}

/// The tree's Feeds branch: the feeds' names, rebuilt when feeds came or went. A feed shown in
/// the list that went away gives the list back to the torrents.
unsafe fn update_feed_tree() {
    unsafe {
        let Some((tree, feeds, current, view)) = with_app(|a| {
            let feeds: Vec<(FeedId, String)> =
                a.rss.feeds.iter().map(|f| (f.id, f.name().to_owned())).collect();
            (a.tree, feeds, a.feed_tree.clone(), a.feed_view)
        }) else {
            return;
        };
        let set_text = |h: HTREEITEM, text: &str| {
            let mut t = wide(text);
            let mut item: TVITEMW = zeroed();
            item.mask = TVIF_TEXT;
            item.hItem = h;
            item.pszText = t.as_mut_ptr();
            send(tree, TVM_SETITEMW, 0, &item as *const _ as LPARAM);
        };
        let same = current.len() == feeds.len() + 1
            && current.iter().skip(1).zip(&feeds).all(|((_, id), (fid, _))| *id == Some(*fid));
        if same {
            for ((h, _), (_, name)) in current.iter().skip(1).zip(&feeds) {
                set_text(*h, name);
            }
            return;
        }
        with_app(|a| a.feed_tree_busy = true);
        if let Some(&(root, _)) = current.first() {
            send(tree, TVM_DELETEITEM, 0, root);
        }
        let root = insert_tree(tree, TVI_ROOT, tr!(CatFeeds), FEED_PARAM);
        let mut items = vec![(root, None)];
        for (i, (id, name)) in feeds.iter().enumerate() {
            items.push((insert_tree(tree, root, name, FEED_PARAM + i + 1), Some(*id)));
        }
        send(tree, TVM_EXPAND, TVE_EXPAND as usize, root);
        let torrents = with_app(|a| {
            a.feed_tree = items.clone();
            a.feed_tree_busy = false;
            a.tree_items.first().map(|t| t.0)
        })
        .flatten();
        if let Some(shown) = view {
            // select the shown feed's new node, or the torrents when it went away
            match items.iter().find(|(_, f)| *f == shown).map(|x| x.0).or(torrents) {
                Some(h) => {
                    send(tree, TVM_SELECTITEM, TVGN_CARET as usize, h);
                }
                None => show_feed_items(None),
            }
        }
    }
}

/// The right-click menu of the feed item list.
unsafe fn feed_item_menu(hwnd: HWND, p: POINT) {
    unsafe {
        let Some(lv) = with_app(|a| a.lv) else { return };
        let any = !controls::selected(lv).is_empty();
        let menu = CreatePopupMenu();
        append_flags(menu, grayed(!any), cmd::ITEM_DOWNLOAD, tr!(MenuDownload));
        separator(menu);
        append(menu, cmd::RSS, tr!(MenuRss));
        match popup(hwnd, menu, p.x, p.y) {
            cmd::ITEM_DOWNLOAD => download_feed_items(),
            0 => {}
            id => on_command(hwnd, id),
        }
    }
}

/// Downloads the selected feed items (a double-click, or the menu's Download).
unsafe fn download_feed_items() {
    let Some(lv) = with_app(|a| a.lv) else { return };
    let rows = unsafe { controls::selected(lv) };
    with_app(|a| {
        let now = unix_now();
        let picked: Vec<(FeedId, String)> =
            rows.iter().filter_map(|&r| a.feed_rows.get(r)).map(|r| (r.feed, r.id.clone())).collect();
        let reqs: Vec<AddRequest> =
            picked.into_iter().filter_map(|(feed, id)| a.rss.request_item(feed, &id, now)).collect();
        rss_download(a, reqs);
    });
    unsafe {
        refresh_feed_rows();
        update_detail_lists();
    }
}

/// The right-click menu of the tree's Feeds branch (µTorrent's: Add RSS Feed, Edit Feed,
/// Disable Feed, Update Feed, Delete Feed).
unsafe fn tree_menu(hwnd: HWND, tree: HWND, p: POINT, keyboard: bool) {
    unsafe {
        let h = if keyboard {
            send(tree, TVM_GETNEXTITEM, TVGN_CARET as usize, 0) as HTREEITEM
        } else {
            let mut ht: TVHITTESTINFO = zeroed();
            ht.pt = p;
            windows_sys::Win32::Graphics::Gdi::ScreenToClient(tree, &mut ht.pt);
            send(tree, TVM_HITTEST, 0, &mut ht as *mut _ as LPARAM) as HTREEITEM
        };
        let Some((feed, enabled)) = with_app(|a| {
            let feed = a.feed_tree.iter().find(|(x, _)| *x == h)?.1;
            Some((feed, feed.and_then(|id| a.rss.feed(id)).is_none_or(|f| f.enabled)))
        })
        .flatten() else {
            return;
        };
        let menu = CreatePopupMenu();
        append(menu, cmd::FEED_ADD, tr!(MenuAddFeed));
        append_flags(menu, grayed(feed.is_none()), cmd::FEED_EDIT, tr!(MenuEditFeed));
        let toggle = if enabled { tr!(MenuDisableFeed) } else { tr!(MenuEnableFeed) };
        append_flags(menu, grayed(feed.is_none()), cmd::FEED_TOGGLE, toggle);
        append(menu, cmd::FEED_UPDATE, tr!(MenuUpdateFeed));
        append_flags(menu, grayed(feed.is_none()), cmd::FEED_REMOVE, tr!(MenuDeleteFeed));
        separator(menu);
        append(menu, cmd::RSS, tr!(MenuRss));
        let id = popup(hwnd, menu, p.x, p.y);
        feed_command(hwnd, id, feed);
    }
}

/// A Feeds branch command on feed `feed` (`None`: the Feeds node).
unsafe fn feed_command(hwnd: HWND, id: u16, feed: Option<FeedId>) {
    unsafe {
        match id {
            cmd::FEED_ADD => {
                let form =
                    crate::rss::FeedForm { url: crate::rss::url_from_clipboard(hwnd), ..Default::default() };
                let Some(form) = crate::rss::feed_dialog(hwnd, tr!(RssAddFeed), form) else { return };
                with_app(|a| {
                    let id = form.add_to(&mut a.rss);
                    a.rss_dirty = true;
                    rss_update(a, Some(id));
                });
            }
            cmd::FEED_EDIT => {
                let Some(id) = feed else { return };
                let Some(form) = with_app(|a| a.rss.feed(id).map(crate::rss::FeedForm::of)).flatten() else {
                    return;
                };
                let Some(form) = crate::rss::feed_dialog(hwnd, tr!(RssEditFeed), form) else { return };
                with_app(|a| {
                    if form.store(&mut a.rss, id) {
                        rss_update(a, Some(id));
                    }
                    a.rss_dirty = true;
                });
            }
            cmd::FEED_TOGGLE => {
                with_app(|a| {
                    if let Some(f) = feed.and_then(|id| a.rss.feed_mut(id)) {
                        f.enabled = !f.enabled;
                        a.rss_dirty = true;
                    }
                });
            }
            cmd::FEED_UPDATE => {
                with_app(|a| rss_update(a, feed));
            }
            cmd::FEED_REMOVE => {
                let Some(id) = feed else { return };
                let name =
                    with_app(|a| a.rss.feed(id).map(|f| f.name().to_owned())).flatten().unwrap_or_default();
                if !dialogs::confirm(hwnd, &tr!(AskDeleteFeed, name)) {
                    return;
                }
                with_app(|a| {
                    a.rss.remove_feed(id);
                    a.rss_dirty = true;
                });
            }
            cmd::RSS => return show_rss(hwnd, feed),
            _ => return,
        }
        update_feed_tree();
        refresh_feed_rows();
    }
}

/// Options > RSS Downloader. The dialog edits the live downloader; when it closes the filters
/// run over the feeds' current items (they may have changed).
unsafe fn show_rss(hwnd: HWND, feed: Option<FeedId>) {
    // the dialog runs its own message loop; the host borrows the app only while it runs
    crate::rss::show(hwnd, &mut AppHost, feed);
    with_app(|a| {
        let now = unix_now();
        let ids: Vec<FeedId> = a.rss.feeds.iter().map(|f| f.id).collect();
        let reqs: Vec<AddRequest> = ids.into_iter().flat_map(|id| a.rss.process(id, now)).collect();
        rss_download(a, reqs);
        a.rss_dirty = true;
    });
    unsafe {
        update_feed_tree();
        refresh_feed_rows();
        update_detail_lists();
    }
}

/// What the RSS Downloader dialog asks of the window.
struct AppHost;

impl crate::rss::Host for AppHost {
    fn view(&mut self, f: &mut dyn FnMut(&RssDownloader)) {
        with_app(|a| f(&a.rss));
    }

    fn edit(&mut self, f: &mut dyn FnMut(&mut RssDownloader)) {
        with_app(|a| {
            f(&mut a.rss);
            a.rss_dirty = true;
        });
    }

    fn updating(&mut self, id: FeedId) -> bool {
        with_app(|a| a.rss_worker.is_busy(id)).unwrap_or(false)
    }

    fn update(&mut self, id: Option<FeedId>) {
        with_app(|a| rss_update(a, id));
    }

    fn labels(&mut self) -> Vec<String> {
        with_app(|a| a.list.labels().into_iter().map(|(l, _)| l).collect()).unwrap_or_default()
    }

    fn set_dialog(&mut self, dlg: HWND) {
        with_app(|a| a.rss_dialog = dlg);
    }
}

unsafe fn on_notify(hwnd: HWND, nm: *const NMHDR) -> LRESULT {
    unsafe {
        let id = (*nm).idFrom;
        let code = (*nm).code;
        match (id, code) {
            (IDC_LIST, LVN_GETDISPINFOW) => {
                let di = &mut *(nm as *mut NMLVDISPINFOW);
                if di.item.mask & LVIF_IMAGE != 0 {
                    di.item.iImage = with_app(|a| {
                        if a.feed_view.is_some()
                            || a.shown.get(di.item.iSubItem as usize) != Some(&Column::Name)
                        {
                            return None;
                        }
                        let tid = *a.list.view().get(di.item.iItem as usize)?;
                        let st = a.list.get(tid)?;
                        Some(status_icon(st, a.list.extra(tid).user_paused) as i32)
                    })
                    .flatten()
                    .unwrap_or(I_IMAGENONE);
                }
                if di.item.mask & LVIF_TEXT != 0 {
                    let text = with_app(|a| {
                        if a.feed_view.is_some() {
                            let row = a.feed_rows.get(di.item.iItem as usize)?;
                            return row.cells.get(di.item.iSubItem as usize).cloned();
                        }
                        let tid = *a.list.view().get(di.item.iItem as usize)?;
                        let col = *a.shown.get(di.item.iSubItem as usize)?;
                        // The Done cell is painted as a bar (see `list_custom_draw`).
                        if col == Column::Done {
                            return Some(String::new());
                        }
                        Some(col.text(a.list.get(tid)?, a.list.extra(tid)))
                    });
                    // The state is borrowed: a paint that a call made while `App` was held
                    // (a message box, a control updated) asks for cells now. They come out
                    // blank, and a double-buffered list keeps them so until the row's next
                    // repaint, which an idle row may never get. Paint it again once the
                    // borrow is gone (InvalidateRect only marks: the paint is deferred).
                    if text.is_none() {
                        InvalidateRect((*nm).hwndFrom, null(), 0);
                    }
                    let text = text.flatten().unwrap_or_default();
                    copy_to(di.item.pszText, di.item.cchTextMax, &text);
                }
                0
            }
            (IDC_LIST, NM_CUSTOMDRAW) => list_custom_draw(&mut *(nm as *mut NMLVCUSTOMDRAW)),
            (IDC_LIST, LVN_COLUMNCLICK) if with_app(|a| a.feed_view.is_some()) == Some(true) => 0,
            (IDC_LIST, LVN_COLUMNCLICK) => {
                let lv = &*(nm as *const NMLISTVIEW);
                with_app(|a| {
                    if let Some(&col) = a.shown.get(lv.iSubItem as usize) {
                        a.list.toggle_sort(col);
                    }
                });
                if let Some((lvh, n, sort)) = with_app(|a| {
                    let sort =
                        a.list.sort().and_then(|(c, d)| Some((a.shown.iter().position(|&s| s == c)?, d)));
                    (a.lv, a.shown.len(), sort)
                }) {
                    controls::set_sort_arrow(lvh, n, sort.map(|s| s.0), sort.is_some_and(|s| s.1));
                }
                refresh_list();
                0
            }
            (IDC_LIST, LVN_ITEMCHANGED) => {
                let lv = &*(nm as *const NMLISTVIEW);
                if lv.uChanged & LVIF_STATE != 0
                    && (lv.uNewState ^ lv.uOldState) & (LVIS_SELECTED | LVIS_FOCUSED) != 0
                {
                    selection_changed();
                }
                0
            }
            (IDC_LIST, LVN_ODSTATECHANGED) => {
                selection_changed();
                0
            }
            (IDC_STATUS, NM_CLICK) if (*(nm as *const NMMOUSE)).dwItemSpec == 0 => {
                let url = with_app(|a| {
                    let url = a.updates.notice().map(Version::release_url);
                    a.updates.dismiss();
                    url
                });
                if let Some(url) = url.flatten() {
                    open_url(hwnd, &url);
                    show_update_notice(hwnd);
                }
                0
            }
            (IDC_LIST, NM_DBLCLK) if with_app(|a| a.feed_view.is_some()) == Some(true) => {
                download_feed_items();
                0
            }
            (IDC_LIST, NM_DBLCLK) => {
                let ids = selected_ids();
                let done =
                    with_app(|a| ids.first().and_then(|id| a.list.get(*id)).is_some_and(|s| s.is_finished));
                if done == Some(true) {
                    torrent_action(cmd::OPEN_FOLDER);
                } else if !ids.is_empty() {
                    show_properties(hwnd);
                }
                0
            }
            (IDC_TREE, TVN_SELCHANGEDW) => {
                let tv = &*(nm as *const NMTREEVIEWW);
                let idx = tv.itemNew.lParam as usize;
                if with_app(|a| a.feed_tree_busy) != Some(false) {
                    return 0;
                }
                if idx >= FEED_PARAM {
                    let feed = with_app(|a| a.feed_tree.get(idx - FEED_PARAM).map(|f| f.1)).flatten();
                    show_feed_items(Some(feed.flatten()));
                    return 0;
                }
                show_feed_items(None);
                with_app(|a| {
                    if let Some((_, f)) = a.tree_items.get(idx) {
                        let f = f.clone();
                        a.list.set_filter(f);
                    }
                });
                refresh_list();
                selection_changed();
                0
            }
            (IDC_TABS, TCN_SELCHANGE) => {
                let Some(tabs) = with_app(|a| a.tabs) else { return 0 };
                let i = send(tabs, TCM_GETCURSEL, 0, 0);
                if let Some(&p) = Page::ALL.get(i as usize) {
                    select_page(p);
                }
                0
            }
            (IDC_PIECE_LIST, LVN_GETDISPINFOW) => {
                let di = &mut *(nm as *mut NMLVDISPINFOW);
                if di.item.mask & LVIF_TEXT != 0 {
                    let text = with_app(|a| {
                        a.detail.cell(Page::Pieces, di.item.iItem as usize, di.item.iSubItem as usize)
                    })
                    .unwrap_or_default();
                    copy_to(di.item.pszText, di.item.cchTextMax, &text);
                }
                0
            }
            (IDC_PIECE_LIST, NM_CUSTOMDRAW) => pieces_custom_draw(&mut *(nm as *mut NMLVCUSTOMDRAW)),
            (i, NM_CUSTOMDRAW) if (IDC_PAGE..IDC_PAGE + Page::ALL.len()).contains(&i) => {
                controls::selection_custom_draw(&mut *(nm as *mut NMLVCUSTOMDRAW))
            }
            (i, LVN_GETDISPINFOW) if (IDC_PAGE..IDC_PAGE + Page::ALL.len()).contains(&i) => {
                let page = Page::ALL[i - IDC_PAGE];
                let di = &mut *(nm as *mut NMLVDISPINFOW);
                if page == Page::Trackers && di.item.mask & LVIF_IMAGE != 0 && di.item.iSubItem == 0 {
                    di.item.iImage = with_app(|a| {
                        a.detail.trackers.get(di.item.iItem as usize).map(|t| tracker_icon(t.status) as i32)
                    })
                    .flatten()
                    .unwrap_or(I_IMAGENONE);
                }
                if di.item.mask & LVIF_TEXT != 0 {
                    let text =
                        with_app(|a| a.detail.cell(page, di.item.iItem as usize, di.item.iSubItem as usize))
                            .unwrap_or_default();
                    copy_to(di.item.pszText, di.item.cchTextMax, &text);
                }
                0
            }
            _ => 0,
        }
    }
}

unsafe fn select_page(page: Page) {
    unsafe {
        let Some(tabs) = with_app(|a| {
            a.page = page;
            a.tabs
        }) else {
            return;
        };
        send(tabs, TCM_SETCURSEL, page.index(), 0);
        let hwnd = with_app(|a| a.hwnd).unwrap_or(null_mut());
        layout(hwnd);
        request_detail();
        update_detail_lists();
        invalidate_page();
    }
}

/// Paints the selection bar (`controls::selection_pre_paint`) and the Done column as a progress
/// bar with the percentage on it.
unsafe fn list_custom_draw(cd: &mut NMLVCUSTOMDRAW) -> LRESULT {
    const ITEM_SUB_PRE: u32 = CDDS_ITEMPREPAINT | CDDS_SUBITEM;
    const ITEM_SUB_POST: u32 = CDDS_ITEMPOSTPAINT | CDDS_SUBITEM;
    unsafe {
        match cd.nmcd.dwDrawStage {
            CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW as LRESULT,
            CDDS_ITEMPREPAINT => controls::selection_pre_paint(cd) | CDRF_NOTIFYSUBITEMDRAW as LRESULT,
            CDDS_ITEMPOSTPAINT => {
                controls::selection_post_paint(cd);
                CDRF_DODEFAULT as LRESULT
            }
            ITEM_SUB_PRE => {
                controls::selection_cell_colours(cd);
                let is_done = with_app(|a| {
                    a.feed_view.is_none() && a.shown.get(cd.iSubItem as usize) == Some(&Column::Done)
                });
                if is_done == Some(true) {
                    CDRF_NOTIFYPOSTPAINT as LRESULT
                } else {
                    CDRF_DODEFAULT as LRESULT
                }
            }
            ITEM_SUB_POST => {
                let row = cd.nmcd.dwItemSpec;
                let sub = cd.iSubItem as usize;
                let Some((lv, ppm, finished, dpi, font)) = with_app(|a| {
                    let st = a.list.get(*a.list.view().get(row)?)?;
                    Some((a.lv, st.progress_ppm, st.is_finished, a.dpi, a.font))
                })
                .flatten() else {
                    return CDRF_DODEFAULT as LRESULT;
                };
                let hdc = cd.nmcd.hdc;
                let cell = controls::cell_rect(lv, row, sub);
                let m = scale(2, dpi).max(1);
                let bar = RECT {
                    left: cell.left + m,
                    top: cell.top + m,
                    right: cell.right - m,
                    bottom: cell.bottom - m,
                };
                if bar.right <= bar.left {
                    return CDRF_DODEFAULT as LRESULT;
                }
                let frame = CreateSolidBrush(0x00a0a0a0);
                let back = CreateSolidBrush(0x00ffffff);
                // µTorrent: blue while downloading, green once complete.
                let fill = CreateSolidBrush(if finished { 0x0043b54b } else { 0x00d8783c });
                FillRect(hdc, &bar, back);
                let w = ((bar.right - bar.left - 2) as i64 * ppm as i64 / 1_000_000) as i32;
                let done = RECT {
                    left: bar.left + 1,
                    top: bar.top + 1,
                    right: bar.left + 1 + w,
                    bottom: bar.bottom - 1,
                };
                FillRect(hdc, &done, fill);
                FrameRect(hdc, &bar, frame);
                for b in [frame, back, fill] {
                    DeleteObject(b);
                }
                let text = wide(&fmt::percent(ppm));
                let mut r = bar;
                let old = SelectObject(hdc, font);
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, 0);
                DrawTextW(
                    hdc,
                    text.as_ptr(),
                    -1,
                    &mut r,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
                SelectObject(hdc, old);
                CDRF_DODEFAULT as LRESULT
            }
            _ => CDRF_DODEFAULT as LRESULT,
        }
    }
}

/// Draws the pieces list's Blocks column (see [`crate::details::block_bar`]).
unsafe fn pieces_custom_draw(cd: &mut NMLVCUSTOMDRAW) -> LRESULT {
    const ITEM_SUB_PRE: u32 = CDDS_ITEMPREPAINT | CDDS_SUBITEM;
    const ITEM_SUB_POST: u32 = CDDS_ITEMPOSTPAINT | CDDS_SUBITEM;
    unsafe {
        match cd.nmcd.dwDrawStage {
            CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW as LRESULT,
            CDDS_ITEMPREPAINT => controls::selection_pre_paint(cd) | CDRF_NOTIFYSUBITEMDRAW as LRESULT,
            CDDS_ITEMPOSTPAINT => {
                controls::selection_post_paint(cd);
                CDRF_DODEFAULT as LRESULT
            }
            ITEM_SUB_PRE => {
                controls::selection_cell_colours(cd);
                if cd.iSubItem as usize == detail::PIECE_BLOCKS_COLUMN {
                    CDRF_NOTIFYPOSTPAINT as LRESULT
                } else {
                    CDRF_DODEFAULT as LRESULT
                }
            }
            ITEM_SUB_POST => {
                let row = cd.nmcd.dwItemSpec;
                with_app(|a| {
                    let Some(p) = a.detail.partial.get(row) else { return };
                    let cell = controls::cell_rect(a.piece_list, row, detail::PIECE_BLOCKS_COLUMN);
                    let m = scale(3, a.dpi).max(1);
                    let bar = RECT {
                        left: cell.left + m,
                        top: cell.top + m,
                        right: cell.right - m,
                        bottom: cell.bottom - m,
                    };
                    if bar.right > bar.left + 2 && bar.bottom > bar.top + 2 {
                        crate::details::block_bar(cd.nmcd.hdc, &bar, p);
                    }
                });
                CDRF_DODEFAULT as LRESULT
            }
            _ => CDRF_DODEFAULT as LRESULT,
        }
    }
}

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => 1,
            // the Pieces panel's list: placed under the map, its notifications handled with the
            // main window's
            WM_SIZE => {
                if with_app(|a| a.pages[Page::Pieces.index()] == hwnd) == Some(true) {
                    place_piece_list();
                }
                0
            }
            WM_NOTIFY => match with_app(|a| a.hwnd) {
                Some(main) => on_notify(main, lparam as *const NMHDR),
                None => 0,
            },
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc: RECT = zeroed();
                GetClientRect(hwnd, &mut rc);
                // Double-buffered: paint into a bitmap, then copy.
                let mem = CreateCompatibleDC(hdc);
                let bmp = CreateCompatibleBitmap(hdc, rc.right.max(1), rc.bottom.max(1));
                let old = SelectObject(mem, bmp);
                let page = Page::ALL.get(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as usize).copied();
                with_app(|a| {
                    let status = a.detail.id.and_then(|id| a.list.get(id).map(|st| (st, a.list.extra(id))));
                    let ctx = PaintCtx {
                        hdc: mem,
                        rc,
                        dpi: a.dpi,
                        font: a.font,
                        bold: a.bold,
                        status,
                        detail: &a.detail,
                    };
                    if let Some(p) = page {
                        crate::details::paint(p, &ctx);
                    }
                });
                BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
                SelectObject(mem, old);
                DeleteObject(bmp);
                DeleteDC(mem);
                EndPaint(hwnd, &ps);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, WAIT_OBJECT_0};

    use super::*;

    /// Open Folder stays inside the save path whatever the torrent's name is (SEC-18).
    #[test]
    fn open_folder_target_stays_in_the_save_path() {
        let save = r"C:\Downloads";
        let (s, t) = open_folder_target(save, "Some Show").unwrap();
        assert_eq!((s.as_path(), t.as_path()), (Path::new(save), Path::new(r"C:\Downloads\Some Show")));
        assert_eq!(open_folder_target(save, "").unwrap().1, Path::new(save));
        for name in [r"\\attacker\share\x", r"C:\Windows", r"D:x", r"\x", "..", r"..\x", r"a\b", "a/b", "/x"]
        {
            assert_eq!(open_folder_target(save, name), None, "{name}");
        }
        assert_eq!(open_folder_target("relative", "x"), None);
    }

    /// Open Folder as the menu runs it, for a hand check (opens Explorer windows; run with
    /// `--ignored`): an existing item is selected in its folder, a missing one opens the save
    /// folder, and a UNC name opens nothing (SEC-18).
    #[test]
    #[ignore]
    fn open_folder_shows_the_item_in_explorer() {
        let save = std::env::temp_dir().join(format!("patorrent-open-folder-{}", std::process::id()));
        std::fs::create_dir_all(save.join("Some Show")).unwrap();
        let save_s = save.to_string_lossy().into_owned();
        for name in ["Some Show", "Not Yet There"] {
            let (s, t) = open_folder_target(&save_s, name).unwrap();
            unsafe { show_in_explorer(&t, &s) };
        }
        assert_eq!(open_folder_target(&save_s, r"\\attacker.invalid\share\x"), None);
        println!("opened {}", save.display());
    }

    /// A forwarded payload is read only when it is ours and has data (SEC-25).
    #[test]
    fn copydata_args_checks_its_data() {
        let words: Vec<u16> = "magnet:?xt=urn:btih:x\0C:\\a.torrent\0\0".encode_utf16().collect();
        let cds = |data: usize, bytes: u32, ptr: *const u16| COPYDATASTRUCT {
            dwData: data,
            cbData: bytes,
            lpData: ptr as *mut c_void,
        };
        let all = (words.len() * 2) as u32;
        assert_eq!(
            copydata_args(&cds(COPYDATA_ARGS, all, words.as_ptr())),
            Some(vec!["magnet:?xt=urn:btih:x".to_owned(), r"C:\a.torrent".to_owned()])
        );
        assert_eq!(copydata_args(&cds(COPYDATA_ARGS, 0, null())), None, "no data, a null pointer");
        assert_eq!(copydata_args(&cds(COPYDATA_ARGS, all, null())), None, "a size with a null pointer");
        assert_eq!(copydata_args(&cds(COPYDATA_ARGS, 1, words.as_ptr())), None, "less than one character");
        assert_eq!(copydata_args(&cds(7, all, words.as_ptr())), None, "another sender's message");
        let odd = (words.as_ptr() as *const u8).wrapping_add(1) as *const u16;
        assert_eq!(copydata_args(&cds(COPYDATA_ARGS, 4, odd)), None, "misaligned");
    }

    /// `WM_COPYDATA` messages a test window took.
    static COPIED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    unsafe extern "system" fn copy_target(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_COPYDATA {
            COPIED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return 1;
        }
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    /// A hidden top-level window that counts the `WM_COPYDATA` it gets.
    fn copy_window() -> HWND {
        static CLASS: std::sync::Once = std::sync::Once::new();
        let class = w!("PaTorrentCopyDataTest");
        unsafe {
            CLASS.call_once(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(copy_target),
                    hInstance: GetModuleHandleW(null()),
                    lpszClassName: class,
                    ..std::mem::zeroed()
                };
                assert_ne!(RegisterClassW(&wc), 0);
            });
            let hwnd = CreateWindowExW(
                0,
                class,
                w!(""),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                10,
                10,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!hwnd.is_null());
            hwnd
        }
    }

    /// Runs [`copydata_sender`] at low integrity against `hwnd`, pumping this thread's messages
    /// meanwhile; its exit code: 1 delivered, 2 refused by Windows (access denied).
    fn send_from_low_integrity(hwnd: HWND) -> u32 {
        use windows_sys::Win32::Security::{
            DuplicateTokenEx, SID_AND_ATTRIBUTES, SecurityImpersonation, SetTokenInformation,
            TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
            TokenIntegrityLevel, TokenPrimary,
        };
        use windows_sys::Win32::System::Threading::{
            CREATE_NO_WINDOW, CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
            PROCESS_INFORMATION, STARTUPINFOW, WaitForSingleObject,
        };
        // S-1-16-4096, the low mandatory level
        let mut low_sid: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 16, 0x00, 0x10, 0, 0];
        unsafe {
            let (mut own, mut low) = (null_mut(), null_mut());
            let access = TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY;
            assert_ne!(OpenProcessToken(GetCurrentProcess(), access, &mut own), 0);
            assert_ne!(DuplicateTokenEx(own, 0, null(), SecurityImpersonation, TokenPrimary, &mut low), 0);
            CloseHandle(own);
            let label = TOKEN_MANDATORY_LABEL {
                Label: SID_AND_ATTRIBUTES { Sid: low_sid.as_mut_ptr().cast(), Attributes: 0x20 }, // SE_GROUP_INTEGRITY
            };
            let size = (size_of::<TOKEN_MANDATORY_LABEL>() + low_sid.len()) as u32;
            assert_ne!(SetTokenInformation(low, TokenIntegrityLevel, (&raw const label).cast(), size), 0);
            let exe = std::env::current_exe().unwrap();
            let mut cmd = wide(&format!(
                "\"{}\" app::tests::copydata_sender --exact --ignored --test-threads=1 hwnd={}",
                exe.display(),
                hwnd as usize
            ));
            let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..std::mem::zeroed() };
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessAsUserW(
                low,
                null(),
                cmd.as_mut_ptr(),
                null(),
                null(),
                0,
                CREATE_NO_WINDOW,
                null(),
                null(),
                &si,
                &mut pi,
            );
            assert_ne!(ok, 0, "CreateProcessAsUserW: {}", std::io::Error::last_os_error());
            CloseHandle(low);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while WaitForSingleObject(pi.hProcess, 20) != WAIT_OBJECT_0 {
                assert!(std::time::Instant::now() < deadline, "the sender did not finish");
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    DispatchMessageW(&msg);
                }
            }
            let mut code = 0;
            GetExitCodeProcess(pi.hProcess, &mut code);
            CloseHandle(pi.hProcess);
            CloseHandle(pi.hThread);
            code
        }
    }

    /// The low integrity half of [`lower_integrity_processes_cannot_forward`]: sends forwarded
    /// arguments to the window named on the command line and exits with the outcome.
    #[test]
    #[ignore = "started by lower_integrity_processes_cannot_forward"]
    fn copydata_sender() {
        let Some(hwnd) =
            std::env::args().find_map(|a| a.strip_prefix("hwnd=").and_then(|n| n.parse::<usize>().ok()))
        else {
            return;
        };
        let payload: Vec<u16> = "magnet:?xt=urn:btih:x\0\0".encode_utf16().collect();
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_ARGS,
            cbData: (payload.len() * 2) as u32,
            lpData: payload.as_ptr() as *mut c_void,
        };
        let mut result = 0;
        let sent = unsafe {
            SendMessageTimeoutW(
                hwnd as HWND,
                WM_COPYDATA,
                0,
                &raw const cds as LPARAM,
                SMTO_ABORTIFHUNG,
                10_000,
                &mut result,
            )
        };
        let denied = unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
        std::process::exit(if sent != 0 {
            1
        } else if denied {
            2
        } else {
            3
        });
    }

    /// A low integrity process (a sandboxed browser, say) cannot hand the main window sources to
    /// add (SEC-25). The control window, which lifts the filter as the window used to, shows
    /// that the sender does reach a window that lets it.
    #[test]
    fn lower_integrity_processes_cannot_forward() {
        let open = copy_window();
        unsafe { ChangeWindowMessageFilterEx(open, WM_COPYDATA, MSGFLT_ALLOW, null_mut()) };
        let before = COPIED.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(send_from_low_integrity(open), 1, "the control window takes it");
        assert_eq!(COPIED.load(std::sync::atomic::Ordering::SeqCst), before + 1);

        let guarded = copy_window();
        unsafe { refuse_lower_integrity_copydata(guarded) };
        assert_eq!(send_from_low_integrity(guarded), 2, "Windows refuses it");
        assert_eq!(COPIED.load(std::sync::atomic::Ordering::SeqCst), before + 1);
        unsafe {
            DestroyWindow(open);
            DestroyWindow(guarded);
        }
    }

    #[test]
    fn the_command_line_takes_sources_and_switches() {
        let args: Vec<String> =
            ["--MINIMIZED", r"C:\a.torrent", "magnet:?xt=urn:btih:x", "--unknown"].map(String::from).to_vec();
        let a = parse_args(&args);
        assert!(a.minimized && a.demo.is_none());
        assert_eq!(a.sources, [r"C:\a.torrent", "magnet:?xt=urn:btih:x"]);
        // the association's command line: "<exe>" --shell "%1"
        let a = parse_args(&["--shell", "magnet:?xt=urn:btih:y"].map(String::from));
        assert!(!a.minimized);
        assert_eq!(a.sources, ["magnet:?xt=urn:btih:y"]);
        // a link with raw quotes, `magnet:?xt=urn:btih:y" --minimized "\\host\x.torrent`, split
        // by Windows into more arguments: only the first is taken (SEC-18)
        let a = parse_args(
            &["--shell", "magnet:?xt=urn:btih:y", "--minimized", "--remote", r"\\host\x.torrent"]
                .map(String::from),
        );
        assert!(!a.minimized && !a.remote);
        assert_eq!(a.sources, ["magnet:?xt=urn:btih:y"]);
    }

    #[test]
    fn the_auto_shutdown_submenu_carries_one_radio_mark() {
        unsafe {
            let bar = main_menu();
            let id = |i: usize| (cmd::AUTO_SHUTDOWN_BASE + i as u16) as u32;
            let checked = |i: usize| GetMenuState(bar, id(i), MF_BYCOMMAND) & MF_CHECKED != 0;
            for i in 0..AutoMode::MENU.len() {
                assert_ne!(GetMenuState(bar, id(i), MF_BYCOMMAND), u32::MAX, "item {i} is in the menu");
            }
            for pick in [7, 1, 0] {
                check_auto_shutdown(bar, AutoMode::MENU[pick]);
                let marks: Vec<usize> = (0..AutoMode::MENU.len()).filter(|&i| checked(i)).collect();
                assert_eq!(marks, [pick]);
                let mut mii: MENUITEMINFOW = zeroed();
                mii.cbSize = size_of::<MENUITEMINFOW>() as u32;
                mii.fMask = MIIM_FTYPE;
                GetMenuItemInfoW(bar, id(pick), 0, &mut mii);
                assert_ne!(mii.fType & MFT_RADIOCHECK, 0, "a radio mark, not a check mark");
            }
            DestroyMenu(bar);
        }
    }
}
