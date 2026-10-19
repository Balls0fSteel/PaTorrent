//! Options → Preferences: µTorrent 2.2.1's dialog, a tree of pages on the left, the page on the
//! right, OK / Cancel / Apply.
//!
//! One dialog template holds every page; page `p`'s controls have ids `1000 + 100 * p ..`, and
//! choosing a page in the tree shows its range and hides the others. The pages and the
//! Advanced table edit one working [`Options`]: page controls are written from it when shown and
//! read back on Apply (only the controls the user changed, so a value set on the Advanced page
//! that a page shows rounded, such as a rate that is not a whole kB/s, survives).

use std::path::{Path, PathBuf};
use std::ptr::null;

use patorrent_core::{EncPolicy, ProxyType};
use patorrent_frontend::options::{self, Options};
use patorrent_frontend::scheduler::{self, Mode};
use patorrent_frontend::settings::{self, Entry, Kind, Value};
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    BST_CHECKED, BST_UNCHECKED, HTREEITEM, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_STATE, LVIF_TEXT,
    LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETEALLITEMS, LVM_ENSUREVISIBLE, LVM_GETNEXTITEM,
    LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMSTATE, LVM_SETITEMTEXTW,
    LVN_ITEMCHANGED, LVNI_SELECTED, LVS_EX_FULLROWSELECT, LVS_NOSORTHEADER, LVS_REPORT, LVS_SHOWSELALWAYS,
    LVS_SINGLESEL, NMHDR, NMLISTVIEW, NMTREEVIEWW, TVE_EXPAND, TVGN_CARET, TVI_LAST, TVI_ROOT, TVIF_PARAM,
    TVIF_TEXT, TVINSERTSTRUCTW, TVM_EXPAND, TVM_INSERTITEMW, TVM_SELECTITEM, TVN_SELCHANGEDW,
    TVS_DISABLEDRAGDROP, TVS_HASBUTTONS, TVS_HASLINES, TVS_LINESATROOT, TVS_SHOWSELALWAYS,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, BS_GROUPBOX,
    BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, DLGTEMPLATE,
    DialogBoxIndirectParamW, EN_CHANGE, ES_AUTOHSCROLL, ES_NUMBER, ES_PASSWORD, EndDialog, GetDlgItem,
    GetWindowLongPtrW, IDCANCEL, IDOK, MapDialogRect, SW_HIDE, SW_SHOW, SetWindowLongPtrW, SetWindowTextW,
    ShowWindow, WM_COMMAND, WM_INITDIALOG, WM_NOTIFY, WS_BORDER, WS_CHILD, WS_DISABLED, WS_GROUP, WS_TABSTOP,
    WS_VSCROLL,
};

use crate::dialogs::{self, BUTTON, COMBOBOX, Class, DWLP_USER, EDIT, STATIC, Template};
use crate::integration::{Integration, Status};
use crate::sched_grid;
use crate::util::{hiword, loword, send, wide, window_text};

/// The tree, in µTorrent 2.2.1's order; Disk Cache sits under Advanced.
pub const PAGES: [&str; 11] = [
    "General",
    "UI Settings",
    "Directories",
    "Connection",
    "Bandwidth",
    "BitTorrent",
    "Queueing",
    "Scheduler",
    "Web UI",
    "Advanced",
    "Disk Cache",
];
const PAGE_ADVANCED: usize = 9;
const PAGE_DISK_CACHE: usize = 10;

const fn id(page: u16, n: u16) -> u16 {
    1000 + page * 100 + n
}

fn page_of(id: u16) -> Option<usize> {
    (1000..1000 + 100 * PAGES.len() as u16).contains(&id).then(|| (id as usize - 1000) / 100)
}

const ID_TREE: u16 = 900;
const ID_APPLY: u16 = 901;

// General
const ID_LANG: u16 = id(0, 1);
const ID_ASSOC_TORRENT: u16 = id(0, 2);
const ID_ASSOC_MAGNET: u16 = id(0, 3);
const ID_CHECK_ASSOC: u16 = id(0, 4);
const ID_AUTOSTART: u16 = id(0, 5);
const ID_START_MIN: u16 = id(0, 6);
const ID_ASSOC_NOTE: u16 = id(0, 7);
const ID_DEFAULT_APPS: u16 = id(0, 8);
// UI Settings
const ID_CONFIRM_DELETE: u16 = id(1, 1);
const ID_BALLOONS: u16 = id(1, 2);
const ID_CLOSE_TRAY: u16 = id(1, 3);
const ID_MIN_TRAY: u16 = id(1, 4);
const ID_ADD_STOPPED: u16 = id(1, 5);
const ID_SHOW_ADD: u16 = id(1, 6);
// Directories
const ID_DL_DIR: u16 = id(2, 1);
const ID_DL_BROWSE: u16 = id(2, 2);
const ID_MOVE: u16 = id(2, 3);
const ID_MOVE_DIR: u16 = id(2, 4);
const ID_MOVE_BROWSE: u16 = id(2, 5);
const ID_MOVE_ONLY_DEFAULT: u16 = id(2, 6);
const ID_STORE: u16 = id(2, 7);
const ID_STORE_DIR: u16 = id(2, 8);
const ID_STORE_BROWSE: u16 = id(2, 9);
const ID_MOVE_TORRENTS: u16 = id(2, 10);
const ID_MOVE_TORRENTS_DIR: u16 = id(2, 11);
const ID_MOVE_TORRENTS_BROWSE: u16 = id(2, 12);
const ID_AUTOLOAD: u16 = id(2, 13);
const ID_AUTOLOAD_DIR: u16 = id(2, 14);
const ID_AUTOLOAD_BROWSE: u16 = id(2, 15);
const ID_AUTOLOAD_DELETE: u16 = id(2, 16);
// Connection
const ID_PORT: u16 = id(3, 1);
const ID_RANDOM_PORT: u16 = id(3, 2);
const ID_RANDOMIZE: u16 = id(3, 3);
const ID_UPNP: u16 = id(3, 4);
const ID_NATPMP: u16 = id(3, 5);
const ID_PROXY_TYPE: u16 = id(3, 6);
const ID_PROXY_HOST: u16 = id(3, 7);
const ID_PROXY_PORT: u16 = id(3, 8);
const ID_PROXY_AUTH: u16 = id(3, 9);
const ID_PROXY_USER: u16 = id(3, 10);
const ID_PROXY_PASS: u16 = id(3, 11);
const ID_PROXY_NAMES: u16 = id(3, 12);
const ID_PROXY_PEERS: u16 = id(3, 13);
const ID_PROXY_TRACKERS: u16 = id(3, 14);
// Bandwidth
const ID_UL_RATE: u16 = id(4, 1);
const ID_DL_RATE: u16 = id(4, 2);
const ID_CONNECTIONS: u16 = id(4, 3);
const ID_PEERS_PER_TORRENT: u16 = id(4, 4);
const ID_SLOTS_PER_TORRENT: u16 = id(4, 5);
const ID_UNCHOKE: u16 = id(4, 6);
// BitTorrent
const ID_DHT: u16 = id(5, 1);
const ID_LSD: u16 = id(5, 2);
const ID_PEX: u16 = id(5, 3);
const ID_UTP: u16 = id(5, 4);
const ID_ENC_OUT: u16 = id(5, 5);
const ID_ENC_LEGACY: u16 = id(5, 6);
const ID_IPF: u16 = id(5, 7);
const ID_IPF_PATH: u16 = id(5, 8);
const ID_IPF_BROWSE: u16 = id(5, 9);
const ID_IPF_RELOAD: u16 = id(5, 10);
const ID_IPF_STATUS: u16 = id(5, 11);
// Queueing
const ID_ACTIVE: u16 = id(6, 1);
const ID_ACTIVE_DL: u16 = id(6, 2);
const ID_ACTIVE_SEEDS: u16 = id(6, 3);
const ID_SLOW: u16 = id(6, 4);
// the Seeding Goal box
const ID_SEED_RATIO: u16 = id(6, 5);
const ID_SEED_TIME: u16 = id(6, 6);
const ID_SEED_LIMIT: u16 = id(6, 7);
const ID_SEED_LIMIT_KIB: u16 = id(6, 8);
// Advanced
const ID_SCHED: u16 = id(7, 1);
const ID_SCHED_GRID: u16 = id(7, 2);
const ID_SCHED_UL: u16 = id(7, 3);
const ID_SCHED_DL: u16 = id(7, 4);
const ID_SCHED_DHT: u16 = id(7, 5);

const ID_ADV_FILTER: u16 = id(9, 1);
const ID_ADV_LIST: u16 = id(9, 2);
const ID_ADV_VALUE: u16 = id(9, 3);
const ID_ADV_TRUE: u16 = id(9, 4);
const ID_ADV_FALSE: u16 = id(9, 5);
const ID_ADV_SET: u16 = id(9, 6);
const ID_ADV_RESET: u16 = id(9, 7);
// Disk Cache
const ID_DISK_QUEUE: u16 = id(10, 1);
const ID_FILE_POOL: u16 = id(10, 2);
const ID_AIO: u16 = id(10, 3);
const ID_HASH_THREADS: u16 = id(10, 4);

/// "Outgoing" encryption, in the combo's order.
const ENC_OUT: [(&str, EncPolicy); 3] =
    [("Disabled", EncPolicy::Disabled), ("Enabled", EncPolicy::Enabled), ("Forced", EncPolicy::Forced)];

/// The proxy "Type" combo, in µTorrent 2.2.1's order (its "HTTPS" entry, CONNECT for
/// everything, is our HTTP type: plain `http://` requests go as proxy requests, the rest through
/// CONNECT). "Authentication" picks the `_pw` variant; SOCKS4 sends the user name as its user id.
const PROXY_TYPES: [&str; 4] = ["(none)", "Socks4", "Socks5", "HTTP"];

fn proxy_to_ui(s: &patorrent_core::Settings) -> (usize, bool) {
    match s.proxy_type {
        ProxyType::None => (0, false),
        ProxyType::Socks4 => (1, !s.proxy_username.is_empty()),
        ProxyType::Socks5 => (2, false),
        ProxyType::Socks5Pw => (2, true),
        ProxyType::Http => (3, false),
        ProxyType::HttpPw => (3, true),
    }
}

fn proxy_from_ui(kind: usize, auth: bool) -> ProxyType {
    match (kind, auth) {
        (1, _) => ProxyType::Socks4,
        (2, false) => ProxyType::Socks5,
        (2, true) => ProxyType::Socks5Pw,
        (3, false) => ProxyType::Http,
        (3, true) => ProxyType::HttpPw,
        _ => ProxyType::None,
    }
}

/// A page control bound to one value of the options.
enum Field {
    Num { id: u16, min: i64, max: i64, get: fn(&Options) -> Option<i64>, set: fn(&mut Options, i64) },
    Check { id: u16, get: fn(&Options) -> bool, set: fn(&mut Options, bool) },
    Text { id: u16, get: fn(&Options) -> String, set: fn(&mut Options, String) },
}

impl Field {
    fn id(&self) -> u16 {
        match *self {
            Field::Num { id, .. } | Field::Check { id, .. } | Field::Text { id, .. } => id,
        }
    }
}

const KB: i64 = 1024;
const MAX_KB: i64 = u32::MAX as i64 / KB;

