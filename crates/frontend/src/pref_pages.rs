//! The Preferences dialog's pages as data: µTorrent 2.2.1's pages, their groups and the fields
//! on them, each bound to one value of [`Options`], laid out as the Win32 dialog lays them out.
//! A toolkit builds its controls from [`pages`] and moves values through their text form
//! ([`Field::shown`], [`Field::apply`]); [`Form`] applies only the fields the user changed (so a
//! value set on the Advanced page that a page shows rounded, such as a rate that is not a whole
//! kB/s, survives) and [`check`] finds what cannot be stored.
//!
//! The GTK dialog is built from it. The Win32 dialog keeps its own template (with the Windows
//! integration on General, which has no counterpart here) and the same rules.

use std::collections::HashMap;

use patorrent_core::{EncPolicy, ProxyType, Settings};

use crate::lang::{self, S};
use crate::options::Options;
use crate::settings::{self, Entry};

/// How a text field is edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind {
    Plain,
    Password,
    /// A folder, with a picker titled by the text.
    Folder(S),
    /// A file, with a picker titled by the text.
    File(S),
}

/// A control bound to one value of the options.
#[derive(Clone, Copy)]
pub enum Control {
    Check {
        get: fn(&Options) -> bool,
        set: fn(&mut Options, bool),
    },
    /// A whole number from `min` to `max`; `get` gives `None` for an empty field.
    Number {
        min: i64,
        max: i64,
        get: fn(&Options) -> Option<i64>,
        set: fn(&mut Options, i64),
    },
    Text {
        kind: TextKind,
        get: fn(&Options) -> String,
        set: fn(&mut Options, String),
    },
    /// A drop-down of `(value, text)` choices.
    Choice {
        choices: fn(&Options) -> Vec<(String, String)>,
        get: fn(&Options) -> String,
        set: fn(&mut Options, String),
    },
}

/// One field of a page.
#[derive(Clone, Copy)]
pub struct Field {
    /// Unique among all pages.
    pub key: &'static str,
    /// The check box's text, or the caption before the control; `None` for a control that
    /// belongs to the field before it (a folder under its check box).
    pub label: Option<S>,
    pub control: Control,
    /// Indented under the field before it.
    pub indent: bool,
    /// On the same row as the field before it (a number after its check box).
    pub beside: bool,
    /// Greyed unless these check boxes (keys) are all on.
    pub needs: &'static [&'static str],
    /// Always greyed: shown, never edited (what a package manages, [`Features::packaged`]).
    pub fixed: bool,
}

/// A button a toolkit acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Puts a random port into the `port` field.
    RandomPort,
    /// Loads the block list in the `ipfilter.path` field now.
    ReloadIpFilter,
}

#[derive(Clone, Copy)]
pub enum Item {
    /// Starts a group box; the fields after it are in it until the next group.
    Group(S),
    Field(Field),
    /// A line or two of explanation.
    Note(fn() -> String),
    /// A button beside the field before it.
    Button {
        action: Action,
        label: S,
    },
    /// The IP filter's status line, filled by the toolkit.
    IpFilterStatus,
    /// The Advanced page's table of every engine setting ([`advanced_rows`]).
    AdvancedTable,
    /// The Scheduler page's 7 × 24 table (`Options::gui.schedule.table`), which the toolkit
    /// draws and edits itself; greyed unless [`SCHED_ENABLE`] is on.
    SchedTable,
}

/// The Scheduler page's "Enable Scheduler" box, which greys the table and the rates.
pub const SCHED_ENABLE: &str = "sched.enable";

pub struct Page {
    pub key: &'static str,
    pub title: S,
    /// Shown under another page in the page tree (Disk Cache under Advanced).
    pub parent: Option<&'static str>,
    pub items: Vec<Item>,
}

/// What a toolkit offers beyond the shared fields.
#[derive(Clone, Copy, Debug, Default)]
pub struct Features {
    /// A tray icon: UI Settings shows the tray and balloon boxes.
    pub tray: bool,
    /// Desktop integration (associations, autostart): General shows its group, bound to
    /// `Options::integration`, which the toolkit reads from the system and writes back.
    pub desktop: bool,
    /// With `desktop`: the associations are the package's (a Flatpak, whose exported desktop
    /// entry declares the types): their boxes are greyed under a note saying so. Autostart
    /// stays editable: the toolkit asks the package's own means (the Background portal).
    pub packaged: bool,
}

const KB: i64 = 1024;
const MAX_KB: i64 = u32::MAX as i64 / KB;

/// The proxy "Type" choices, in µTorrent 2.2.1's order (its "HTTPS" entry, CONNECT for
/// everything, is our HTTP type). "Authentication" picks the `_pw` variant; SOCKS4 sends the
/// user name as its user id.
fn proxy_choices(_: &Options) -> Vec<(String, String)> {
    [tr_owned(S::PrefProxyNone), "Socks4".into(), "Socks5".into(), "HTTP".into()]
        .into_iter()
        .enumerate()
        .map(|(i, t)| (i.to_string(), t))
        .collect()
}

pub fn proxy_to_ui(s: &Settings) -> (usize, bool) {
    match s.proxy_type {
        ProxyType::None => (0, false),
        ProxyType::Socks4 => (1, !s.proxy_username.is_empty()),
        ProxyType::Socks5 => (2, false),
        ProxyType::Socks5Pw => (2, true),
        ProxyType::Http => (3, false),
        ProxyType::HttpPw => (3, true),
    }
}