static FIELDS: &[Field] = &[
    Field::Check {
        id: ID_CHECK_ASSOC,
        get: |o| o.gui.check_association,
        set: |o, v| o.gui.check_association = v,
    },
    Field::Check { id: ID_SCHED, get: |o| o.gui.schedule.enabled, set: |o, v| o.gui.schedule.enabled = v },
    Field::Num {
        id: ID_SCHED_UL,
        min: 0,
        max: MAX_KB,
        get: |o| Some(o.gui.schedule.limited_up_kib as i64),
        set: |o, v| o.gui.schedule.limited_up_kib = v as u32,
    },
    Field::Num {
        id: ID_SCHED_DL,
        min: 0,
        max: MAX_KB,
        get: |o| Some(o.gui.schedule.limited_down_kib as i64),
        set: |o, v| o.gui.schedule.limited_down_kib = v as u32,
    },
    Field::Check {
        id: ID_SCHED_DHT,
        get: |o| o.gui.schedule.disable_dht,
        set: |o, v| o.gui.schedule.disable_dht = v,
    },
    Field::Check {
        id: ID_CONFIRM_DELETE,
        get: |o| o.gui.confirm_delete,
        set: |o, v| o.gui.confirm_delete = v,
    },
    Field::Check { id: ID_BALLOONS, get: |o| o.gui.balloons, set: |o, v| o.gui.balloons = v },
    Field::Check { id: ID_CLOSE_TRAY, get: |o| o.gui.close_to_tray, set: |o, v| o.gui.close_to_tray = v },
    Field::Check { id: ID_MIN_TRAY, get: |o| o.gui.minimize_to_tray, set: |o, v| o.gui.minimize_to_tray = v },
    Field::Check { id: ID_ADD_STOPPED, get: |o| o.gui.add_stopped, set: |o, v| o.gui.add_stopped = v },
    Field::Check { id: ID_SHOW_ADD, get: |o| o.gui.show_add_dialog, set: |o, v| o.gui.show_add_dialog = v },
    Field::Text { id: ID_DL_DIR, get: |o| o.gui.download_dir.clone(), set: |o, v| o.gui.download_dir = v },
    Field::Check { id: ID_MOVE, get: |o| o.gui.move_completed, set: |o, v| o.gui.move_completed = v },
    Field::Text {
        id: ID_MOVE_DIR,
        get: |o| o.gui.completed_dir.clone(),
        set: |o, v| o.gui.completed_dir = v.trim().to_owned(),
    },
    Field::Check {
        id: ID_MOVE_ONLY_DEFAULT,
        get: |o| o.gui.move_only_default,
        set: |o, v| o.gui.move_only_default = v,
    },
    Field::Check {
        id: ID_STORE,
        get: |o| o.gui.torrent_folders.store,
        set: |o, v| o.gui.torrent_folders.store = v,
    },
    Field::Text {
        id: ID_STORE_DIR,
        get: |o| o.gui.torrent_folders.store_dir.clone(),
        set: |o, v| o.gui.torrent_folders.store_dir = v.trim().to_owned(),
    },
    Field::Check {
        id: ID_MOVE_TORRENTS,
        get: |o| o.gui.torrent_folders.move_finished,
        set: |o, v| o.gui.torrent_folders.move_finished = v,
    },
    Field::Text {
        id: ID_MOVE_TORRENTS_DIR,
        get: |o| o.gui.torrent_folders.finished_dir.clone(),
        set: |o, v| o.gui.torrent_folders.finished_dir = v.trim().to_owned(),
    },
    Field::Check {
        id: ID_AUTOLOAD,
        get: |o| o.gui.torrent_folders.autoload,
        set: |o, v| o.gui.torrent_folders.autoload = v,
    },
    Field::Text {
        id: ID_AUTOLOAD_DIR,
        get: |o| o.gui.torrent_folders.autoload_dir.clone(),
        set: |o, v| o.gui.torrent_folders.autoload_dir = v.trim().to_owned(),
    },
    Field::Check {
        id: ID_AUTOLOAD_DELETE,
        get: |o| o.gui.torrent_folders.autoload_delete,
        set: |o, v| o.gui.torrent_folders.autoload_delete = v,
    },
    Field::Num {
        id: ID_PORT,
        min: 1,
        max: 65535,
        get: |o| settings::listen_port(&o.settings).map(i64::from),
        set: |o, v| settings::set_listen_port(&mut o.settings, v as u16),
    },
    Field::Check { id: ID_RANDOMIZE, get: |o| o.gui.random_port, set: |o, v| o.gui.random_port = v },
    Field::Check { id: ID_UPNP, get: |o| o.settings.enable_upnp, set: |o, v| o.settings.enable_upnp = v },
    Field::Check {
        id: ID_NATPMP,
        get: |o| o.settings.enable_natpmp,
        set: |o, v| o.settings.enable_natpmp = v,
    },
    Field::Text {
        id: ID_PROXY_HOST,
        get: |o| o.settings.proxy_hostname.clone(),
        set: |o, v| o.settings.proxy_hostname = v.trim().to_owned(),
    },
    Field::Num {
        id: ID_PROXY_PORT,
        min: 0,
        max: 65535,
        get: |o| Some(o.settings.proxy_port as i64),
        set: |o, v| o.settings.proxy_port = v as u16,
    },
    Field::Text {
        id: ID_PROXY_USER,
        get: |o| o.settings.proxy_username.clone(),
        set: |o, v| o.settings.proxy_username = v,
    },
    Field::Text {
        id: ID_PROXY_PASS,
        get: |o| o.settings.proxy_password.clone(),
        set: |o, v| o.settings.proxy_password = v,
    },
    Field::Check {
        id: ID_PROXY_NAMES,
        get: |o| o.settings.proxy_hostnames,
        set: |o, v| o.settings.proxy_hostnames = v,
    },
    Field::Check {
        id: ID_PROXY_PEERS,
        get: |o| o.settings.proxy_peer_connections,
        set: |o, v| o.settings.proxy_peer_connections = v,
    },
    Field::Check {
        id: ID_PROXY_TRACKERS,
        get: |o| o.settings.proxy_tracker_connections,
        set: |o, v| o.settings.proxy_tracker_connections = v,
    },
    Field::Num {
        id: ID_UL_RATE,
        min: 0,
        max: MAX_KB,
        get: |o| Some(o.settings.upload_rate_limit as i64 / KB),
        set: |o, v| o.settings.upload_rate_limit = (v * KB) as u32,
    },
    Field::Num {
        id: ID_DL_RATE,
        min: 0,
        max: MAX_KB,
        get: |o| Some(o.settings.download_rate_limit as i64 / KB),
        set: |o, v| o.settings.download_rate_limit = (v * KB) as u32,
    },
    Field::Num {
        id: ID_CONNECTIONS,
        min: 2,
        max: 65535,
        get: |o| Some(o.settings.connections_limit as i64),
        set: |o, v| o.settings.connections_limit = v as i32,
    },
    Field::Num {
        id: ID_PEERS_PER_TORRENT,
        min: 0,
        max: 65535,
        get: |o| Some(o.gui.max_peers_per_torrent as i64),
        set: |o, v| o.gui.max_peers_per_torrent = v as u32,
    },
    Field::Num {
        id: ID_SLOTS_PER_TORRENT,
        min: 0,
        max: 65535,
        get: |o| Some(o.gui.upload_slots_per_torrent as i64),
        set: |o, v| o.gui.upload_slots_per_torrent = v as u32,
    },
    Field::Num {
        id: ID_UNCHOKE,
        min: -1,
        max: 65535,
        get: |o| Some(o.settings.unchoke_slots_limit as i64),
        set: |o, v| o.settings.unchoke_slots_limit = v as i32,
    },
    Field::Check { id: ID_DHT, get: |o| o.settings.enable_dht, set: |o, v| o.settings.enable_dht = v },
    Field::Check { id: ID_LSD, get: |o| o.settings.enable_lsd, set: |o, v| o.settings.enable_lsd = v },
    Field::Check { id: ID_PEX, get: |o| o.gui.pex, set: |o, v| o.gui.pex = v },
    // uTP both ways; the four transports one by one are bt.transp_disposition on Advanced
    Field::Check {
        id: ID_UTP,
        get: |o| o.settings.enable_outgoing_utp || o.settings.enable_incoming_utp,
        set: |o, v| (o.settings.enable_outgoing_utp, o.settings.enable_incoming_utp) = (v, v),
    },
    Field::Check { id: ID_IPF, get: |o| o.gui.ip_filter, set: |o, v| o.gui.ip_filter = v },
    Field::Text {
        id: ID_IPF_PATH,
        get: |o| o.gui.ip_filter_path.clone(),
        set: |o, v| o.gui.ip_filter_path = v.trim().to_owned(),
    },
    Field::Num {
        id: ID_ACTIVE,
        min: -1,
        max: 65535,
        get: |o| Some(o.settings.active_limit as i64),
        set: |o, v| o.settings.active_limit = v as i32,
    },
    Field::Num {
        id: ID_ACTIVE_DL,
        min: -1,
        max: 65535,
        get: |o| Some(o.settings.active_downloads as i64),
        set: |o, v| o.settings.active_downloads = v as i32,
    },
    Field::Num {
        id: ID_ACTIVE_SEEDS,
        min: -1,
        max: 65535,
        get: |o| Some(o.settings.active_seeds as i64),
        set: |o, v| o.settings.active_seeds = v as i32,
    },
    Field::Check {
        id: ID_SLOW,
        get: |o| o.settings.dont_count_slow_torrents,
        set: |o, v| o.settings.dont_count_slow_torrents = v,
    },
    Field::Num {
        id: ID_SEED_RATIO,
        min: 0,
        max: 1_000_000,
        get: |o| Some(o.gui.seed_ratio as i64),
        set: |o, v| o.gui.seed_ratio = v as u32,
    },
    Field::Num {
        id: ID_SEED_TIME,
        min: 0,
        max: 1_000_000,
        get: |o| Some(o.gui.seed_minutes as i64),
        set: |o, v| o.gui.seed_minutes = v as u32,
    },
    Field::Check {
        id: ID_SEED_LIMIT,
        get: |o| o.gui.seed_limit_upload,
        set: |o, v| o.gui.seed_limit_upload = v,
    },
    Field::Num {
        id: ID_SEED_LIMIT_KIB,
        min: 0,
        max: MAX_KB,
        get: |o| Some(o.gui.seed_limit_kib as i64),
        set: |o, v| o.gui.seed_limit_kib = v as u32,
    },
    Field::Num {
        id: ID_DISK_QUEUE,
        min: 1,
        max: MAX_KB,
        get: |o| Some(o.settings.max_queued_disk_bytes as i64 / KB),
        set: |o, v| o.settings.max_queued_disk_bytes = (v * KB) as u32,
    },
    Field::Num {
        id: ID_FILE_POOL,
        min: 1,
        max: 65535,
        get: |o| Some(o.settings.file_pool_size as i64),
        set: |o, v| o.settings.file_pool_size = v as u32,
    },
    Field::Num {
        id: ID_AIO,
        min: 1,
        max: 64,
        get: |o| Some(o.settings.aio_threads as i64),
        set: |o, v| o.settings.aio_threads = v as u32,
    },
    Field::Num {
        id: ID_HASH_THREADS,
        min: 1,
        max: 64,
        get: |o| Some(o.settings.hashing_threads as i64),
        set: |o, v| o.settings.hashing_threads = v as u32,
    },
];

/// Loads a block list: the number of rules or an error.
pub type ReloadIpFilter<'a> = Box<dyn FnMut(&Path) -> Result<usize, String> + 'a>;

/// The Windows integration: `None` reads it, `Some((old, new))` changes it from `old` (as last
/// read) to `new`; either way the registry as found afterwards, or an error.
pub type Integrate<'a> = Box<dyn FnMut(Option<(&Integration, &Integration)>) -> Result<Status, String> + 'a>;

/// What the dialog asks of its owner.
pub struct Hooks<'a> {
    /// OK and Apply: store the options and hand them to the engine.
    pub apply: Box<dyn FnMut(&Options) + 'a>,
    /// "Reload" on the BitTorrent page: load this block list; the number of rules or an error.
    pub reload_ip_filter: ReloadIpFilter<'a>,
    /// The General page's associations and autostart, which live in the registry.
    pub integration: Integrate<'a>,
}

struct State<'a> {
    /// The working copy every page edits.
    opts: Options,
    hooks: Hooks<'a>,
    /// Where `ipfilter.dat` is looked for when no path is set.
    settings_dir: PathBuf,
    page: usize,
    tree_items: Vec<HTREEITEM>,
    /// Each field's control text as last written, to apply only what the user changed.
    shown: Vec<(u16, String)>,
    /// The Advanced list's rows.
    rows: Vec<&'static Entry>,
    /// The options were applied at least once (OK or Apply).
    applied: bool,
    /// The Windows integration as last read.
    integration: Status,
}

/// Shows the dialog modally. Returns whether anything was applied.
pub fn show(owner: HWND, opts: Options, settings_dir: &Path, hooks: Hooks<'_>) -> bool {
    let mut st = State::new(opts, settings_dir, hooks);
    let template = template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(proc_),
            &mut st as *mut State<'_> as LPARAM,
        );
    }
    st.applied
}

impl<'a> State<'a> {
    fn new(opts: Options, settings_dir: &Path, hooks: Hooks<'a>) -> State<'a> {
        State {
            opts,
            hooks,
            settings_dir: settings_dir.to_path_buf(),
            page: 0,
            tree_items: Vec::new(),
            shown: Vec::new(),
            rows: Vec::new(),
            applied: false,
            integration: Status::default(),
        }
    }
}

/// Left edge and top of the page area, and its width (dialog units).
const X0: i16 = 114;
const Y0: i16 = 7;
const W: i16 = 310;

const C: u32 = WS_CHILD;
const TAB: u32 = C | WS_TABSTOP;
const OFF: u32 = WS_DISABLED;
const LINE: u32 = TAB | WS_BORDER | ES_AUTOHSCROLL as u32;
const NUMBER: u32 = LINE | ES_NUMBER as u32;
const CHECK: u32 = TAB | BS_AUTOCHECKBOX as u32;
const PUSH: u32 = TAB | BS_PUSHBUTTON as u32;
const COMBO: u32 = TAB | WS_VSCROLL | CBS_DROPDOWNLIST as u32;

/// Places a page's controls relative to the page area and numbers its labels.
struct Builder {
    t: Template,
    page: u16,
    next_label: u16,
}

impl Builder {
    fn page(&mut self, page: usize) {
        self.page = page as u16;
        self.next_label = 50;
    }

    fn ctl(&mut self, class: Class, id: u16, style: u32, [x, y, w, h]: [i16; 4], text: &str) {
        self.t.item(class, id, style, [X0 + x, Y0 + y, w, h], text);
    }

    fn label_id(&mut self) -> u16 {
        self.next_label += 1;
        id(self.page, self.next_label)
    }

    fn group(&mut self, rect: [i16; 4], text: &str) {
        let id = self.label_id();
        self.ctl(BUTTON, id, C | BS_GROUPBOX as u32, rect, text);
    }

    fn label(&mut self, rect: [i16; 4], text: &str) {
        let id = self.label_id();
        self.ctl(STATIC, id, C, rect, text);
    }

    /// A control that is a placeholder until its feature exists.
    fn stub(&mut self, class: Class, style: u32, rect: [i16; 4], text: &str) {
        let id = self.label_id();
        self.ctl(class, id, style | OFF, rect, text);
    }

    /// A label and a number field on one row of a group.
    fn number(&mut self, y: i16, id: u16, style: u32, text: &str) {
        self.label([8, y + 2, 228, 9], text);
        self.ctl(EDIT, id, style, [240, y, 54, 13], "");
    }
}

fn template() -> Vec<u16> {
    // the Scheduler page names the grid's class
    sched_grid::register();
    let mut b = Builder { t: Template::new(431, 271), page: 0, next_label: 50 };
    let tree_style = TAB
        | WS_BORDER
        | windows_sys::Win32::UI::WindowsAndMessaging::WS_VISIBLE
        | TVS_HASBUTTONS
        | TVS_HASLINES
        | TVS_LINESATROOT
        | TVS_SHOWSELALWAYS
        | TVS_DISABLEDRAGDROP;
    b.t.item(Class::Name("SysTreeView32"), ID_TREE, tree_style, [7, 7, 100, 236], "");

    b.page(0);
    b.group([0, 0, W, 34], "Language");
    b.label([8, 15, 60, 9], "User interface:");
    b.ctl(COMBOBOX, ID_LANG, COMBO | OFF, [72, 13, 120, 60], "");
    b.group([0, 40, W, 112], "Windows Integration");
    b.ctl(BUTTON, ID_ASSOC_TORRENT, CHECK, [8, 53, 290, 10], "Associate with .torrent files");
    b.ctl(BUTTON, ID_ASSOC_MAGNET, CHECK, [8, 67, 290, 10], "Associate with magnet: links");
    b.ctl(BUTTON, ID_CHECK_ASSOC, CHECK, [8, 81, 290, 10], "Check association on startup");
    b.ctl(BUTTON, ID_AUTOSTART, CHECK, [8, 95, 290, 10], "Start PaTorrent on system startup");
    b.ctl(BUTTON, ID_START_MIN, CHECK, [20, 109, 278, 10], "Start minimized");
    b.ctl(STATIC, ID_ASSOC_NOTE, C, [8, 126, 226, 18], "");
    b.ctl(BUTTON, ID_DEFAULT_APPS, PUSH, [240, 127, 62, 14], "Default apps...");

    b.page(1);
    b.group([0, 0, W, 46], "Display Options");
    b.ctl(BUTTON, ID_CONFIRM_DELETE, CHECK, [8, 14, 290, 10], "Confirm when deleting torrents");
    b.ctl(BUTTON, ID_BALLOONS, CHECK, [8, 28, 290, 10], "Show balloon notifications in tray");
    b.group([0, 52, W, 46], "System Tray");
    b.ctl(BUTTON, ID_CLOSE_TRAY, CHECK, [8, 66, 290, 10], "Close to tray");
    b.ctl(BUTTON, ID_MIN_TRAY, CHECK, [8, 80, 290, 10], "Minimize to tray");
    b.group([0, 104, W, 46], "When Adding Torrents");
    b.ctl(BUTTON, ID_ADD_STOPPED, CHECK, [8, 118, 290, 10], "Don't start the download automatically");
    b.ctl(
        BUTTON,
        ID_SHOW_ADD,
        CHECK,
        [8, 132, 290, 10],
        "Show a window that displays the files inside the torrent",
    );

    b.page(2);
    b.group([0, 0, W, 96], "Location of Downloaded Files");
    b.label([8, 14, 290, 9], "Put new downloads in:");
    b.ctl(EDIT, ID_DL_DIR, LINE, [8, 26, 272, 13], "");
    b.ctl(BUTTON, ID_DL_BROWSE, PUSH, [284, 26, 18, 13], "...");
    b.ctl(BUTTON, ID_MOVE, CHECK, [8, 46, 290, 10], "Move completed downloads to:");
    b.ctl(EDIT, ID_MOVE_DIR, LINE, [8, 59, 272, 13], "");
    b.ctl(BUTTON, ID_MOVE_BROWSE, PUSH, [284, 59, 18, 13], "...");
    b.ctl(
        BUTTON,
        ID_MOVE_ONLY_DEFAULT,
        CHECK,
        [8, 78, 290, 10],
        "Only move from the default download directory",
    );
    b.group([0, 102, W, 128], "Location of .torrents");
    b.ctl(BUTTON, ID_STORE, CHECK, [8, 116, 290, 10], "Store .torrents in:");
    b.ctl(EDIT, ID_STORE_DIR, LINE, [8, 129, 272, 13], "");
    b.ctl(BUTTON, ID_STORE_BROWSE, PUSH, [284, 129, 18, 13], "...");
    b.ctl(BUTTON, ID_MOVE_TORRENTS, CHECK, [20, 148, 278, 10], "Move .torrents for finished jobs to:");
    b.ctl(EDIT, ID_MOVE_TORRENTS_DIR, LINE, [20, 161, 260, 13], "");
    b.ctl(BUTTON, ID_MOVE_TORRENTS_BROWSE, PUSH, [284, 161, 18, 13], "...");
    b.ctl(BUTTON, ID_AUTOLOAD, CHECK, [8, 181, 290, 10], "Automatically load .torrents from:");
    b.ctl(EDIT, ID_AUTOLOAD_DIR, LINE, [8, 194, 272, 13], "");
    b.ctl(BUTTON, ID_AUTOLOAD_BROWSE, PUSH, [284, 194, 18, 13], "...");
    b.ctl(BUTTON, ID_AUTOLOAD_DELETE, CHECK, [20, 213, 278, 10], "Delete loaded .torrents");

    b.page(3);
    b.group([0, 0, W, 78], "Listening Port");
    b.label([8, 15, 128, 9], "Port used for incoming connections:");
    b.ctl(EDIT, ID_PORT, NUMBER, [140, 13, 40, 13], "");
    b.ctl(BUTTON, ID_RANDOM_PORT, PUSH, [186, 12, 60, 14], "Random port");
    b.ctl(BUTTON, ID_RANDOMIZE, CHECK, [8, 32, 290, 10], "Randomize port each start");
    b.ctl(BUTTON, ID_UPNP, CHECK, [8, 46, 290, 10], "Enable UPnP port mapping");
    b.ctl(BUTTON, ID_NATPMP, CHECK, [8, 60, 290, 10], "Enable NAT-PMP port mapping");
    b.group([0, 84, W, 118], "Proxy Server");
    b.label([8, 98, 30, 9], "Type:");
    b.ctl(COMBOBOX, ID_PROXY_TYPE, COMBO, [40, 96, 70, 60], "");
    b.label([118, 98, 24, 9], "Proxy:");
    b.ctl(EDIT, ID_PROXY_HOST, LINE, [144, 96, 100, 13], "");
    b.label([250, 98, 18, 9], "Port:");
    b.ctl(EDIT, ID_PROXY_PORT, NUMBER, [270, 96, 32, 13], "");
    b.ctl(BUTTON, ID_PROXY_AUTH, CHECK, [8, 114, 290, 10], "Authentication");
    b.label([8, 130, 40, 9], "Username:");
    b.ctl(EDIT, ID_PROXY_USER, LINE, [50, 128, 90, 13], "");
    b.label([150, 130, 40, 9], "Password:");
    b.ctl(EDIT, ID_PROXY_PASS, LINE | ES_PASSWORD as u32, [192, 128, 110, 13], "");
    b.ctl(BUTTON, ID_PROXY_NAMES, CHECK, [8, 146, 290, 10], "Resolve hostnames through proxy");
    b.ctl(BUTTON, ID_PROXY_PEERS, CHECK, [8, 160, 290, 10], "Use proxy for peer-to-peer connections");
    b.ctl(BUTTON, ID_PROXY_TRACKERS, CHECK, [8, 174, 290, 10], "Use proxy for tracker connections");

    b.page(4);
    b.group([0, 0, W, 32], "Global Upload Rate Limiting");
    b.number(13, ID_UL_RATE, NUMBER, "Maximum upload rate (kB/s): [0: unlimited]");
    b.group([0, 38, W, 32], "Global Download Rate Limiting");
    b.number(51, ID_DL_RATE, NUMBER, "Maximum download rate (kB/s): [0: unlimited]");
    b.group([0, 76, W, 82], "Number of Connections");
    b.number(89, ID_CONNECTIONS, NUMBER, "Global maximum number of connections:");
    b.number(
        105,
        ID_PEERS_PER_TORRENT,
        NUMBER,
        "Maximum number of connected peers per torrent: [0: no limit]",
    );
    b.number(121, ID_SLOTS_PER_TORRENT, NUMBER, "Number of upload slots per torrent: [0: no limit]");
    b.number(137, ID_UNCHOKE, LINE, "Global number of upload slots: [-1: no limit]");
    b.label([0, 164, W, 9], "The per-torrent limits apply to torrents added from now on.");

    b.page(5);
    b.group([0, 0, W, 72], "Basic BitTorrent Features");
    b.ctl(BUTTON, ID_DHT, CHECK, [8, 13, 290, 10], "Enable DHT Network");
    b.ctl(BUTTON, ID_LSD, CHECK, [8, 27, 290, 10], "Enable Local Peer Discovery");
    b.ctl(BUTTON, ID_PEX, CHECK, [8, 41, 290, 10], "Enable Peer Exchange");
    b.ctl(BUTTON, ID_UTP, CHECK, [8, 55, 290, 10], "Enable uTP (UDP transport with bandwidth management)");
    b.group([0, 78, W, 46], "Protocol Encryption");
    b.label([8, 93, 36, 9], "Outgoing:");
    b.ctl(COMBOBOX, ID_ENC_OUT, COMBO, [46, 91, 80, 60], "");
    b.ctl(BUTTON, ID_ENC_LEGACY, CHECK, [8, 108, 290, 10], "Allow incoming legacy connections");
    b.group([0, 130, W, 62], "IP Filter");
    b.ctl(BUTTON, ID_IPF, CHECK, [8, 143, 290, 10], "Enable IP filter");
    b.ctl(EDIT, ID_IPF_PATH, LINE, [8, 157, 224, 13], "");
    b.ctl(BUTTON, ID_IPF_BROWSE, PUSH, [236, 157, 18, 13], "...");
    b.ctl(BUTTON, ID_IPF_RELOAD, PUSH, [258, 156, 44, 14], "Reload");
    b.ctl(STATIC, ID_IPF_STATUS, C, [8, 175, 294, 9], "");

    b.page(6);
    b.group([0, 0, W, 78], "Queue Settings");
    b.number(13, ID_ACTIVE, LINE, "Maximum number of active torrents (upload or download): [-1: no limit]");
    b.number(29, ID_ACTIVE_DL, LINE, "Maximum number of active downloads: [-1: no limit]");
    b.number(45, ID_ACTIVE_SEEDS, LINE, "Maximum number of active seeds: [-1: no limit]");
    b.ctl(BUTTON, ID_SLOW, CHECK, [8, 62, 290, 10], "Don't count slow torrents toward these limits");
    b.group([0, 84, W, 92], "Seeding Goal");
    b.number(97, ID_SEED_RATIO, NUMBER, "Minimum ratio (%): [0: no ratio goal]");
    b.number(113, ID_SEED_TIME, NUMBER, "Minimum seeding time (minutes): [0: no time goal]");
    b.label([8, 132, 294, 9], "When a torrent reaches the seeding goal (forced torrents never do):");
    b.ctl(BUTTON, ID_SEED_LIMIT, CHECK, [8, 147, 228, 10], "Limit the upload rate to (kB/s): [0: stop]");
    b.ctl(EDIT, ID_SEED_LIMIT_KIB, NUMBER, [240, 145, 54, 13], "");

    b.page(7);
    b.ctl(BUTTON, ID_SCHED, CHECK, [0, 0, 200, 10], "Enable Scheduler");
    b.group([0, 14, W, 132], "Scheduler Table");
    b.ctl(Class::Name(crate::sched_grid::CLASS), ID_SCHED_GRID, C, [6, 25, W - 12, 117], "");
    b.group([0, 152, W, 64], "Scheduler Settings");
    b.number(165, ID_SCHED_UL, NUMBER, "Limited upload rate (kB/s): [0: unlimited]");
    b.number(181, ID_SCHED_DL, NUMBER, "Limited download rate (kB/s): [0: unlimited]");
    b.ctl(BUTTON, ID_SCHED_DHT, CHECK, [8, 199, 290, 10], "Disable DHT when turning off");

    b.page(8);
    b.stub(BUTTON, CHECK, [0, 0, 200, 10], "Enable Web UI");
    b.group([0, 16, W, 64], "Authentication");
    b.label([8, 31, 50, 9], "Username:");
    b.stub(EDIT, LINE, [60, 29, 120, 13], "");
    b.label([8, 47, 50, 9], "Password:");
    b.stub(EDIT, LINE, [60, 45, 120, 13], "");
    b.label([8, 64, 294, 9], "The Web UI comes with patorrent-nox.");

    b.page(PAGE_ADVANCED);
    b.label([0, 2, 24, 9], "Filter:");
    b.ctl(EDIT, ID_ADV_FILTER, LINE, [28, 0, W - 28, 13], "");
    let list = TAB | WS_BORDER | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_NOSORTHEADER;
    b.ctl(Class::Name("SysListView32"), ID_ADV_LIST, list, [0, 18, W, 172], "");
    b.label([0, 199, 26, 9], "Value:");
    b.ctl(EDIT, ID_ADV_VALUE, LINE, [28, 197, 170, 13], "");
    b.ctl(BUTTON, ID_ADV_TRUE, TAB | WS_GROUP | BS_AUTORADIOBUTTON as u32, [28, 198, 40, 10], "True");
    b.ctl(BUTTON, ID_ADV_FALSE, C | BS_AUTORADIOBUTTON as u32, [72, 198, 40, 10], "False");
    b.ctl(BUTTON, ID_ADV_SET, PUSH | WS_GROUP, [206, 196, 50, 14], "Set");
    b.ctl(BUTTON, ID_ADV_RESET, PUSH, [260, 196, 50, 14], "Reset");
    b.label(
        [0, 216, W, 18],
        "libtorrent setting names; * marks a changed value. Values are applied with OK or Apply.",
    );

    b.page(PAGE_DISK_CACHE);
    b.group([0, 0, W, 80], "Disk Queue and Files");
    b.number(13, ID_DISK_QUEUE, NUMBER, "Disk write queue (kB):");
    b.number(29, ID_FILE_POOL, NUMBER, "Maximum number of open files:");
    b.number(45, ID_AIO, NUMBER, "Disk I/O threads (used from the next start):");
    b.number(61, ID_HASH_THREADS, NUMBER, "Hashing threads (used from the next start):");
    b.label(
        [0, 88, W, 27],
        "PaTorrent has no disk read/write cache yet, so µTorrent's cache size settings have no \
         counterpart; these are the limits of the engine's disk queue.",
    );

    let mut t = b.t;
    let vis = windows_sys::Win32::UI::WindowsAndMessaging::WS_VISIBLE;
    t.item(BUTTON, IDOK as u16, TAB | vis | BS_DEFPUSHBUTTON as u32, [263, 250, 50, 14], "OK");
    t.item(BUTTON, IDCANCEL as u16, PUSH | vis, [317, 250, 50, 14], "Cancel");
    t.item(BUTTON, ID_APPLY, PUSH | vis, [371, 250, 53, 14], "&Apply");
    t.finish()
}

unsafe fn item(dlg: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(dlg, id as i32) }
}