pub fn proxy_from_ui(kind: usize, auth: bool) -> ProxyType {
    match (kind, auth) {
        (1, _) => ProxyType::Socks4,
        (2, false) => ProxyType::Socks5,
        (2, true) => ProxyType::Socks5Pw,
        (3, false) => ProxyType::Http,
        (3, true) => ProxyType::HttpPw,
        _ => ProxyType::None,
    }
}

/// "Outgoing" encryption, in the drop-down's order.
pub const ENC_OUT: [(S, EncPolicy); 3] = [
    (S::EncDisabled, EncPolicy::Disabled),
    (S::EncEnabled, EncPolicy::Enabled),
    (S::EncForced, EncPolicy::Forced),
];

fn enc_choices(_: &Options) -> Vec<(String, String)> {
    ENC_OUT.iter().enumerate().map(|(i, (t, _))| (i.to_string(), tr_owned(*t))).collect()
}

fn tr_owned(s: S) -> String {
    lang::text(s).to_owned()
}

fn language_choices(o: &Options) -> Vec<(String, String)> {
    lang::choices(&o.gui.language, &lang::system_languages())
}

const fn field(key: &'static str, label: Option<S>, control: Control) -> Field {
    Field { key, label, control, indent: false, beside: false, needs: &[], fixed: false }
}

const fn checkbox(
    key: &'static str,
    label: S,
    get: fn(&Options) -> bool,
    set: fn(&mut Options, bool),
) -> Field {
    field(key, Some(label), Control::Check { get, set })
}

const fn number(
    key: &'static str,
    label: Option<S>,
    min: i64,
    max: i64,
    get: fn(&Options) -> Option<i64>,
    set: fn(&mut Options, i64),
) -> Field {
    field(key, label, Control::Number { min, max, get, set })
}

const fn text(
    key: &'static str,
    label: Option<S>,
    kind: TextKind,
    get: fn(&Options) -> String,
    set: fn(&mut Options, String),
) -> Field {
    field(key, label, Control::Text { kind, get, set })
}

impl Field {
    const fn indent(mut self) -> Field {
        self.indent = true;
        self
    }

    const fn beside(mut self) -> Field {
        self.beside = true;
        self
    }

    const fn needs(mut self, keys: &'static [&'static str]) -> Field {
        self.needs = keys;
        self
    }

    const fn fixed(mut self, on: bool) -> Field {
        self.fixed = on;
        self
    }

    /// The value as its control shows it: `true` / `false`, the number (empty for none), the
    /// text, the choice's value.
    pub fn shown(&self, o: &Options) -> String {
        match self.control {
            Control::Check { get, .. } => get(o).to_string(),
            Control::Number { get, .. } => get(o).map(|v| v.to_string()).unwrap_or_default(),
            Control::Text { get, .. } | Control::Choice { get, .. } => get(o),
        }
    }

    /// Sets the value from its control's text; the message for the user when it is not one.
    pub fn apply(&self, o: &mut Options, value: &str) -> Result<(), String> {
        match self.control {
            Control::Check { set, .. } => set(o, value == "true"),
            Control::Number { min, max, set, .. } => match value.trim().parse::<i64>() {
                Ok(v) if (min..=max).contains(&v) => set(o, v),
                _ => return Err(crate::tr!(PrefBadNumber, min, max)),
            },
            Control::Text { set, .. } | Control::Choice { set, .. } => set(o, value.to_owned()),
        }
        Ok(())
    }

    /// Whether the control is enabled, with `on` telling whether a check box is ticked now.
    pub fn enabled(&self, on: impl Fn(&str) -> bool) -> bool {
        self.needs.iter().all(|k| on(k))
    }
}