unsafe fn set_text(hwnd: HWND, s: &str) {
    let w = wide(s);
    unsafe { SetWindowTextW(hwnd, w.as_ptr()) };
}

unsafe fn checked(dlg: HWND, id: u16) -> bool {
    unsafe { send(item(dlg, id), BM_GETCHECK, 0, 0) == BST_CHECKED as isize }
}

unsafe fn set_check(dlg: HWND, id: u16, on: bool) {
    unsafe { send(item(dlg, id), BM_SETCHECK, if on { BST_CHECKED } else { BST_UNCHECKED } as usize, 0) };
}

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            init(dlg, &mut *(lp as *mut State<'_>));
            return 1;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut State<'_>;
        if st.is_null() {
            return 0;
        }
        let st = &mut *st;
        match msg {
            WM_COMMAND => command(dlg, st, loword(wp), hiword(wp) as u32) as isize,
            WM_NOTIFY => {
                notify(dlg, st, &*(lp as *const NMHDR));
                0
            }
            _ => 0,
        }
    }
}

unsafe fn init(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        set_text(dlg, "Preferences");
        let tree = item(dlg, ID_TREE);
        let mut parent = TVI_ROOT;
        for (i, title) in PAGES.iter().enumerate() {
            let under = if i == PAGE_DISK_CACHE { parent } else { TVI_ROOT };
            let h = insert_tree(tree, under, title, i);
            if i == PAGE_ADVANCED {
                parent = h;
            }
            st.tree_items.push(h);
        }
        send(tree, TVM_EXPAND, TVE_EXPAND as usize, parent);

        let lang = wide("English");
        send(item(dlg, ID_LANG), CB_ADDSTRING, 0, lang.as_ptr() as LPARAM);
        send(item(dlg, ID_LANG), CB_SETCURSEL, 0, 0);
        for label in PROXY_TYPES {
            let w = wide(label);
            send(item(dlg, ID_PROXY_TYPE), CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        for (label, _) in ENC_OUT {
            let w = wide(label);
            send(item(dlg, ID_ENC_OUT), CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }

        let lv = item(dlg, ID_ADV_LIST);
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, LVS_EX_FULLROWSELECT as usize, LVS_EX_FULLROWSELECT as isize);
        // columns in dialog units, like the rest of the layout
        let mut rc = RECT { left: 0, top: 0, right: 196, bottom: 0 };
        MapDialogRect(dlg, &mut rc);
        let name_w = rc.right;
        rc = RECT { left: 0, top: 0, right: 96, bottom: 0 };
        MapDialogRect(dlg, &mut rc);
        for (i, (title, width)) in [("Name", name_w), ("Value", rc.right)].into_iter().enumerate() {
            let mut text = wide(title);
            let mut c: LVCOLUMNW = std::mem::zeroed();
            c.mask = LVCF_TEXT | LVCF_WIDTH;
            c.cx = width;
            c.pszText = text.as_mut_ptr();
            send(lv, LVM_INSERTCOLUMNW, i, &c as *const _ as LPARAM);
        }

        st.integration = (st.hooks.integration)(None).unwrap_or_default();
        write_controls(dlg, st);
        fill_advanced(dlg, st, None);
        send(tree, TVM_SELECTITEM, TVGN_CARET as usize, st.tree_items[0]);
        show_page(dlg, st, 0);
    }
}

unsafe fn insert_tree(tree: HWND, parent: HTREEITEM, text: &str, page: usize) -> HTREEITEM {
    let mut t = wide(text);
    unsafe {
        let mut ins: TVINSERTSTRUCTW = std::mem::zeroed();
        ins.hParent = parent;
        ins.hInsertAfter = TVI_LAST;
        ins.Anonymous.item.mask = TVIF_TEXT | TVIF_PARAM;
        ins.Anonymous.item.pszText = t.as_mut_ptr();
        ins.Anonymous.item.lParam = page as isize;
        send(tree, TVM_INSERTITEMW, 0, &ins as *const _ as LPARAM)
    }
}

/// Shows page `page`'s controls and hides every other page's.
unsafe fn show_page(dlg: HWND, st: &mut State<'_>, page: usize) {
    unsafe {
        st.page = page;
        for p in 0..PAGES.len() {
            let cmd = if p == page { SW_SHOW } else { SW_HIDE };
            for n in 0..100 {
                let h = item(dlg, id(p as u16, n));
                if !h.is_null() {
                    ShowWindow(h, cmd);
                }
            }
        }
        if page == PAGE_ADVANCED {
            show_value_controls(dlg, st);
        }
    }
}

/// Selects `page` in the tree (which shows it).
unsafe fn select_page(dlg: HWND, st: &mut State<'_>, page: usize) {
    unsafe {
        // shown first, so the tree's change notification finds the page already current
        show_page(dlg, st, page);
        send(item(dlg, ID_TREE), TVM_SELECTITEM, TVGN_CARET as usize, st.tree_items[page]);
    }
}

/// Field controls from the working options; remembers what was written.
unsafe fn write_controls(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        st.shown.clear();
        for f in FIELDS {
            let text = match f {
                Field::Num { id, get, .. } => {
                    let s = get(&st.opts).map(|v| v.to_string()).unwrap_or_default();
                    set_text(item(dlg, *id), &s);
                    s
                }
                Field::Check { id, get, .. } => {
                    set_check(dlg, *id, get(&st.opts));
                    get(&st.opts).to_string()
                }
                Field::Text { id, get, .. } => {
                    let s = get(&st.opts);
                    set_text(item(dlg, *id), &s);
                    s
                }
            };
            st.shown.push((f.id(), text));
        }
        let enc = ENC_OUT.iter().position(|(_, p)| *p == st.opts.settings.out_enc_policy).unwrap_or(1);
        send(item(dlg, ID_ENC_OUT), CB_SETCURSEL, enc, 0);
        st.shown.push((ID_ENC_OUT, enc.to_string()));
        let legacy = settings::allow_legacy_incoming(&st.opts.settings);
        set_check(dlg, ID_ENC_LEGACY, legacy);
        st.shown.push((ID_ENC_LEGACY, legacy.to_string()));
        let (kind, auth) = proxy_to_ui(&st.opts.settings);
        send(item(dlg, ID_PROXY_TYPE), CB_SETCURSEL, kind, 0);
        st.shown.push((ID_PROXY_TYPE, kind.to_string()));
        set_check(dlg, ID_PROXY_AUTH, auth);
        st.shown.push((ID_PROXY_AUTH, auth.to_string()));
        let table = st.opts.gui.schedule.table;
        send(item(dlg, ID_SCHED_GRID), sched_grid::SET, 0, &table as *const _ as LPARAM);
        st.shown.push((ID_SCHED_GRID, st.opts.gui.schedule.table_text()));
        update_ip_filter_status(dlg, st, None);
        write_integration(dlg, st);
        update_move_controls(dlg);
        update_torrent_folder_controls(dlg);
        update_sched_controls(dlg);
    }
}

/// The General page's boxes from the registry as last read.
unsafe fn write_integration(dlg: HWND, st: &State<'_>) {
    unsafe {
        let set = st.integration.set;
        set_check(dlg, ID_ASSOC_TORRENT, set.torrent);
        set_check(dlg, ID_ASSOC_MAGNET, set.magnet);
        set_check(dlg, ID_AUTOSTART, set.autostart);
        set_check(dlg, ID_START_MIN, set.minimized);
        let note = st.integration.note();
        let note = if note.is_empty() { note } else { format!("{note} Choose PaTorrent in Default apps.") };
        set_text(item(dlg, ID_ASSOC_NOTE), &note);
        update_autostart_controls(dlg);
    }
}

/// The boxes as edited.
unsafe fn integration_controls(dlg: HWND) -> Integration {
    unsafe {
        Integration {
            torrent: checked(dlg, ID_ASSOC_TORRENT),
            magnet: checked(dlg, ID_ASSOC_MAGNET),
            autostart: checked(dlg, ID_AUTOSTART),
            minimized: checked(dlg, ID_START_MIN),
        }
        .normalized()
    }
}

/// "Start minimized" follows "Start PaTorrent on system startup".
unsafe fn update_autostart_controls(dlg: HWND) {
    unsafe { EnableWindow(item(dlg, ID_START_MIN), checked(dlg, ID_AUTOSTART) as i32) };
}

/// Changes the registry if the General page's boxes changed. False (after saying why) when it
/// could not. When Windows' own choice still wins for what was just associated, offers to open
/// Default apps.
unsafe fn apply_integration(dlg: HWND, st: &mut State<'_>) -> bool {
    unsafe {
        let old = st.integration.set.normalized();
        let want = integration_controls(dlg);
        if want == old {
            return true;
        }
        match (st.hooks.integration)(Some((&old, &want))) {
            Ok(status) => {
                let overridden = (!old.torrent && want.torrent && status.torrent_choice.is_some())
                    || (!old.magnet && want.magnet && status.magnet_choice.is_some());
                st.integration = status;
                write_integration(dlg, st);
                if overridden {
                    let text = format!(
                        "{}\n\nWindows keeps a program chosen in Windows itself, and only you can change \
                         that choice, in Settings > Default apps. Open Default apps now?",
                        st.integration.note()
                    );
                    if dialogs::confirm(dlg, &text) {
                        crate::integration::open_default_apps(dlg);
                    }
                }
                true
            }
            Err(e) => {
                select_page(dlg, st, 0);
                dialogs::error(dlg, &format!("Could not change the Windows integration:\n{e}"));
                false
            }
        }
    }
}

/// The table and the Limited rates follow "Enable Scheduler", as in µTorrent.
unsafe fn update_sched_controls(dlg: HWND) {
    unsafe {
        let on = checked(dlg, ID_SCHED) as i32;
        for id in [ID_SCHED_GRID, ID_SCHED_UL, ID_SCHED_DL, ID_SCHED_DHT] {
            EnableWindow(item(dlg, id), on);
        }
    }
}

/// The scheduler table as edited.
unsafe fn sched_table(dlg: HWND) -> [Mode; scheduler::CELLS] {
    let mut table = [Mode::Full; scheduler::CELLS];
    unsafe { send(item(dlg, ID_SCHED_GRID), sched_grid::GET, 0, &mut table as *mut _ as LPARAM) };
    table
}

/// The folder and "Only move from ..." follow "Move completed downloads to", as in µTorrent.
unsafe fn update_move_controls(dlg: HWND) {
    unsafe {
        let on = checked(dlg, ID_MOVE) as i32;
        for id in [ID_MOVE_DIR, ID_MOVE_BROWSE, ID_MOVE_ONLY_DEFAULT] {
            EnableWindow(item(dlg, id), on);
        }
    }
}

/// "Location of .torrents" greys as µTorrent's did: the "Move .torrents for finished jobs"
/// box under "Store .torrents in", each folder under its box, "Delete loaded .torrents" under
/// "Automatically load .torrents from".
unsafe fn update_torrent_folder_controls(dlg: HWND) {
    unsafe {
        let store = checked(dlg, ID_STORE);
        let moving = store && checked(dlg, ID_MOVE_TORRENTS);
        let autoload = checked(dlg, ID_AUTOLOAD);
        for (id, on) in [
            (ID_STORE_DIR, store),
            (ID_STORE_BROWSE, store),
            (ID_MOVE_TORRENTS, store),
            (ID_MOVE_TORRENTS_DIR, moving),
            (ID_MOVE_TORRENTS_BROWSE, moving),
            (ID_AUTOLOAD_DIR, autoload),
            (ID_AUTOLOAD_BROWSE, autoload),
            (ID_AUTOLOAD_DELETE, autoload),
        ] {
            EnableWindow(item(dlg, id), on as i32);
        }
    }
}

/// A control's current value in the form `write_controls` remembers.
unsafe fn control_value(dlg: HWND, f: &Field) -> String {
    unsafe {
        match f {
            Field::Num { id, .. } | Field::Text { id, .. } => window_text(item(dlg, *id)),
            Field::Check { id, .. } => checked(dlg, *id).to_string(),
        }
    }
}

/// Applies the controls the user changed to the working options. On a bad value nothing is
/// changed and the offending control's id and a message come back.
unsafe fn read_controls(dlg: HWND, st: &mut State<'_>) -> Result<(), (u16, String)> {
    unsafe {
        let shown = |id: u16| st.shown.iter().find(|(i, _)| *i == id).map(|(_, s)| s.as_str());
        let mut o = st.opts.clone();
        for f in FIELDS {
            let now = control_value(dlg, f);
            if shown(f.id()) == Some(now.as_str()) {
                continue;
            }
            match f {
                Field::Num { id, min, max, set, .. } => match now.trim().parse::<i64>() {
                    Ok(v) if (*min..=*max).contains(&v) => set(&mut o, v),
                    _ => return Err((*id, format!("Please enter a number from {min} to {max}."))),
                },
                Field::Check { set, .. } => set(&mut o, now == "true"),
                Field::Text { set, .. } => set(&mut o, now),
            }
        }
        let enc = (send(item(dlg, ID_ENC_OUT), CB_GETCURSEL, 0, 0).max(0) as usize).min(ENC_OUT.len() - 1);
        let legacy = checked(dlg, ID_ENC_LEGACY);
        if shown(ID_ENC_OUT) != Some(enc.to_string().as_str())
            || shown(ID_ENC_LEGACY) != Some(legacy.to_string().as_str())
        {
            settings::set_encryption(&mut o.settings, ENC_OUT[enc].1, legacy);
        }
        let table = sched_table(dlg);
        let text: String = table.iter().map(|m| m.code()).collect();
        if shown(ID_SCHED_GRID) != Some(text.as_str()) {
            o.gui.schedule.table = table;
        }
        let kind =
            (send(item(dlg, ID_PROXY_TYPE), CB_GETCURSEL, 0, 0).max(0) as usize).min(PROXY_TYPES.len() - 1);
        let auth = checked(dlg, ID_PROXY_AUTH);
        if shown(ID_PROXY_TYPE) != Some(kind.to_string().as_str())
            || shown(ID_PROXY_AUTH) != Some(auth.to_string().as_str())
        {
            o.settings.proxy_type = proxy_from_ui(kind, auth);
        }
        if o.settings.proxy_type != ProxyType::None {
            if o.settings.proxy_hostname.is_empty() {
                return Err((ID_PROXY_HOST, "Please enter the proxy server's address.".to_owned()));
            }
            if o.settings.proxy_port == 0 {
                return Err((ID_PROXY_PORT, "Please enter the proxy server's port.".to_owned()));
            }
        }
        if o.gui.download_dir.trim().is_empty() {
            return Err((ID_DL_DIR, "Please choose a folder for new downloads.".to_owned()));
        }
        if o.gui.move_completed && o.gui.completed_dir.is_empty() {
            return Err((ID_MOVE_DIR, "Please choose a folder for completed downloads.".to_owned()));
        }
        let tf = &o.gui.torrent_folders;
        if tf.store && tf.store_dir.is_empty() {
            return Err((ID_STORE_DIR, "Please choose a folder to store .torrents in.".to_owned()));
        }
        if tf.store && tf.move_finished && tf.finished_dir.is_empty() {
            let message = "Please choose a folder for the .torrents of finished jobs.";
            return Err((ID_MOVE_TORRENTS_DIR, message.to_owned()));
        }
        if tf.autoload && tf.autoload_dir.is_empty() {
            return Err((ID_AUTOLOAD_DIR, "Please choose a folder to load .torrents from.".to_owned()));
        }
        st.opts = o;
        Ok(())
    }
}

/// Reads the pages; on a bad value shows its page and says what is wrong.
unsafe fn read_or_complain(dlg: HWND, st: &mut State<'_>) -> bool {
    unsafe {
        match read_controls(dlg, st) {
            Ok(()) => true,
            Err((id, message)) => {
                if let Some(p) = page_of(id) {
                    select_page(dlg, st, p);
                }
                dialogs::error(dlg, &message);
                SetFocus(item(dlg, id));
                false
            }
        }
    }
}

/// OK and Apply.
unsafe fn apply(dlg: HWND, st: &mut State<'_>) -> bool {
    unsafe {
        if !read_or_complain(dlg, st) || !apply_integration(dlg, st) {
            return false;
        }
        (st.hooks.apply)(&st.opts);
        st.applied = true;
        write_controls(dlg, st);
        let sel = selected_row(dlg);
        fill_advanced(dlg, st, sel);
        true
    }
}

unsafe fn command(dlg: HWND, st: &mut State<'_>, id: u16, code: u32) -> bool {
    unsafe {
        match id {
            // Enter in the Advanced value field sets the value instead of closing
            _ if id == IDOK as u16 && GetFocus() == item(dlg, ID_ADV_VALUE) => advanced_set(dlg, st, false),
            _ if id == IDOK as u16 => {
                if apply(dlg, st) {
                    EndDialog(dlg, IDOK as isize);
                }
            }
            _ if id == IDCANCEL as u16 => {
                EndDialog(dlg, IDCANCEL as isize);
            }
            ID_APPLY => {
                apply(dlg, st);
            }
            ID_DL_BROWSE if code == BN_CLICKED => {
                let now = window_text(item(dlg, ID_DL_DIR));
                let title = "Select the folder for new downloads";
                if let Some(p) = crate::create::pick_folder_at(dlg, title, Some(&now)) {
                    set_text(item(dlg, ID_DL_DIR), &p);
                }
            }
            ID_MOVE if code == BN_CLICKED => update_move_controls(dlg),
            ID_STORE | ID_MOVE_TORRENTS | ID_AUTOLOAD if code == BN_CLICKED => {
                update_torrent_folder_controls(dlg)
            }
            ID_STORE_BROWSE | ID_MOVE_TORRENTS_BROWSE | ID_AUTOLOAD_BROWSE if code == BN_CLICKED => {
                let (edit, title) = match id {
                    ID_STORE_BROWSE => (ID_STORE_DIR, "Select the folder to store .torrents in"),
                    ID_MOVE_TORRENTS_BROWSE => {
                        (ID_MOVE_TORRENTS_DIR, "Select the folder for the .torrents of finished jobs")
                    }
                    _ => (ID_AUTOLOAD_DIR, "Select the folder to load .torrents from"),
                };
                let now = window_text(item(dlg, edit));
                if let Some(p) = crate::create::pick_folder_at(dlg, title, Some(&now)) {
                    set_text(item(dlg, edit), &p);
                }
            }
            ID_SCHED if code == BN_CLICKED => update_sched_controls(dlg),
            ID_AUTOSTART if code == BN_CLICKED => update_autostart_controls(dlg),
            ID_DEFAULT_APPS if code == BN_CLICKED => crate::integration::open_default_apps(dlg),
            ID_MOVE_BROWSE if code == BN_CLICKED => {
                let now = window_text(item(dlg, ID_MOVE_DIR));
                let title = "Select the folder for completed downloads";
                if let Some(p) = crate::create::pick_folder_at(dlg, title, Some(&now)) {
                    set_text(item(dlg, ID_MOVE_DIR), &p);
                }
            }
            ID_RANDOM_PORT if code == BN_CLICKED => {
                set_text(item(dlg, ID_PORT), &options::random_port().to_string());
            }
            ID_IPF_BROWSE if code == BN_CLICKED => {
                if let Some(p) = crate::create::pick_file(dlg) {
                    set_text(item(dlg, ID_IPF_PATH), &p);
                }
            }
            ID_IPF_RELOAD if code == BN_CLICKED => reload_ip_filter(dlg, st),
            ID_ADV_FILTER if code == EN_CHANGE => fill_advanced(dlg, st, None),
            ID_ADV_SET if code == BN_CLICKED => advanced_set(dlg, st, false),
            ID_ADV_RESET if code == BN_CLICKED => advanced_set(dlg, st, true),
            _ => return false,
        }
        true
    }
}

unsafe fn notify(dlg: HWND, st: &mut State<'_>, nm: &NMHDR) {
    unsafe {
        match (nm.idFrom as u16, nm.code) {
            (ID_TREE, TVN_SELCHANGEDW) => {
                let tv = &*(nm as *const NMHDR as *const NMTREEVIEWW);
                let page = (tv.itemNew.lParam as usize).min(PAGES.len() - 1);
                if page == st.page {
                    return;
                }
                if page == PAGE_ADVANCED {
                    // the table shows what the pages say now
                    if !read_or_complain(dlg, st) {
                        return;
                    }
                    let sel = selected_row(dlg);
                    fill_advanced(dlg, st, sel);
                }
                show_page(dlg, st, page);
            }
            (ID_ADV_LIST, LVN_ITEMCHANGED) => {
                let lv = &*(nm as *const NMHDR as *const NMLISTVIEW);
                if lv.uChanged & LVIF_STATE != 0 && (lv.uNewState ^ lv.uOldState) & LVIS_SELECTED != 0 {
                    show_value_controls(dlg, st);
                }
            }
            _ => {}
        }
    }
}

unsafe fn selected_row(dlg: HWND) -> Option<usize> {
    let i = unsafe { send(item(dlg, ID_ADV_LIST), LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize) };
    (i >= 0).then_some(i as usize)
}

fn row_name(e: &Entry, o: &Options) -> String {
    if e.is_default(&o.settings) { e.name.to_owned() } else { format!("{}*", e.name) }
}