/// Every page, in µTorrent 2.2.1's order, with what `features` allows.
pub fn pages(features: Features) -> Vec<Page> {
    use Item::{Field as F, Group, Note};
    let mut ui = vec![
        Group(S::PrefDisplay),
        F(checkbox(
            "ui.confirm_delete",
            S::PrefConfirmDelete,
            |o| o.gui.confirm_delete,
            |o, v| o.gui.confirm_delete = v,
        )),
    ];
    if features.tray {
        ui.extend([
            F(checkbox("ui.balloons", S::PrefBalloons, |o| o.gui.balloons, |o, v| o.gui.balloons = v)),
            Group(S::PrefTray),
            F(checkbox(
                "ui.close_to_tray",
                S::PrefCloseToTray,
                |o| o.gui.close_to_tray,
                |o, v| o.gui.close_to_tray = v,
            )),
            F(checkbox(
                "ui.minimize_to_tray",
                S::PrefMinimizeToTray,
                |o| o.gui.minimize_to_tray,
                |o, v| o.gui.minimize_to_tray = v,
            )),
        ]);
    }
    ui.extend([
        Group(S::PrefWhenAdding),
        F(checkbox("ui.add_stopped", S::PrefDontStart, |o| o.gui.add_stopped, |o, v| o.gui.add_stopped = v)),
        F(checkbox(
            "ui.show_add_dialog",
            S::PrefShowAddDialog,
            |o| o.gui.show_add_dialog,
            |o, v| o.gui.show_add_dialog = v,
        )),
    ]);
    let mut general = vec![
        Group(S::PrefLanguage),
        F(field(
            "ui.language",
            Some(S::PrefUiLanguage),
            Control::Choice {
                choices: language_choices,
                get: |o| o.gui.language.clone(),
                set: |o, v| o.gui.language = v.trim().to_owned(),
            },
        )),
    ];
    if features.desktop {
        general.push(Group(S::PrefDesktopIntegration));
        if features.packaged {
            general.push(Note(|| tr_owned(S::PrefDesktopPackaged)));
        }
        // the associations only: autostart goes through the package (see `Features::packaged`)
        let fixed = features.packaged;
        general.extend([
            F(checkbox(
                "assoc.torrent",
                S::PrefAssocTorrent,
                |o| o.integration.torrent,
                |o, v| o.integration.torrent = v,
            )
            .fixed(fixed)),
            F(checkbox(
                "assoc.magnet",
                S::PrefAssocMagnet,
                |o| o.integration.magnet,
                |o, v| o.integration.magnet = v,
            )
            .fixed(fixed)),
            F(checkbox(
                "ui.check_assoc",
                S::PrefCheckAssoc,
                |o| o.gui.check_association,
                |o, v| o.gui.check_association = v,
            )
            .fixed(fixed)),
            F(checkbox(
                "assoc.autostart",
                S::PrefAutostart,
                |o| o.integration.autostart,
                |o, v| o.integration.autostart = v,
            )),
            F(checkbox(
                "assoc.minimized",
                S::PrefStartMinimized,
                |o| o.integration.minimized,
                |o, v| o.integration.minimized = v,
            )
            .indent()
            .needs(&["assoc.autostart"])),
        ]);
    }
    vec![
        Page { key: "general", title: S::PageGeneral, parent: None, items: general },
        Page { key: "ui", title: S::PageUi, parent: None, items: ui },
        Page {
            key: "directories",
            title: S::PageDirectories,
            parent: None,
            items: vec![
                Group(S::PrefDownloadLocation),
                F(text(
                    "dir.download",
                    Some(S::PrefNewDownloadsIn),
                    TextKind::Folder(S::PickDownloadDir),
                    |o| o.gui.download_dir.clone(),
                    |o, v| o.gui.download_dir = v,
                )),
                F(checkbox(
                    "dir.move_completed",
                    S::PrefMoveCompleted,
                    |o| o.gui.move_completed,
                    |o, v| o.gui.move_completed = v,
                )),
                F(text(
                    "dir.completed",
                    None,
                    TextKind::Folder(S::PickCompletedDir),
                    |o| o.gui.completed_dir.clone(),
                    |o, v| o.gui.completed_dir = v.trim().to_owned(),
                )
                .needs(&["dir.move_completed"])),
                F(checkbox(
                    "dir.move_only_default",
                    S::PrefMoveOnlyDefault,
                    |o| o.gui.move_only_default,
                    |o, v| o.gui.move_only_default = v,
                )
                .needs(&["dir.move_completed"])),
                Group(S::PrefTorrentLocation),
                F(checkbox(
                    "dir.store_torrents",
                    S::PrefStoreTorrents,
                    |o| o.gui.torrent_folders.store,
                    |o, v| o.gui.torrent_folders.store = v,
                )),
                F(text(
                    "dir.torrents",
                    None,
                    TextKind::Folder(S::PickStoreDir),
                    |o| o.gui.torrent_folders.store_dir.clone(),
                    |o, v| o.gui.torrent_folders.store_dir = v.trim().to_owned(),
                )
                .needs(&["dir.store_torrents"])),
                F(checkbox(
                    "dir.move_torrents",
                    S::PrefMoveTorrents,
                    |o| o.gui.torrent_folders.move_finished,
                    |o, v| o.gui.torrent_folders.move_finished = v,
                )
                .indent()
                .needs(&["dir.store_torrents"])),
                F(text(
                    "dir.finished_torrents",
                    None,
                    TextKind::Folder(S::PickFinishedDir),
                    |o| o.gui.torrent_folders.finished_dir.clone(),
                    |o, v| o.gui.torrent_folders.finished_dir = v.trim().to_owned(),
                )
                .indent()
                .needs(&["dir.store_torrents", "dir.move_torrents"])),
                F(checkbox(
                    "dir.autoload",
                    S::PrefAutoload,
                    |o| o.gui.torrent_folders.autoload,
                    |o, v| o.gui.torrent_folders.autoload = v,
                )),
                F(text(
                    "dir.autoload_dir",
                    None,
                    TextKind::Folder(S::PickAutoloadDir),
                    |o| o.gui.torrent_folders.autoload_dir.clone(),
                    |o, v| o.gui.torrent_folders.autoload_dir = v.trim().to_owned(),
                )
                .needs(&["dir.autoload"])),
                F(checkbox(
                    "dir.autoload_delete",
                    S::PrefAutoloadDelete,
                    |o| o.gui.torrent_folders.autoload_delete,
                    |o, v| o.gui.torrent_folders.autoload_delete = v,
                )
                .indent()
                .needs(&["dir.autoload"])),
            ],
        },
        Page {
            key: "connection",
            title: S::PageConnection,
            parent: None,
            items: vec![
                Group(S::PrefListeningPort),
                F(number(
                    "port",
                    Some(S::PrefPort),
                    1,
                    65535,
                    |o| settings::listen_port(&o.settings).map(i64::from),
                    |o, v| settings::set_listen_port(&mut o.settings, v as u16),
                )),
                Item::Button { action: Action::RandomPort, label: S::PrefRandomPort },
                F(checkbox(
                    "net.random_port",
                    S::PrefRandomize,
                    |o| o.gui.random_port,
                    |o, v| o.gui.random_port = v,
                )),
                F(checkbox(
                    "enable_upnp",
                    S::PrefUpnp,
                    |o| o.settings.enable_upnp,
                    |o, v| o.settings.enable_upnp = v,
                )),
                F(checkbox(
                    "enable_natpmp",
                    S::PrefNatPmp,
                    |o| o.settings.enable_natpmp,
                    |o, v| o.settings.enable_natpmp = v,
                )),
                Group(S::PrefProxyServer),
                // each part of the type sets the type with the other part as it stands, so
                // both changed together count
                F(field(
                    "proxy.type",
                    Some(S::PrefProxyType),
                    Control::Choice {
                        choices: proxy_choices,
                        get: |o| proxy_to_ui(&o.settings).0.to_string(),
                        set: |o, v| {
                            let auth = proxy_to_ui(&o.settings).1;
                            o.settings.proxy_type = proxy_from_ui(v.parse().unwrap_or(0), auth);
                        },
                    },
                )),
                F(text(
                    "proxy_hostname",
                    Some(S::PrefProxy),
                    TextKind::Plain,
                    |o| o.settings.proxy_hostname.clone(),
                    |o, v| o.settings.proxy_hostname = v.trim().to_owned(),
                )),
                F(number(
                    "proxy_port",
                    Some(S::PrefProxyPort),
                    0,
                    65535,
                    |o| Some(o.settings.proxy_port.into()),
                    |o, v| o.settings.proxy_port = v as u16,
                )),
                F(checkbox(
                    "proxy.auth",
                    S::PrefAuthentication,
                    |o| proxy_to_ui(&o.settings).1,
                    |o, v| {
                        let kind = proxy_to_ui(&o.settings).0;
                        o.settings.proxy_type = proxy_from_ui(kind, v);
                    },
                )),
                F(text(
                    "proxy_username",
                    Some(S::PrefUsername),
                    TextKind::Plain,
                    |o| o.settings.proxy_username.clone(),
                    |o, v| o.settings.proxy_username = v,
                )
                .indent()),
                F(text(
                    "proxy_password",
                    Some(S::PrefPassword),
                    TextKind::Password,
                    |o| o.settings.proxy_password.clone(),
                    |o, v| o.settings.proxy_password = v,
                )
                .indent()),
                F(checkbox(
                    "proxy_hostnames",
                    S::PrefProxyNames,
                    |o| o.settings.proxy_hostnames,
                    |o, v| o.settings.proxy_hostnames = v,
                )),
                F(checkbox(
                    "proxy_peer_connections",
                    S::PrefProxyPeers,
                    |o| o.settings.proxy_peer_connections,
                    |o, v| o.settings.proxy_peer_connections = v,
                )),
                F(checkbox(
                    "proxy_tracker_connections",
                    S::PrefProxyTrackers,
                    |o| o.settings.proxy_tracker_connections,
                    |o, v| o.settings.proxy_tracker_connections = v,
                )),
            ],
        },
        Page {
            key: "bandwidth",
            title: S::PageBandwidth,
            parent: None,
            items: vec![
                Group(S::PrefUpLimiting),
                F(number(
                    "upload_rate_limit",
                    Some(S::PrefUpRate),
                    0,
                    MAX_KB,
                    |o| Some(o.settings.upload_rate_limit as i64 / KB),
                    |o, v| o.settings.upload_rate_limit = (v * KB) as u32,
                )),
                Group(S::PrefDownLimiting),
                F(number(
                    "download_rate_limit",
                    Some(S::PrefDownRate),
                    0,
                    MAX_KB,
                    |o| Some(o.settings.download_rate_limit as i64 / KB),
                    |o, v| o.settings.download_rate_limit = (v * KB) as u32,
                )),
                Group(S::PrefConnections),
                F(number(
                    "connections_limit",
                    Some(S::PrefMaxConnections),
                    2,
                    65535,
                    |o| Some(o.settings.connections_limit.into()),
                    |o, v| o.settings.connections_limit = v as i32,
                )),
                F(number(
                    "bt.max_peers",
                    Some(S::PrefPeersPerTorrent),
                    0,
                    65535,
                    |o| Some(o.gui.max_peers_per_torrent.into()),
                    |o, v| o.gui.max_peers_per_torrent = v as u32,
                )),
                F(number(
                    "bt.upload_slots",
                    Some(S::PrefSlotsPerTorrent),
                    0,
                    65535,
                    |o| Some(o.gui.upload_slots_per_torrent.into()),
                    |o, v| o.gui.upload_slots_per_torrent = v as u32,
                )),
                F(number(
                    "unchoke_slots_limit",
                    Some(S::PrefGlobalSlots),
                    -1,
                    65535,
                    |o| Some(o.settings.unchoke_slots_limit.into()),
                    |o, v| o.settings.unchoke_slots_limit = v as i32,
                )),
                Note(|| tr_owned(S::PrefPerTorrentNote)),
            ],
        },
        Page {
            key: "bittorrent",
            title: S::PageBitTorrent,
            parent: None,
            items: vec![
                Group(S::PrefBasicFeatures),
                F(checkbox(
                    "enable_dht",
                    S::PrefDht,
                    |o| o.settings.enable_dht,
                    |o, v| o.settings.enable_dht = v,
                )),
                F(checkbox(
                    "enable_lsd",
                    S::PrefLsd,
                    |o| o.settings.enable_lsd,
                    |o, v| o.settings.enable_lsd = v,
                )),
                F(checkbox("bt.pex", S::PrefPex, |o| o.gui.pex, |o, v| o.gui.pex = v)),
                // uTP both ways; the four transports one by one are bt.transp_disposition on
                // Advanced
                F(checkbox(
                    "utp",
                    S::PrefUtp,
                    |o| o.settings.enable_outgoing_utp || o.settings.enable_incoming_utp,
                    |o, v| (o.settings.enable_outgoing_utp, o.settings.enable_incoming_utp) = (v, v),
                )),
                Group(S::PrefEncryption),
                F(field(
                    "enc.out",
                    Some(S::PrefOutgoing),
                    Control::Choice {
                        choices: enc_choices,
                        get: |o| {
                            ENC_OUT
                                .iter()
                                .position(|(_, p)| *p == o.settings.out_enc_policy)
                                .unwrap_or(1)
                                .to_string()
                        },
                        set: |o, v| {
                            let policy = ENC_OUT
                                .get(v.parse::<usize>().unwrap_or(1))
                                .map_or(EncPolicy::Enabled, |e| e.1);
                            let legacy = settings::allow_legacy_incoming(&o.settings);
                            settings::set_encryption(&mut o.settings, policy, legacy);
                        },
                    },
                )),
                F(checkbox(
                    "enc.legacy",
                    S::PrefLegacy,
                    |o| settings::allow_legacy_incoming(&o.settings),
                    |o, v| {
                        let policy = o.settings.out_enc_policy;
                        settings::set_encryption(&mut o.settings, policy, v);
                    },
                )),
                Group(S::PrefIpFilter),
                F(checkbox(
                    "ipfilter.enable",
                    S::PrefIpFilterEnable,
                    |o| o.gui.ip_filter,
                    |o, v| o.gui.ip_filter = v,
                )),
                F(text(
                    "ipfilter.path",
                    None,
                    TextKind::File(S::PrefIpFilter),
                    |o| o.gui.ip_filter_path.clone(),
                    |o, v| o.gui.ip_filter_path = v.trim().to_owned(),
                )),
                Item::Button { action: Action::ReloadIpFilter, label: S::PrefReload },
                Item::IpFilterStatus,
            ],
        },
        Page {
            key: "queueing",
            title: S::PageQueueing,
            parent: None,
            items: vec![
                Group(S::PrefQueue),
                F(number(
                    "active_limit",
                    Some(S::PrefActive),
                    -1,
                    65535,
                    |o| Some(o.settings.active_limit.into()),
                    |o, v| o.settings.active_limit = v as i32,
                )),
                F(number(
                    "active_downloads",
                    Some(S::PrefActiveDownloads),
                    -1,
                    65535,
                    |o| Some(o.settings.active_downloads.into()),
                    |o, v| o.settings.active_downloads = v as i32,
                )),
                F(number(
                    "active_seeds",
                    Some(S::PrefActiveSeeds),
                    -1,
                    65535,
                    |o| Some(o.settings.active_seeds.into()),
                    |o, v| o.settings.active_seeds = v as i32,
                )),
                F(checkbox(
                    "dont_count_slow_torrents",
                    S::PrefSlow,
                    |o| o.settings.dont_count_slow_torrents,
                    |o, v| o.settings.dont_count_slow_torrents = v,
                )),
                Group(S::PrefSeedingGoal),
                F(number(
                    "queue.seed_ratio",
                    Some(S::PrefRatio),
                    0,
                    1_000_000,
                    |o| Some(o.gui.seed_ratio.into()),
                    |o, v| o.gui.seed_ratio = v as u32,
                )),
                F(number(
                    "queue.seed_time",
                    Some(S::PrefSeedTime),
                    0,
                    1_000_000,
                    |o| Some(o.gui.seed_minutes.into()),
                    |o, v| o.gui.seed_minutes = v as u32,
                )),
                Note(|| tr_owned(S::PrefGoalReached)),
                F(checkbox(
                    "queue.seed_limit",
                    S::PrefGoalLimit,
                    |o| o.gui.seed_limit_upload,
                    |o, v| o.gui.seed_limit_upload = v,
                )),
                F(number(
                    "queue.seed_limit_kib",
                    None,
                    0,
                    MAX_KB,
                    |o| Some(o.gui.seed_limit_kib.into()),
                    |o, v| o.gui.seed_limit_kib = v as u32,
                )
                .beside()),
            ],
        },
        Page {
            key: "scheduler",
            title: S::PageScheduler,
            parent: None,
            items: vec![
                F(checkbox(
                    SCHED_ENABLE,
                    S::PrefScheduler,
                    |o| o.gui.schedule.enabled,
                    |o, v| o.gui.schedule.enabled = v,
                )),
                Group(S::PrefSchedTable),
                Item::SchedTable,
                Group(S::PrefSchedSettings),
                F(number(
                    "sched.ul_rate",
                    Some(S::PrefSchedUp),
                    0,
                    MAX_KB,
                    |o| Some(o.gui.schedule.limited_up_kib.into()),
                    |o, v| o.gui.schedule.limited_up_kib = v as u32,
                )
                .needs(&[SCHED_ENABLE])),
                F(number(
                    "sched.dl_rate",
                    Some(S::PrefSchedDown),
                    0,
                    MAX_KB,
                    |o| Some(o.gui.schedule.limited_down_kib.into()),
                    |o, v| o.gui.schedule.limited_down_kib = v as u32,
                )
                .needs(&[SCHED_ENABLE])),
                F(checkbox(
                    "sched.dis_dht",
                    S::PrefSchedDht,
                    |o| o.gui.schedule.disable_dht,
                    |o, v| o.gui.schedule.disable_dht = v,
                )
                .needs(&[SCHED_ENABLE])),
            ],
        },
        Page {
            key: "advanced",
            title: S::PageAdvanced,
            parent: None,
            items: vec![Item::AdvancedTable, Note(|| tr_owned(S::PrefAdvancedNote))],
        },
        Page {
            key: "disk_cache",
            title: S::PageDiskCache,
            parent: Some("advanced"),
            items: vec![
                Group(S::PrefReadCache),
                F(checkbox(
                    "use_read_cache",
                    S::PrefReadCacheEnable,
                    |o| o.settings.use_read_cache,
                    |o, v| o.settings.use_read_cache = v,
                )),
                // each box before its number, so a box and a number changed together both count
                F(checkbox(
                    "cache.override",
                    S::PrefCacheOverride,
                    |o| settings::cache_size_overridden(&o.settings),
                    |o, v| {
                        let mb = settings::cache_size_mb(&o.settings);
                        settings::set_cache_size(&mut o.settings, v, mb)
                    },
                )
                .needs(&["use_read_cache"])),
                F(number(
                    "cache.mb",
                    None,
                    1,
                    65535,
                    |o| Some(settings::cache_size_mb(&o.settings).into()),
                    |o, v| {
                        let on = settings::cache_size_overridden(&o.settings);
                        settings::set_cache_size(&mut o.settings, on, v as u32)
                    },
                )
                .beside()
                .needs(&["use_read_cache", "cache.override"])),
                F(checkbox(
                    "cache.expire",
                    S::PrefCacheExpire,
                    |o| settings::cache_expiry_on(&o.settings),
                    |o, v| {
                        let secs = settings::cache_expiry_secs(&o.settings);
                        settings::set_cache_expiry(&mut o.settings, v, secs)
                    },
                )
                .needs(&["use_read_cache"])),
                F(number(
                    "cache.expiry",
                    None,
                    1,
                    86400,
                    |o| Some(settings::cache_expiry_secs(&o.settings).into()),
                    |o, v| {
                        let on = settings::cache_expiry_on(&o.settings);
                        settings::set_cache_expiry(&mut o.settings, on, v as u32)
                    },
                )
                .beside()
                .needs(&["use_read_cache", "cache.expire"])),
                Group(S::PrefDiskQueue),
                F(number(
                    "max_queued_disk_bytes",
                    Some(S::PrefDiskWriteQueue),
                    1,
                    MAX_KB,
                    |o| Some(o.settings.max_queued_disk_bytes as i64 / KB),
                    |o, v| o.settings.max_queued_disk_bytes = (v * KB) as u32,
                )),
                F(number(
                    "file_pool_size",
                    Some(S::PrefOpenFiles),
                    1,
                    65535,
                    |o| Some(o.settings.file_pool_size.into()),
                    |o, v| o.settings.file_pool_size = v as u32,
                )),
                F(number(
                    "aio_threads",
                    Some(S::PrefAioThreads),
                    1,
                    64,
                    |o| Some(o.settings.aio_threads.into()),
                    |o, v| o.settings.aio_threads = v as u32,
                )),
                F(number(
                    "hashing_threads",
                    Some(S::PrefHashThreads),
                    1,
                    64,
                    |o| Some(o.settings.hashing_threads.into()),
                    |o, v| o.settings.hashing_threads = v as u32,
                )),
                Note(|| crate::tr!(PrefReadCacheNote, mb = settings::AUTO_CACHE_MB)),
            ],
        },
    ]
}