/// Fills the Advanced list with the rows matching the filter, sorted by name; selects
/// `select` (a row index) again if given.
unsafe fn fill_advanced(dlg: HWND, st: &mut State<'_>, select: Option<usize>) {
    unsafe {
        let filter = window_text(item(dlg, ID_ADV_FILTER)).trim().to_lowercase();
        let keep = select.and_then(|i| st.rows.get(i)).map(|e| e.name);
        let mut rows: Vec<&'static Entry> =
            settings::TABLE.iter().filter(|e| filter.is_empty() || e.name.contains(&filter)).collect();
        rows.sort_by_key(|e| e.name);
        st.rows = rows;
        let lv = item(dlg, ID_ADV_LIST);
        send(lv, LVM_DELETEALLITEMS, 0, 0);
        for (i, e) in st.rows.iter().enumerate() {
            let mut name = wide(&row_name(e, &st.opts));
            let mut it: LVITEMW = std::mem::zeroed();
            it.mask = LVIF_TEXT;
            it.iItem = i as i32;
            it.pszText = name.as_mut_ptr();
            send(lv, LVM_INSERTITEMW, 0, &it as *const _ as LPARAM);
            let mut value = wide(&e.get(&st.opts.settings).to_string());
            it.iSubItem = 1;
            it.pszText = value.as_mut_ptr();
            send(lv, LVM_SETITEMTEXTW, i, &it as *const _ as LPARAM);
        }
        if let Some(i) = keep.and_then(|n| st.rows.iter().position(|e| e.name == n)) {
            let mut it: LVITEMW = std::mem::zeroed();
            it.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
            it.state = LVIS_SELECTED | LVIS_FOCUSED;
            send(lv, LVM_SETITEMSTATE, i, &it as *const _ as LPARAM);
            send(lv, LVM_ENSUREVISIBLE, i, 0);
        }
        show_value_controls(dlg, st);
    }
}

/// The value field for the selected row: True / False for switches, a text field otherwise.
unsafe fn show_value_controls(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        let entry = selected_row(dlg).and_then(|i| st.rows.get(i).copied());
        let is_bool = entry.is_some_and(|e| e.kind == Kind::Bool);
        let visible = st.page == PAGE_ADVANCED;
        let show = |id: u16, on: bool| {
            ShowWindow(item(dlg, id), if visible && on { SW_SHOW } else { SW_HIDE });
        };
        show(ID_ADV_VALUE, !is_bool);
        show(ID_ADV_TRUE, is_bool);
        show(ID_ADV_FALSE, is_bool);
        match entry.map(|e| e.get(&st.opts.settings)) {
            Some(Value::Bool(b)) => {
                set_check(dlg, ID_ADV_TRUE, b);
                set_check(dlg, ID_ADV_FALSE, !b);
            }
            Some(v) => set_text(item(dlg, ID_ADV_VALUE), &v.to_string()),
            None => set_text(item(dlg, ID_ADV_VALUE), ""),
        }
        for id in [ID_ADV_VALUE, ID_ADV_SET, ID_ADV_RESET] {
            EnableWindow(item(dlg, id), entry.is_some() as i32);
        }
    }
}

/// "Set" (or "Reset" to the default) for the selected row; the pages follow.
unsafe fn advanced_set(dlg: HWND, st: &mut State<'_>, reset: bool) {
    unsafe {
        let Some(i) = selected_row(dlg) else { return };
        let Some(e) = st.rows.get(i).copied() else { return };
        let text = if reset {
            e.get(&patorrent_core::Settings::default()).to_string()
        } else if e.kind == Kind::Bool {
            checked(dlg, ID_ADV_TRUE).to_string()
        } else {
            window_text(item(dlg, ID_ADV_VALUE))
        };
        if let Err(msg) = e.set(&mut st.opts.settings, &text) {
            dialogs::error(dlg, &msg);
            SetFocus(item(dlg, ID_ADV_VALUE));
            return;
        }
        // the pages show the table's values (the page fields were read when Advanced opened)
        write_controls(dlg, st);
        fill_advanced(dlg, st, Some(i));
    }
}

unsafe fn update_ip_filter_status(dlg: HWND, st: &State<'_>, result: Option<Result<usize, String>>) {
    unsafe {
        let path = window_text(item(dlg, ID_IPF_PATH));
        let text = match result {
            Some(Ok(n)) => format!("{n} rules loaded."),
            Some(Err(e)) => e,
            None if path.trim().is_empty() => {
                format!("Empty: {}", st.settings_dir.join("ipfilter.dat").display())
            }
            None => String::new(),
        };
        set_text(item(dlg, ID_IPF_STATUS), &text);
    }
}

/// "Reload": loads the block list in the path field now (whether or not the rest is applied).
unsafe fn reload_ip_filter(dlg: HWND, st: &mut State<'_>) {
    unsafe {
        if !checked(dlg, ID_IPF) {
            update_ip_filter_status(dlg, st, Some(Err("The IP filter is off.".to_owned())));
            return;
        }
        let mut gui = st.opts.gui.clone();
        gui.ip_filter_path = window_text(item(dlg, ID_IPF_PATH)).trim().to_owned();
        let file = gui.ip_filter_file(&st.settings_dir);
        let result = (st.hooks.reload_ip_filter)(&file).map_err(|e| format!("{}: {e}", file.display()));
        update_ip_filter_status(dlg, st, Some(result));
    }
}

#[cfg(test)]
mod tests {
    //! The dialog built from its template, hidden and modeless, driven through its messages.

    use std::cell::RefCell;
    use std::rc::Rc;

    use patorrent_core::{EncLevel, Settings};
    use patorrent_frontend::prefs::Prefs;
    use windows_sys::Win32::UI::Controls::{
        ICC_LISTVIEW_CLASSES, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx,
        LVM_GETITEMCOUNT, LVM_GETITEMTEXTW,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CB_GETCOUNT, CreateDialogIndirectParamW, DestroyWindow, GWL_STYLE, GetWindowLongW, SendMessageW,
        WS_VISIBLE,
    };

    use super::*;

    type Log = Rc<RefCell<Vec<Options>>>;

    struct Fixture {
        dlg: HWND,
        st: Box<State<'static>>,
        applied: Log,
        reloads: Rc<RefCell<Vec<PathBuf>>>,
        /// The fake registry: what it reads as, and the changes asked for.
        registry: Rc<RefCell<Status>>,
        integrations: Rc<RefCell<Vec<(Integration, Integration)>>>,
    }

    impl Fixture {
        fn new(opts: Options) -> Fixture {
            Fixture::with_registry(opts, Status::default())
        }

        fn with_registry(opts: Options, registry: Status) -> Fixture {
            unsafe {
                let icc = INITCOMMONCONTROLSEX {
                    dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                    dwICC: ICC_LISTVIEW_CLASSES | ICC_TREEVIEW_CLASSES,
                };
                InitCommonControlsEx(&icc);
            }
            let applied: Log = Rc::default();
            let reloads: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
            let (a, r) = (applied.clone(), reloads.clone());
            let registry = Rc::new(RefCell::new(registry));
            let integrations: Rc<RefCell<Vec<(Integration, Integration)>>> = Rc::default();
            let (reg, ints) = (registry.clone(), integrations.clone());
            let hooks = Hooks {
                integration: Box::new(move |change| {
                    if let Some((old, new)) = change {
                        ints.borrow_mut().push((*old, *new));
                        reg.borrow_mut().set = *new;
                    }
                    Ok(reg.borrow().clone())
                }),
                apply: Box::new(move |o: &Options| a.borrow_mut().push(o.clone())),
                reload_ip_filter: Box::new(move |p: &Path| {
                    r.borrow_mut().push(p.to_path_buf());
                    if p.ends_with("missing.dat") { Err("not found".into()) } else { Ok(42) }
                }),
            };
            let mut st = Box::new(State::new(opts, Path::new(r"C:\settings"), hooks));
            let template = template();
            let dlg = unsafe {
                CreateDialogIndirectParamW(
                    GetModuleHandleW(null()),
                    template.as_ptr() as *const DLGTEMPLATE,
                    std::ptr::null_mut(),
                    Some(proc_),
                    &mut *st as *mut State<'_> as LPARAM,
                )
            };
            assert!(!dlg.is_null(), "the template does not load");
            Fixture { dlg, st, applied, reloads, registry, integrations }
        }

        fn click(&self, id: u16) {
            unsafe { SendMessageW(self.dlg, WM_COMMAND, id as usize | (BN_CLICKED as usize) << 16, 0) };
        }

        fn text(&self, id: u16) -> String {
            unsafe { window_text(item(self.dlg, id)) }
        }

        fn set(&self, id: u16, s: &str) {
            unsafe { set_text(item(self.dlg, id), s) };
        }

        fn check(&self, id: u16, on: bool) {
            unsafe { set_check(self.dlg, id, on) };
        }

        fn page(&mut self, page: usize) {
            unsafe { select_page(self.dlg, &mut self.st, page) };
        }

        fn last_applied(&self) -> Options {
            self.applied.borrow().last().cloned().expect("nothing applied")
        }

        fn advanced_row(&self, name: &str) -> usize {
            self.st.rows.iter().position(|e| e.name == name).unwrap_or_else(|| panic!("no row {name}"))
        }

        fn select_row(&self, i: usize) {
            unsafe {
                let mut it: LVITEMW = std::mem::zeroed();
                it.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
                it.state = LVIS_SELECTED | LVIS_FOCUSED;
                send(item(self.dlg, ID_ADV_LIST), LVM_SETITEMSTATE, i, &it as *const _ as LPARAM);
            }
        }

        fn row_text(&self, i: usize, sub: i32) -> String {
            let mut buf = [0u16; 256];
            unsafe {
                let mut it: LVITEMW = std::mem::zeroed();
                it.iSubItem = sub;
                it.pszText = buf.as_mut_ptr();
                it.cchTextMax = buf.len() as i32;
                send(item(self.dlg, ID_ADV_LIST), LVM_GETITEMTEXTW, i, &mut it as *mut _ as LPARAM);
            }
            crate::util::from_wide(&buf)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.dlg) };
        }
    }

    /// The control's own visible style (the hidden dialog makes `IsWindowVisible` false for all).
    unsafe fn shown(h: HWND) -> bool {
        unsafe { GetWindowLongW(h, GWL_STYLE) as u32 & WS_VISIBLE != 0 }
    }

    fn defaults() -> Options {
        Options::load(&Prefs::default(), r"C:\Downloads")
    }

    #[test]
    fn every_page_and_control_exists() {
        let mut f = Fixture::new(defaults());
        let dlg = f.dlg;
        unsafe {
            for id in [ID_TREE, ID_APPLY, IDOK as u16, IDCANCEL as u16] {
                assert!(!item(dlg, id).is_null(), "control {id}");
            }
            for field in FIELDS {
                assert!(!item(dlg, field.id()).is_null(), "field {}", field.id());
            }
            for id in [
                ID_ENC_OUT,
                ID_ENC_LEGACY,
                ID_IPF_RELOAD,
                ID_ADV_LIST,
                ID_ADV_SET,
                ID_PROXY_TYPE,
                ID_PROXY_AUTH,
            ] {
                assert!(!item(dlg, id).is_null(), "control {id}");
            }
            assert_eq!(f.st.tree_items.len(), PAGES.len());
            assert_eq!(send(item(dlg, ID_ENC_OUT), CB_GETCOUNT, 0, 0), 3);
            assert_eq!(send(item(dlg, ID_PROXY_TYPE), CB_GETCOUNT, 0, 0), PROXY_TYPES.len() as isize);
            // only the shown page's controls are visible
            for page in 0..PAGES.len() {
                f.page(page);
                assert_eq!(f.st.page, page);
                let on_page = shown(item(dlg, id(page as u16, 51)));
                let other = shown(item(dlg, id(((page + 1) % PAGES.len()) as u16, 51)));
                assert!(on_page && !other, "page {page}");
            }
            assert_eq!(send(item(dlg, ID_ADV_LIST), LVM_GETITEMCOUNT, 0, 0) as usize, settings::TABLE.len());
        }
    }

    #[test]
    fn values_round_trip_through_the_pages() {
        let mut o = defaults();
        o.settings.upload_rate_limit = 100 * 1024;
        o.settings.connections_limit = 123;
        o.settings.enable_lsd = false;
        settings::set_listen_port(&mut o.settings, 23456);
        settings::set_encryption(&mut o.settings, EncPolicy::Forced, false);
        o.gui.minimize_to_tray = true;
        o.gui.max_peers_per_torrent = 50;
        o.gui.ip_filter = true;
        let f = Fixture::new(o.clone());
        assert_eq!(f.text(ID_PORT), "23456");
        assert_eq!(f.text(ID_UL_RATE), "100");
        assert_eq!(f.text(ID_DL_RATE), "0");
        assert_eq!(f.text(ID_CONNECTIONS), "123");
        assert_eq!(f.text(ID_PEERS_PER_TORRENT), "50");
        assert_eq!(f.text(ID_DL_DIR), r"C:\Downloads");
        assert_eq!(f.text(ID_ACTIVE_DL), "3");
        unsafe {
            assert!(checked(f.dlg, ID_MIN_TRAY) && checked(f.dlg, ID_CLOSE_TRAY));
            assert!(!checked(f.dlg, ID_LSD) && checked(f.dlg, ID_DHT) && checked(f.dlg, ID_UTP));
            assert!(checked(f.dlg, ID_IPF) && !checked(f.dlg, ID_ENC_LEGACY));
            assert_eq!(send(item(f.dlg, ID_ENC_OUT), CB_GETCURSEL, 0, 0), 2);
        }
        assert!(f.text(ID_IPF_STATUS).contains(r"C:\settings\ipfilter.dat"));

        // Apply without changes hands back exactly what came in
        f.click(ID_APPLY);
        assert_eq!(f.last_applied(), o);
        assert!(f.st.applied);
    }

    #[test]
    fn apply_hands_the_changed_settings_over() {
        let f = Fixture::new(defaults());
        f.set(ID_PORT, "40000");
        f.set(ID_UL_RATE, "50");
        f.set(ID_DL_RATE, "2500");
        f.set(ID_CONNECTIONS, "300");
        f.set(ID_SLOTS_PER_TORRENT, "4");
        f.set(ID_ACTIVE, "-1");
        f.set(ID_ACTIVE_SEEDS, "9");
        f.set(ID_SEED_RATIO, "200");
        f.set(ID_SEED_TIME, "45");
        f.check(ID_SEED_LIMIT, true);
        f.set(ID_SEED_LIMIT_KIB, "8");
        f.set(ID_DISK_QUEUE, "4096");
        f.set(ID_DL_DIR, r"D:\Torrents");
        f.set(ID_IPF_PATH, r"D:\level1.dat");
        f.check(ID_DHT, false);
        f.check(ID_PEX, false);
        f.check(ID_UTP, false);
        f.check(ID_UPNP, false);
        f.check(ID_ENC_LEGACY, false);
        f.check(ID_ADD_STOPPED, true);
        f.check(ID_CONFIRM_DELETE, false);
        unsafe { send(item(f.dlg, ID_ENC_OUT), CB_SETCURSEL, 0, 0) };
        f.click(ID_APPLY);

        let o = f.last_applied();
        let mut want = Settings::default();
        settings::set_listen_port(&mut want, 40000);
        want.upload_rate_limit = 50 * 1024;
        want.download_rate_limit = 2500 * 1024;
        want.connections_limit = 300;
        want.active_limit = -1;
        want.active_seeds = 9;
        want.max_queued_disk_bytes = 4096 * 1024;
        want.enable_dht = false;
        want.enable_upnp = false;
        want.enable_incoming_utp = false;
        want.enable_outgoing_utp = false;
        want.out_enc_policy = EncPolicy::Disabled;
        want.in_enc_policy = EncPolicy::Forced;
        want.allowed_enc_level = EncLevel::Both;
        assert_eq!(o.settings, want);
        assert_eq!(o.settings.listen_interfaces, "0.0.0.0:40000,[::]:40000");
        assert_eq!(o.gui.download_dir, r"D:\Torrents");
        assert_eq!(o.gui.ip_filter_path, r"D:\level1.dat");
        assert_eq!(o.gui.upload_slots_per_torrent, 4);
        assert!(!o.gui.pex && o.gui.add_stopped && !o.gui.confirm_delete);
        let goal = o.gui.seeding_goal();
        assert_eq!(
            (goal.goal.ratio_pct, goal.goal.minutes, goal.limit_upload, goal.limit_kib),
            (200, 45, true, 8)
        );

        // persisted and loaded back the same
        let mut prefs = Prefs::default();
        o.store(&mut prefs);
        assert_eq!(Options::load(&prefs, "unused"), o);
    }

    #[test]
    fn the_general_page_changes_the_windows_integration() {
        let registry = Status {
            set: Integration { magnet: true, ..Integration::default() },
            magnet_choice: Some("qBittorrent.Url.Magnet".into()),
            ..Status::default()
        };
        let f = Fixture::with_registry(defaults(), registry);
        unsafe {
            assert!(!checked(f.dlg, ID_ASSOC_TORRENT) && checked(f.dlg, ID_ASSOC_MAGNET));
            assert!(!checked(f.dlg, ID_AUTOSTART) && !checked(f.dlg, ID_CHECK_ASSOC));
            assert_eq!(IsWindowEnabled(item(f.dlg, ID_START_MIN)), 0, "greyed without autostart");
        }
        assert_eq!(
            f.text(ID_ASSOC_NOTE),
            "Windows still opens magnet: links with qBittorrent.Url.Magnet. Choose PaTorrent in Default apps."
        );

        // Apply with nothing changed leaves the registry alone
        f.click(ID_APPLY);
        assert!(f.integrations.borrow().is_empty());

        f.check(ID_ASSOC_TORRENT, true);
        f.check(ID_AUTOSTART, true);
        f.click(ID_AUTOSTART);
        unsafe { assert_ne!(IsWindowEnabled(item(f.dlg, ID_START_MIN)), 0) };
        f.check(ID_START_MIN, true);
        f.check(ID_CHECK_ASSOC, true);
        f.click(ID_APPLY);
        let all = Integration { torrent: true, magnet: true, autostart: true, minimized: true };
        let old = Integration { magnet: true, ..Integration::default() };
        assert_eq!(*f.integrations.borrow(), [(old, all)]);
        assert!(f.last_applied().gui.check_association, "an option, stored in patorrent.ini");

        // "Start minimized" alone means nothing once autostart is off
        f.check(ID_AUTOSTART, false);
        f.click(ID_AUTOSTART);
        f.click(ID_APPLY);
        let off = Integration { autostart: false, minimized: false, ..all };
        assert_eq!(f.integrations.borrow().last(), Some(&(all, off)));
        assert_eq!(f.registry.borrow().set, off);
        unsafe { assert!(!checked(f.dlg, ID_START_MIN), "shown as read back") };
    }

    #[test]
    fn the_add_dialog_switch_is_on_ui_settings() {
        let mut f = Fixture::new(defaults());
        f.page(1);
        unsafe {
            assert!(shown(item(f.dlg, ID_SHOW_ADD)));
            assert!(checked(f.dlg, ID_SHOW_ADD), "on by default, as in µTorrent");
        }
        f.check(ID_SHOW_ADD, false);
        f.click(ID_APPLY);
        let o = f.last_applied();
        assert!(!o.gui.show_add_dialog);
        let mut prefs = Prefs::default();
        o.store(&mut prefs);
        assert!(prefs.to_text().contains("ui.show_add_dialog=0"));
        assert!(!Options::load(&prefs, "unused").gui.show_add_dialog);
    }

    #[test]
    fn the_proxy_box_sets_the_proxy_settings() {
        let f = Fixture::new(defaults());
        unsafe {
            assert_eq!(send(item(f.dlg, ID_PROXY_TYPE), CB_GETCURSEL, 0, 0), 0);
            send(item(f.dlg, ID_PROXY_TYPE), CB_SETCURSEL, 2, 0);
        }
        f.set(ID_PROXY_HOST, " proxy.example ");
        f.set(ID_PROXY_PORT, "1080");
        f.check(ID_PROXY_AUTH, true);
        f.set(ID_PROXY_USER, "me");
        f.set(ID_PROXY_PASS, "secret");
        f.check(ID_PROXY_TRACKERS, false);
        f.click(ID_APPLY);
        let o = f.last_applied();
        let s = &o.settings;
        assert_eq!(s.proxy_type, ProxyType::Socks5Pw);
        assert_eq!((s.proxy_hostname.as_str(), s.proxy_port), ("proxy.example", 1080));
        assert_eq!((s.proxy_username.as_str(), s.proxy_password.as_str()), ("me", "secret"));
        assert!(s.proxy_hostnames && s.proxy_peer_connections && !s.proxy_tracker_connections);
        let mut prefs = Prefs::default();
        o.store(&mut prefs);
        assert_eq!(Options::load(&prefs, "unused"), o);

        // shown back the same way
        let g = Fixture::new(o.clone());
        unsafe {
            assert_eq!(send(item(g.dlg, ID_PROXY_TYPE), CB_GETCURSEL, 0, 0), 2);
            assert!(checked(g.dlg, ID_PROXY_AUTH));
        }
        assert_eq!(g.text(ID_PROXY_PORT), "1080");
        for (t, auth) in [(ProxyType::None, false), (ProxyType::Socks5, false), (ProxyType::HttpPw, true)] {
            let mut s = Settings { proxy_type: t, ..Settings::default() };
            assert_eq!(proxy_from_ui(proxy_to_ui(&s).0, auth), t);
            s.proxy_username = "id".into();
            assert_eq!(proxy_to_ui(&Settings { proxy_type: ProxyType::Socks4, ..s }), (1, true));
        }
    }

    #[test]
    fn a_proxy_needs_an_address() {
        let mut f = Fixture::new(defaults());
        unsafe { send(item(f.dlg, ID_PROXY_TYPE), CB_SETCURSEL, 3, 0) };
        assert_eq!(unsafe { read_controls(f.dlg, &mut f.st) }.unwrap_err().0, ID_PROXY_HOST);
        f.set(ID_PROXY_HOST, "10.0.0.1");
        assert_eq!(unsafe { read_controls(f.dlg, &mut f.st) }.unwrap_err().0, ID_PROXY_PORT);
        f.set(ID_PROXY_PORT, "3128");
        unsafe { read_controls(f.dlg, &mut f.st) }.unwrap();
        assert_eq!(f.st.opts.settings.proxy_type, ProxyType::Http);
    }

    #[test]
    fn bad_values_are_refused_on_their_page() {
        let mut f = Fixture::new(defaults());
        f.set(ID_CONNECTIONS, "1");
        // Apply would show a modal error box; the reader behind it is checked directly
        let err = unsafe { read_controls(f.dlg, &mut f.st) }.unwrap_err();
        assert_eq!(err.0, ID_CONNECTIONS);
        assert_eq!(page_of(err.0), Some(4));
        assert_eq!(f.st.opts, defaults(), "nothing half-applied");
        f.set(ID_CONNECTIONS, "200");
        f.set(ID_PORT, "70000");
        assert_eq!(unsafe { read_controls(f.dlg, &mut f.st) }.unwrap_err().0, ID_PORT);
        f.set(ID_PORT, "6881");
        f.set(ID_DL_DIR, " ");
        assert_eq!(unsafe { read_controls(f.dlg, &mut f.st) }.unwrap_err().0, ID_DL_DIR);
        assert!(f.applied.borrow().is_empty());
    }

    #[test]
    fn unchanged_controls_keep_values_they_round() {
        let mut o = defaults();
        // 1000 B/s shows as 0 kB/s; leaving the field alone must not turn it into "unlimited"
        o.settings.upload_rate_limit = 1000;
        let f = Fixture::new(o);
        assert_eq!(f.text(ID_UL_RATE), "0");
        f.check(ID_LSD, false);
        f.click(ID_APPLY);
        let a = f.last_applied();
        assert_eq!(a.settings.upload_rate_limit, 1000);
        assert!(!a.settings.enable_lsd);
    }

    #[test]
    fn advanced_table_edits_follow_the_pages() {
        let f = Fixture::new(defaults());
        // a page edit shows in the table once Advanced opens
        f.set(ID_CONNECTIONS, "77");
        unsafe {
            let tree = item(f.dlg, ID_TREE);
            send(tree, TVM_SELECTITEM, TVGN_CARET as usize, f.st.tree_items[PAGE_ADVANCED]);
        }
        assert_eq!(f.st.page, PAGE_ADVANCED);
        let row = f.advanced_row("connections_limit");
        assert_eq!(f.row_text(row, 0), "connections_limit*");
        assert_eq!(f.row_text(row, 1), "77");

        // a number
        let row = f.advanced_row("max_pex_peers");
        f.select_row(row);
        assert_eq!(f.text(ID_ADV_VALUE), "50");
        f.set(ID_ADV_VALUE, "75");
        f.click(ID_ADV_SET);
        assert_eq!(f.st.opts.settings.max_pex_peers, 75);
        let row = f.advanced_row("max_pex_peers");
        assert_eq!(f.row_text(row, 1), "75");

        // a switch, shown as True / False
        let row = f.advanced_row("enable_dht");
        f.select_row(row);
        unsafe {
            assert!(shown(item(f.dlg, ID_ADV_TRUE)));
            assert!(!shown(item(f.dlg, ID_ADV_VALUE)));
            assert!(checked(f.dlg, ID_ADV_TRUE));
        }
        f.check(ID_ADV_TRUE, false);
        f.check(ID_ADV_FALSE, true);
        f.click(ID_ADV_SET);
        assert!(!f.st.opts.settings.enable_dht);
        unsafe { assert!(!checked(f.dlg, ID_DHT), "the BitTorrent page follows") };

        // µTorrent's transport bits drive the uTP box
        let row = f.advanced_row("bt.transp_disposition");
        f.select_row(row);
        assert_eq!(f.text(ID_ADV_VALUE), "15");
        f.set(ID_ADV_VALUE, "5");
        f.click(ID_ADV_SET);
        unsafe { assert!(!checked(f.dlg, ID_UTP)) };

        // a bad value is refused (the error box is skipped by checking the parser directly)
        assert!(settings::find("connections_limit").unwrap().parse("many").is_err());

        // Reset brings the default back
        let row = f.advanced_row("max_pex_peers");
        f.select_row(row);
        f.click(ID_ADV_RESET);
        assert_eq!(f.st.opts.settings.max_pex_peers, 50);

        // the filter narrows the list
        f.set(ID_ADV_FILTER, "utp_");
        let n = unsafe { send(item(f.dlg, ID_ADV_LIST), LVM_GETITEMCOUNT, 0, 0) } as usize;
        assert_eq!(n, f.st.rows.len());
        assert!(n >= 9 && f.st.rows.iter().all(|e| e.name.contains("utp_")));

        f.click(ID_APPLY);
        let a = f.last_applied();
        assert_eq!(a.settings.connections_limit, 77);
        assert_eq!(a.settings.max_pex_peers, 50);
        assert!(!a.settings.enable_dht);
        assert_eq!(settings::transp_disposition(&a.settings), 5);
    }

    #[test]
    fn ip_filter_reload_uses_the_path_field() {
        let mut o = defaults();
        o.gui.ip_filter = true;
        let f = Fixture::new(o);
        f.click(ID_IPF_RELOAD);
        assert_eq!(f.reloads.borrow().last().unwrap(), &Path::new(r"C:\settings").join("ipfilter.dat"));
        assert_eq!(f.text(ID_IPF_STATUS), "42 rules loaded.");
        f.set(ID_IPF_PATH, r"C:\x\missing.dat");
        f.click(ID_IPF_RELOAD);
        assert_eq!(f.reloads.borrow().last().unwrap(), Path::new(r"C:\x\missing.dat"));
        assert!(f.text(ID_IPF_STATUS).contains("not found"));
        f.check(ID_IPF, false);
        f.click(ID_IPF_RELOAD);
        assert_eq!(f.reloads.borrow().len(), 2, "no reload while the filter is off");
        assert!(f.applied.borrow().is_empty(), "Reload does not apply the rest");
    }

    #[test]
    fn move_completed_downloads_on_directories() {
        let mut f = Fixture::new(defaults());
        f.page(2);
        unsafe {
            for id in [ID_MOVE, ID_MOVE_DIR, ID_MOVE_BROWSE, ID_MOVE_ONLY_DEFAULT] {
                assert!(shown(item(f.dlg, id)), "control {id}");
            }
            assert!(!checked(f.dlg, ID_MOVE));
            assert!(checked(f.dlg, ID_MOVE_ONLY_DEFAULT), "on by default, as in µTorrent");
            assert_eq!(IsWindowEnabled(item(f.dlg, ID_MOVE_DIR)), 0, "greyed while off");
            assert_eq!(IsWindowEnabled(item(f.dlg, ID_MOVE_ONLY_DEFAULT)), 0);
        }
        f.check(ID_MOVE, true);
        f.click(ID_MOVE);
        unsafe {
            assert_ne!(IsWindowEnabled(item(f.dlg, ID_MOVE_DIR)), 0);
            assert_ne!(IsWindowEnabled(item(f.dlg, ID_MOVE_BROWSE)), 0);
        }
        // on without a folder is refused on the Directories page
        let err = unsafe { read_controls(f.dlg, &mut f.st) }.unwrap_err();
        assert_eq!((err.0, page_of(err.0)), (ID_MOVE_DIR, Some(2)));

        f.set(ID_MOVE_DIR, r" D:\Done ");
        f.check(ID_MOVE_ONLY_DEFAULT, false);
        f.click(ID_APPLY);
        let o = f.last_applied();
        assert!(o.gui.move_completed && !o.gui.move_only_default);
        assert_eq!(o.gui.completed_dir, r"D:\Done");
        let m = o.gui.move_completed().expect("a mover");
        assert_eq!((m.dir.as_str(), m.only_from), (r"D:\Done", None));
        let mut prefs = Prefs::default();
        o.store(&mut prefs);
        let text = prefs.to_text();
        assert!(text.contains("dir.move_completed=1") && text.contains(r"dir.completed=D:\Done"));
        assert_eq!(Options::load(&prefs, "unused"), o);

        // shown back, enabled; turned off again it keeps the folder
        let g = Fixture::new(o);
        assert_eq!(g.text(ID_MOVE_DIR), r"D:\Done");
        unsafe {
            assert!(checked(g.dlg, ID_MOVE) && !checked(g.dlg, ID_MOVE_ONLY_DEFAULT));
            assert_ne!(IsWindowEnabled(item(g.dlg, ID_MOVE_DIR)), 0);
        }
        g.check(ID_MOVE, false);
        g.click(ID_MOVE);
        g.click(ID_APPLY);
        let o = g.last_applied();
        assert!(!o.gui.move_completed && o.gui.move_completed().is_none());
        assert_eq!(o.gui.completed_dir, r"D:\Done");
        unsafe { assert_eq!(IsWindowEnabled(item(g.dlg, ID_MOVE_DIR)), 0) };
    }

    #[test]
    fn torrent_folders_on_directories() {
        let mut f = Fixture::new(defaults());
        f.page(2);
        let all = [
            ID_STORE,
            ID_STORE_DIR,
            ID_STORE_BROWSE,
            ID_MOVE_TORRENTS,
            ID_MOVE_TORRENTS_DIR,
            ID_MOVE_TORRENTS_BROWSE,
            ID_AUTOLOAD,
            ID_AUTOLOAD_DIR,
            ID_AUTOLOAD_BROWSE,
            ID_AUTOLOAD_DELETE,
        ];
        let enabled = |f: &Fixture, id: u16| unsafe { IsWindowEnabled(item(f.dlg, id)) != 0 };
        unsafe {
            for id in all {
                assert!(shown(item(f.dlg, id)), "control {id}");
            }
            assert!(!checked(f.dlg, ID_STORE) && !checked(f.dlg, ID_AUTOLOAD));
        }
        // greyed as in µTorrent: everything under an unchecked box
        for id in all.into_iter().filter(|&id| id != ID_STORE && id != ID_AUTOLOAD) {
            assert!(!enabled(&f, id), "control {id} greyed");
        }
        f.check(ID_STORE, true);
        f.click(ID_STORE);
        assert!(enabled(&f, ID_STORE_DIR) && enabled(&f, ID_MOVE_TORRENTS));
        assert!(!enabled(&f, ID_MOVE_TORRENTS_DIR), "until its own box is checked");
        f.check(ID_MOVE_TORRENTS, true);
        f.click(ID_MOVE_TORRENTS);
        assert!(enabled(&f, ID_MOVE_TORRENTS_DIR) && enabled(&f, ID_MOVE_TORRENTS_BROWSE));
        f.check(ID_AUTOLOAD, true);
        f.click(ID_AUTOLOAD);
        assert!(enabled(&f, ID_AUTOLOAD_DIR) && enabled(&f, ID_AUTOLOAD_DELETE));

        // each checked box needs its folder, refused on the Directories page
        for (id, text) in [
            (ID_STORE_DIR, r"D:\Torrents"),
            (ID_MOVE_TORRENTS_DIR, r" D:\Done "),
            (ID_AUTOLOAD_DIR, r"D:\Watch"),
        ] {
            let err = unsafe { read_controls(f.dlg, &mut f.st) }.unwrap_err();
            assert_eq!((err.0, page_of(err.0)), (id, Some(2)));
            f.set(id, text);
        }
        f.check(ID_AUTOLOAD_DELETE, true);
        f.click(ID_APPLY);
        let o = f.last_applied();
        let tf = &o.gui.torrent_folders;
        assert!(tf.store && tf.move_finished && tf.autoload && tf.autoload_delete);
        assert_eq!(tf.stores_in(), Some(r"D:\Torrents"));
        assert_eq!(tf.moves_finished_to(), Some(r"D:\Done"));
        assert_eq!(tf.loads_from(), Some(r"D:\Watch"));
        let mut prefs = Prefs::default();
        o.store(&mut prefs);
        let text = prefs.to_text();
        assert!(text.contains("dir.store_torrents=1") && text.contains(r"dir.autoload_dir=D:\Watch"));
        assert_eq!(Options::load(&prefs, "unused"), o);

        // shown back; unchecking Store greys the move box and its folder but keeps them
        let g = Fixture::new(o);
        assert_eq!(g.text(ID_MOVE_TORRENTS_DIR), r"D:\Done");
        assert!(enabled(&g, ID_MOVE_TORRENTS_DIR));
        g.check(ID_STORE, false);
        g.click(ID_STORE);
        assert!(!enabled(&g, ID_MOVE_TORRENTS) && !enabled(&g, ID_MOVE_TORRENTS_DIR));
        g.click(ID_APPLY);
        let tf = g.last_applied().gui.torrent_folders;
        assert!(!tf.store && tf.move_finished);
        assert_eq!((tf.stores_in(), tf.moves_finished_to()), (None, None));
    }

    #[test]
    fn random_port_fills_the_field() {
        let f = Fixture::new(defaults());
        f.click(ID_RANDOM_PORT);
        let port: u16 = f.text(ID_PORT).parse().unwrap();
        assert!(port >= 10_000);
        f.click(ID_APPLY);
        assert_eq!(settings::listen_port(&f.last_applied().settings), Some(port));
    }

    #[test]
    fn the_scheduler_page_edits_the_schedule() {
        let mut opts = defaults();
        opts.gui.schedule.set(2, 5, Mode::Off);
        let mut f = Fixture::new(opts);
        f.page(7);
        let grid = unsafe { item(f.dlg, ID_SCHED_GRID) };
        unsafe {
            for id in [ID_SCHED, ID_SCHED_GRID, ID_SCHED_UL, ID_SCHED_DL, ID_SCHED_DHT] {
                assert!(shown(item(f.dlg, id)), "control {id}");
            }
            assert!(!checked(f.dlg, ID_SCHED));
            assert!(checked(f.dlg, ID_SCHED_DHT), "on by default, as in µTorrent");
            assert_eq!(IsWindowEnabled(grid), 0, "greyed while off");
            assert_eq!(sched_table(f.dlg)[2 * scheduler::HOURS + 5], Mode::Off);
        }
        f.check(ID_SCHED, true);
        f.click(ID_SCHED);
        unsafe { assert_ne!(IsWindowEnabled(grid), 0) };
        // Monday 01:00 clicked once (Limited), then dragged over 02:00 and 03:00
        let at = |day: usize, hour: usize| unsafe {
            let mut rc: RECT = std::mem::zeroed();
            windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(grid, &mut rc);
            let l = crate::sched_grid::Layout::new(rc.right, rc.bottom);
            let c = l.cell_rect(day, hour);
            ((c.top + 2) << 16 | (c.left + 2)) as LPARAM
        };
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};
            SendMessageW(grid, WM_LBUTTONDOWN, 1, at(0, 1));
            SendMessageW(grid, WM_MOUSEMOVE, 1, at(0, 2));
            SendMessageW(grid, WM_MOUSEMOVE, 1, at(0, 3));
            SendMessageW(grid, WM_LBUTTONUP, 0, at(0, 3));
            SendMessageW(grid, WM_MOUSEMOVE, 0, at(0, 4));
        }
        f.set(ID_SCHED_UL, "20");
        f.set(ID_SCHED_DL, "300");
        f.check(ID_SCHED_DHT, false);
        f.click(ID_APPLY);
        let s = f.last_applied().gui.schedule;
        assert!(s.enabled && !s.disable_dht);
        assert_eq!((s.limited_up_kib, s.limited_down_kib), (20, 300));
        let row: Vec<Mode> = (0..5).map(|h| s.get(0, h)).collect();
        assert_eq!(row, [Mode::Full, Mode::Limited, Mode::Limited, Mode::Limited, Mode::Full]);
        assert_eq!(s.get(2, 5), Mode::Off);
        // an untouched grid keeps a table set elsewhere (the controls are rewritten on Apply)
        f.st.opts.gui.schedule.set(6, 23, Mode::SeedOnly);
        unsafe { write_controls(f.dlg, &mut f.st) };
        f.click(ID_APPLY);
        assert_eq!(f.last_applied().gui.schedule.get(6, 23), Mode::SeedOnly);
    }
}