/// The fields of `pages`, in page order.
pub fn fields(pages: &[Page]) -> impl Iterator<Item = &Field> {
    pages.iter().flat_map(|p| &p.items).filter_map(|i| match i {
        Item::Field(f) => Some(f),
        _ => None,
    })
}

/// The page holding the field `key`.
pub fn page_of<'a>(pages: &'a [Page], key: &str) -> Option<&'a Page> {
    pages.iter().find(|p| p.items.iter().any(|i| matches!(i, Item::Field(f) if f.key == key)))
}

/// What cannot be stored: the field to fix and why.
pub fn check(o: &Options) -> Result<(), (&'static str, String)> {
    let s = &o.settings;
    if s.proxy_type != ProxyType::None {
        if s.proxy_hostname.is_empty() {
            return Err(("proxy_hostname", tr_owned(S::PrefNoProxyHost)));
        }
        if s.proxy_port == 0 {
            return Err(("proxy_port", tr_owned(S::PrefNoProxyPort)));
        }
    }
    let g = &o.gui;
    if g.download_dir.trim().is_empty() {
        return Err(("dir.download", tr_owned(S::PrefNoDownloadDir)));
    }
    if g.move_completed && g.completed_dir.is_empty() {
        return Err(("dir.completed", tr_owned(S::PrefNoCompletedDir)));
    }
    let tf = &g.torrent_folders;
    if tf.store && tf.store_dir.is_empty() {
        return Err(("dir.torrents", tr_owned(S::PrefNoStoreDir)));
    }
    if tf.store && tf.move_finished && tf.finished_dir.is_empty() {
        return Err(("dir.finished_torrents", tr_owned(S::PrefNoFinishedDir)));
    }
    if tf.autoload && tf.autoload_dir.is_empty() {
        return Err(("dir.autoload_dir", tr_owned(S::PrefNoAutoloadDir)));
    }
    Ok(())
}

/// The fields' values as last written to the controls, so only what the user changed is
/// applied.
#[derive(Clone, Debug, Default)]
pub struct Form {
    shown: HashMap<&'static str, String>,
}

impl Form {
    /// The values to write into the controls, remembered.
    pub fn write(&mut self, pages: &[Page], o: &Options) -> Vec<(&'static str, String)> {
        let values: Vec<(&'static str, String)> = fields(pages).map(|f| (f.key, f.shown(o))).collect();
        self.shown = values.iter().cloned().collect();
        values
    }

    /// `o` with the fields whose control (`now`) differs from what was written, applied in
    /// page order, then [`check`]ed. On a bad value nothing is changed and the field's key and
    /// a message come back.
    pub fn read(
        &self,
        pages: &[Page],
        o: &Options,
        now: impl Fn(&'static str) -> String,
    ) -> Result<Options, (&'static str, String)> {
        let mut new = o.clone();
        for f in fields(pages) {
            let value = now(f.key);
            if self.shown.get(f.key) == Some(&value) {
                continue;
            }
            f.apply(&mut new, &value).map_err(|e| (f.key, e))?;
        }
        check(&new)?;
        Ok(new)
    }
}

/// The Advanced table's rows: every engine setting whose name contains `filter` (any case),
/// by name.
pub fn advanced_rows(filter: &str) -> Vec<&'static Entry> {
    let filter = filter.trim().to_lowercase();
    let mut rows: Vec<&'static Entry> =
        settings::TABLE.iter().filter(|e| filter.is_empty() || e.name.contains(&filter)).collect();
    rows.sort_by_key(|e| e.name);
    rows
}

/// A row's name, with `*` when it differs from the default.
pub fn advanced_name(e: &Entry, s: &Settings) -> String {
    if e.is_default(s) { e.name.to_owned() } else { format!("{}*", e.name) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::Prefs;

    fn opts() -> Options {
        Options::load(&Prefs::default(), "/dl")
    }

    #[test]
    fn keys_are_unique_and_every_value_round_trips() {
        let pages = pages(Features { tray: true, desktop: true, packaged: false });
        let mut seen = std::collections::HashSet::new();
        for f in fields(&pages) {
            assert!(seen.insert(f.key), "{} twice", f.key);
            for k in f.needs {
                let needed = fields(&pages).find(|g| g.key == *k).unwrap_or_else(|| panic!("{k}"));
                assert!(matches!(needed.control, Control::Check { .. }), "{k} is not a check box");
            }
            // what a control shows, applied back, changes nothing
            let o = opts();
            let mut again = o.clone();
            let shown = f.shown(&o);
            if !shown.is_empty() {
                f.apply(&mut again, &shown).unwrap_or_else(|e| panic!("{}: {e}", f.key));
            }
            assert!(again == o, "{} does not round-trip", f.key);
        }
        assert!(seen.contains("ui.close_to_tray"));
        let without = super::pages(Features::default());
        assert!(fields(&without).all(|f| !f.key.contains("tray") && f.key != "ui.balloons"));
        assert!(fields(&without).all(|f| !f.key.starts_with("assoc.") && f.key != "ui.check_assoc"));
        assert_eq!(page_of(&pages, "assoc.minimized").map(|p| p.key), Some("general"));
        assert!(fields(&pages).all(|f| !f.fixed));
        // in a Flatpak the same boxes under the note, the associations greyed, autostart live
        let packaged = super::pages(Features { tray: true, desktop: true, packaged: true });
        let desktop = |f: &&Field| f.key.starts_with("assoc.") || f.key == "ui.check_assoc";
        assert_eq!(fields(&packaged).filter(desktop).count(), 5);
        let associations = ["assoc.torrent", "assoc.magnet", "ui.check_assoc"];
        assert!(fields(&packaged).all(|f| f.fixed == associations.contains(&f.key)));
        let general = &packaged.iter().find(|p| p.key == "general").unwrap().items;
        assert!(
            general.iter().any(|i| matches!(i, Item::Note(t) if t() == lang::text(S::PrefDesktopPackaged)))
        );
        assert_eq!(page_of(&without, "cache.mb").map(|p| p.key), Some("disk_cache"));
        assert_eq!(without.iter().find(|p| p.key == "disk_cache").unwrap().parent, Some("advanced"));
        let order: Vec<&str> = without.iter().map(|p| p.key).collect();
        assert_eq!(order[6..], ["queueing", "scheduler", "advanced", "disk_cache"]);
    }

    #[test]
    fn the_scheduler_page_edits_the_schedule() {
        let pages = pages(Features::default());
        let sched = pages.iter().find(|p| p.key == "scheduler").unwrap();
        assert!(sched.items.iter().any(|i| matches!(i, Item::SchedTable)));
        let o = opts();
        let mut form = Form::default();
        let shown: HashMap<_, _> = form.write(&pages, &o).into_iter().collect();
        assert_eq!(shown[SCHED_ENABLE], "false");
        let edited = |k: &str| match k {
            SCHED_ENABLE => "true".to_owned(),
            "sched.ul_rate" => "20".to_owned(),
            "sched.dl_rate" => "300".to_owned(),
            "sched.dis_dht" => "false".to_owned(),
            _ => shown[k].clone(),
        };
        let s = form.read(&pages, &o, edited).unwrap().gui.schedule;
        assert!(s.enabled && !s.disable_dht);
        assert_eq!((s.limited_up_kib, s.limited_down_kib), (20, 300));
        let rate = fields(&pages).find(|f| f.key == "sched.ul_rate").unwrap();
        assert!(!rate.enabled(|_| false) && rate.enabled(|k| k == SCHED_ENABLE));
    }

    #[test]
    fn only_changed_fields_are_applied() {
        let pages = pages(Features::default());
        let mut o = opts();
        // 1.5 kB/s, which the Bandwidth page shows as 1
        o.settings.upload_rate_limit = 1536;
        let mut form = Form::default();
        let shown: HashMap<_, _> = form.write(&pages, &o).into_iter().collect();
        assert_eq!(shown["upload_rate_limit"], "1");
        let unchanged = form.read(&pages, &o, |k| shown[k].clone()).unwrap();
        assert_eq!(unchanged.settings.upload_rate_limit, 1536, "untouched, not rounded");

        let edited = |k: &str| match k {
            "download_rate_limit" => "20".to_owned(),
            "enable_dht" => "false".to_owned(),
            "ui.show_add_dialog" => "false".to_owned(),
            _ => shown[k].clone(),
        };
        let new = form.read(&pages, &o, edited).unwrap();
        assert_eq!(new.settings.download_rate_limit, 20 * 1024);
        assert!(!new.settings.enable_dht && !new.gui.show_add_dialog);
        assert_eq!(new.settings.upload_rate_limit, 1536);
    }

    #[test]
    fn composite_fields_changed_together_both_count() {
        let pages = pages(Features::default());
        let o = opts();
        let mut form = Form::default();
        let shown: HashMap<_, _> = form.write(&pages, &o).into_iter().collect();
        assert_eq!((shown["proxy.type"].as_str(), shown["proxy.auth"].as_str()), ("0", "false"));
        let edited = |k: &str| match k {
            "proxy.type" => "2".to_owned(),
            "proxy.auth" => "true".to_owned(),
            "proxy_hostname" => " proxy.lan ".to_owned(),
            "proxy_port" => "1080".to_owned(),
            "cache.override" => "true".to_owned(),
            "cache.mb" => "100".to_owned(),
            "enc.out" => "2".to_owned(),
            "enc.legacy" => "false".to_owned(),
            _ => shown[k].clone(),
        };
        let new = form.read(&pages, &o, edited).unwrap();
        assert_eq!(new.settings.proxy_type, ProxyType::Socks5Pw);
        assert_eq!((new.settings.proxy_hostname.as_str(), new.settings.proxy_port), ("proxy.lan", 1080));
        assert!(settings::cache_size_overridden(&new.settings));
        assert_eq!(settings::cache_size_mb(&new.settings), 100);
        assert_eq!(new.settings.out_enc_policy, EncPolicy::Forced);
        assert!(!settings::allow_legacy_incoming(&new.settings));
    }

    #[test]
    fn bad_values_name_their_field() {
        let pages = pages(Features::default());
        let o = opts();
        let mut form = Form::default();
        let shown: HashMap<_, _> = form.write(&pages, &o).into_iter().collect();
        let bad_port = |k: &str| if k == "port" { "70000".to_owned() } else { shown[k].clone() };
        let (key, message) = form.read(&pages, &o, bad_port).unwrap_err();
        assert_eq!(key, "port");
        assert_eq!(message, crate::tr!(PrefBadNumber, min = 1, max = 65535));
        let no_host = |k: &str| if k == "proxy.type" { "1".to_owned() } else { shown[k].clone() };
        assert_eq!(form.read(&pages, &o, no_host).unwrap_err().0, "proxy_hostname");
        let no_folder = |k: &str| if k == "dir.autoload" { "true".to_owned() } else { shown[k].clone() };
        assert_eq!(form.read(&pages, &o, no_folder).unwrap_err().0, "dir.autoload_dir");
        assert_eq!(form.read(&pages, &o, |k| shown[k].clone()).map(|n| n == o), Ok(true));
    }

    #[test]
    fn greying_follows_the_boxes() {
        let pages = pages(Features::default());
        let f = fields(&pages).find(|f| f.key == "cache.mb").unwrap();
        assert!(f.enabled(|k| k == "use_read_cache" || k == "cache.override"));
        assert!(!f.enabled(|k| k == "use_read_cache"));
        assert!(fields(&pages).find(|f| f.key == "port").unwrap().enabled(|_| false));
    }

    #[test]
    fn the_advanced_table() {
        let all = advanced_rows("");
        assert_eq!(all.len(), settings::TABLE.len());
        assert!(all.windows(2).all(|w| w[0].name <= w[1].name));
        let dht = advanced_rows(" DHT ");
        assert!(!dht.is_empty() && dht.iter().all(|e| e.name.contains("dht")));
        let mut s = Settings::default();
        let e = settings::TABLE.iter().find(|e| e.name == "enable_dht").unwrap();
        assert_eq!(advanced_name(e, &s), "enable_dht");
        e.set(&mut s, "false").unwrap();
        assert_eq!(advanced_name(e, &s), "enable_dht*");
    }
}
